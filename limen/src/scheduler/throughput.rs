//! Throughput-oriented dequeue policy.
use limen_core::scheduling::{DequeuePolicy, NodeSummary, Readiness};
use limen_core::types::NodeIndex;

/// Simple throughput policy: prefer Ready over ReadyUnderPressure, with round-robin tie-breaking.
#[derive(Debug, Default)]
pub struct ThroughputPolicy {
    rr: usize,
}

impl DequeuePolicy for ThroughputPolicy {
    fn select_next(&mut self, candidates: &[NodeSummary]) -> Option<NodeIndex> {
        let n = candidates.len().max(1);
        // First pass: Ready
        for off in 0..candidates.len() {
            let i = (self.rr + off) % n;
            if candidates[i].readiness == Readiness::Ready {
                self.rr = self.rr.wrapping_add(1);
                return Some(candidates[i].index);
            }
        }
        // Second pass: ReadyUnderPressure
        for off in 0..candidates.len() {
            let i = (self.rr + off) % n;
            if candidates[i].readiness == Readiness::ReadyUnderPressure {
                self.rr = self.rr.wrapping_add(1);
                return Some(candidates[i].index);
            }
        }
        None
    }
}
