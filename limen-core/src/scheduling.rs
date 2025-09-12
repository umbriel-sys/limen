//! Scheduling traits: readiness and dequeue policy (EDF hooks).
//!
//! Concrete schedulers live in `limen-light` (P0/P1) and `limen` (P2).

use crate::types::{DeadlineNs, NodeIndex};
use crate::policy::WatermarkState;

/// Readiness level derived from inputs and backpressure state.
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
