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

use crate::policy::WatermarkState;
use crate::types::{DeadlineNs, NodeIndex};

/// Readiness level derived from inputs and backpressure state.
///
/// Contract followed by runtimes:
/// - `NotReady`: all inputs empty (no work).
/// - `ReadyUnderPressure`: some input available AND at least one output watermark is
///   `BetweenSoftAndHard` or `AtOrAboveHard`.
/// - `Ready`: some input available AND all outputs are `BelowSoft`.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeSummary {
    /// Index of the node in the graph.
    pub index: NodeIndex,
    /// Earliest absolute deadline (if any) among its ready inputs.
    pub earliest_deadline: Option<DeadlineNs>,
    /// Readiness level.
    pub readiness: Readiness,
    /// Current backpressure state from outputs.
    pub backpressure: WatermarkState,
}

/// A dequeue policy selects the next node to run from a set of summaries.
pub trait DequeuePolicy {
    /// Select the next node index to step, or `None` if none should run.
    fn select_next(&mut self, candidates: &[NodeSummary]) -> Option<NodeIndex>;
}
