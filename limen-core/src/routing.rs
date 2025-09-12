//! Fan-out (split) and fan-in (join) operator traits.
//!
//! Implementations live in `limen-processing`. These traits allow developers
//! to plug custom routing/aggregation logic (copy, scatter, concat, sum, learned).

use crate::message::Payload;
use crate::types::Ticks;

/// A split operator transforms one input payload into N output payloads.
pub trait SplitOperator<InP: Payload, OutP: Payload, const N: usize> {
    /// Perform the split operation, potentially using a time budget.
    fn split(&mut self, input: &InP, budget: Option<Ticks>, outputs: &mut [OutP; N]);
}

/// A join operator aggregates M input payloads into one output payload.
pub trait JoinOperator<InP: Payload, OutP: Payload, const M: usize> {
    /// Perform the join/aggregation operation with an optional time budget.
    fn join(&mut self, inputs: &[InP; M], budget: Option<Ticks>) -> OutP;
}
