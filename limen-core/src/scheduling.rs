//! Scheduling traits: readiness and dequeue policy (EDF hooks).
//!
//! How a runtime composes `NodeSummary` (contract):
//!
//! - `readiness`
//!   * Compute from **input occupancies** and **output pressure**.
//!   * If all inputs empty → `NotReady`.
//!   * If any input non-empty AND any output watermark ≥ soft → `ReadyUnderPressure`.
//!   * If any input non-empty AND all outputs below soft → `Ready`.
//!
//! - `backpressure`
//!   * Take the **max** watermark over all outputs (BelowSoft < BetweenSoftAndHard < AtOrAboveHard).
//!
//! - `earliest_deadline`
//!   * If any input has an absolute deadline → use the **minimum** of present deadlines.
//!   * Else if `NodePolicy.deadline.default_deadline_ns` is `Some(d)` → synthesize as `now + d`.
//!   * Else → `None`.
//!
//! Notes:
//! * Watermarks come from edge occupancy snapshots (`EdgeOccupancy`), which the runtime fills
//!   via `GraphApi::write_all_edge_occupancies` and refreshes via `refresh_occupancies_for_node`.
//! * Dequeue strategies (FIFO, EDF, QoS-weighted) live in runtimes; `DequeuePolicy` stays unchanged.

use crate::node::StepResult;
use crate::policy::WatermarkState;
use crate::types::{DeadlineNs, NodeIndex, Ticks};

/// Readiness level derived from inputs and backpressure state.
///
/// Contract followed by runtimes:
/// - `NotReady`: all inputs empty (no work).
/// - `ReadyUnderPressure`: some input available AND at least one output watermark is
///   `BetweenSoftAndHard` or `AtOrAboveHard`.
/// - `Ready`: some input available AND all outputs are `BelowSoft`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// No input available or blocked by backpressure.
    NotReady,
    /// Input(s) available but under pressure.
    ReadyUnderPressure,
    /// Input(s) available and not under pressure.
    Ready,
}

/// A minimal summary of a node for scheduling decisions.
///
/// - `index`: the node identifier in the graph.
/// - `earliest_deadline`: minimum absolute deadline among ready inputs, or synthesized `now + default_deadline_ns`
///   if policy provides a default and inputs lack deadlines; otherwise `None`.
/// - `readiness`: derived from input availability and output pressure (see `Readiness` contract above).
/// - `backpressure`: the maximum watermark state across all outputs.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeSummary {
    /// Index of the node in the graph.
    index: NodeIndex,
    /// Earliest absolute deadline (if any) among its ready inputs.
    earliest_deadline: Option<DeadlineNs>,
    /// Readiness level.
    readiness: Readiness,
    /// Current backpressure state from outputs.
    backpressure: WatermarkState,
}

impl NodeSummary {
    /// Constructs a new `NodeSummary` from the provided fields.
    pub const fn new(
        index: NodeIndex,
        earliest_deadline: Option<DeadlineNs>,
        readiness: Readiness,
        backpressure: WatermarkState,
    ) -> Self {
        Self {
            index,
            earliest_deadline,
            readiness,
            backpressure,
        }
    }

    /// Index of the node in the graph.
    #[inline]
    pub const fn index(&self) -> &NodeIndex {
        &self.index
    }

    /// Earliest absolute deadline (if any) among its ready inputs.
    #[inline]
    pub const fn earliest_deadline(&self) -> &Option<DeadlineNs> {
        &self.earliest_deadline
    }

    /// Readiness level.
    #[inline]
    pub const fn readiness(&self) -> &Readiness {
        &self.readiness
    }

    /// Current backpressure state from outputs.
    #[inline]
    pub const fn backpressure(&self) -> &WatermarkState {
        &self.backpressure
    }
}

/// A dequeue policy selects the next node to run from a set of summaries.
pub trait DequeuePolicy {
    /// Select the next node index to step, or `None` if none should run.
    fn select_next(&mut self, candidates: &[NodeSummary]) -> Option<NodeIndex>;
}

// ---------------------------------------------------------------------------
// Concurrent worker scheduling
// ---------------------------------------------------------------------------

/// Scheduling decision for a concurrent worker.
///
/// Returned by [`WorkerScheduler::decide`] to control per-worker stepping in
/// scoped concurrent execution.
///
/// `#[non_exhaustive]` — future variants will be added as planned work lands:
/// - `StepBatch(usize)` (C1 batch semantics — step N messages in one call)
/// - `StepWithBudget { max_micros: u64 }` (R11 execution measurement)
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerDecision {
    /// Step the node once.
    Step,
    /// Wait before calling `decide()` again. Duration in microseconds.
    WaitMicros(u64),
    /// This worker should exit.
    Stop,
}

/// Per-worker state snapshot for scheduling decisions.
///
/// Populated by the codegen-generated worker loop before each
/// [`WorkerScheduler::decide`] call. Edge occupancy is queried from concrete
/// edge types (no dyn dispatch) and summarised here.
///
/// **Extensibility model:** `#[non_exhaustive]` — fields will be added as
/// planned work lands. The codegen loop queries edges using concrete types
/// (no dyn) and populates this snapshot. The scheduler never touches edges
/// directly — it only sees the snapshot. New edge capabilities
/// (peek_header, mailbox, urgency) are exposed by adding fields here, with
/// the codegen loop doing the actual querying.
///
/// **Planned extensions (one field per planned item):**
/// - `earliest_deadline: Option<DeadlineNs>` (R1 — from peeked input headers)
/// - `max_input_urgency: Option<Urgency>` (R1 — highest urgency among inputs)
/// - `inputs_fresh: bool` (R2 — all inputs within max_age threshold)
/// - `input_liveness_ok: bool` (R5 — all inputs within liveness threshold)
/// - `micros_since_last_step: u64` (R6 — for node liveness violation detection)
/// - `criticality: CriticalityClass` (R8 — node's criticality tier)
/// - `input_count: usize` / `inputs_ready_count: usize` (C2 — N→M per-port)
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct WorkerState {
    /// Index of this node in the graph.
    pub node_index: usize,
    /// Total number of nodes in the graph.
    pub node_count: usize,
    /// Current clock tick (monotonic). Enables freshness/liveness calculations
    /// without the scheduler needing a clock reference.
    pub current_tick: Ticks,
    /// Readiness derived from input availability and output pressure.
    pub readiness: Readiness,
    /// Max output backpressure (watermark across all outputs).
    pub backpressure: WatermarkState,
    /// Result of the last step, if any.
    pub last_step: Option<StepResult>,
    /// Whether the last step errored (NodeError details go to telemetry).
    pub last_error: bool,
}

impl WorkerState {
    /// Construct a new initial worker state for a given node.
    #[inline]
    pub const fn new(node_index: usize, node_count: usize, current_tick: Ticks) -> Self {
        Self {
            node_index,
            node_count,
            current_tick,
            readiness: Readiness::NotReady,
            backpressure: WatermarkState::BelowSoft,
            last_step: None,
            last_error: false,
        }
    }
}

/// Per-worker scheduling for concurrent execution.
///
/// Called from worker threads via **static dispatch** — the concrete scheduler
/// type is a generic parameter on `ScopedGraphApi::run_scoped`, so no `dyn`.
///
/// Sequential runtimes use [`DequeuePolicy`] (centralized, one node per tick).
/// Concurrent runtimes use `WorkerScheduler` (per-worker, one decision per step).
pub trait WorkerScheduler: Send + Sync {
    /// Decide what this worker should do next.
    fn decide(&self, state: &WorkerState) -> WorkerDecision;
}
