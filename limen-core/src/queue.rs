//! Single-producer single-consumer queue trait and related types.
//!
//! Concrete implementations live in `limen-light` (P0/P1) and `limen` (P2).
//! The core defines capacities, watermarking, admission, and generic results.

use crate::errors::QueueError;
use crate::message::{payload::Payload, Message};
use crate::policy::{AdmissionDecision, EdgePolicy, WatermarkState};

pub mod descriptor;

pub mod spsc_array;

#[cfg(feature = "alloc")]
pub mod spsc_vecdeque;

#[cfg(feature = "std")]
pub mod spsc_ringbuf;

#[cfg(feature = "std")]
pub mod spsc_concurrent;

#[cfg(feature = "spsc_raw")]
pub mod spsc_raw;

pub mod spsc_priority2;

/// Push result for enqueue attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueResult {
    /// Item was enqueued successfully.
    Enqueued,
    /// Item was dropped per policy (DropNewest).
    DroppedNewest,
    /// Item could not be enqueued due to backpressure or full capacity.
    Rejected,
}

/// Queue occupancy snapshot used for decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueOccupancy {
    /// Number of items currently in the queue.
    pub items: usize,
    /// Estimated bytes currently in the queue.
    pub bytes: usize,
    /// Watermark state derived from capacities.
    pub watermark: WatermarkState,
}

/// A single-producer, single-consumer queue contract.
///
/// The `Item` type is typically a [`Message<P>`](Message) with some payload `P`,
/// but the trait is generic and can be used for other types as needed.
pub trait SpscQueue {
    /// The type of items stored in the queue.
    type Item: Clone;

    /// Attempt to push an item onto the queue using the given edge policy.
    ///
    /// Implementations may evict an existing item if `DropOldest` is configured.
    fn try_push(&mut self, item: Self::Item, policy: &EdgePolicy) -> EnqueueResult;

    /// Attempt to pop an item from the queue.
    fn try_pop(&mut self) -> Result<Self::Item, QueueError>;

    /// Return a snapshot of occupancy used for telemetry and admission.
    fn occupancy(&self, policy: &EdgePolicy) -> QueueOccupancy;

    /// Return `true` if the queue is empty.
    fn is_empty(&self) -> bool {
        matches!(self.try_peek(), Err(QueueError::Empty))
    }

    /// Peek at the front item without removing it. Default implementation
    /// uses `try_pop` and re-insert is left to concrete impls; many queues
    /// will override this to be non-destructive.
    fn try_peek(&self) -> Result<&Self::Item, QueueError>;

    /// std-only helper: clone the front item without removing it.
    ///
    /// Default implementation calls `try_peek()` and clones the result.
    /// Concurrent implementations that cannot return `&Item` across lock
    /// guards should override this to avoid returning `Unsupported`.
    #[cfg(feature = "std")]
    fn try_peek_cloned(&self) -> Result<Self::Item, QueueError> {
        self.try_peek().cloned()
    }
}

/// Convenience helper to enqueue a message using policy-derived admission logic.
pub fn enqueue_with_admission<P: Payload, Q: SpscQueue<Item = Message<P>>>(
    queue: &mut Q,
    policy: &EdgePolicy,
    msg: Message<P>,
) -> EnqueueResult {
    let occ = queue.occupancy(policy);
    match policy.decide(occ.items, occ.bytes, msg.header.deadline_ns, msg.header.qos) {
        AdmissionDecision::Admit => queue.try_push(msg, policy),
        AdmissionDecision::Reject => EnqueueResult::Rejected,
    }
}
