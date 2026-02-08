//! Limen single-producer single-consumer edge trait and related types.

use crate::errors::QueueError;
use crate::message::{payload::Payload, Message};
use crate::policy::{AdmissionDecision, EdgePolicy, WatermarkState};

pub mod bench;
pub mod link;

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
#[non_exhaustive]
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
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeOccupancy {
    /// Number of items currently in the queue.
    items: usize,
    /// Estimated bytes currently in the queue.
    bytes: usize,
    /// Watermark state derived from capacities.
    watermark: WatermarkState,
}

impl EdgeOccupancy {
    /// Create a new `EdgeOccupancy`.
    #[inline]
    pub const fn new(items: usize, bytes: usize, watermark: WatermarkState) -> Self {
        Self {
            items,
            bytes,
            watermark,
        }
    }

    /// Number of items currently in the queue.
    #[inline]
    pub fn items(&self) -> &usize {
        &self.items
    }

    /// Estimated bytes currently in the queue.
    #[inline]
    pub fn bytes(&self) -> &usize {
        &self.bytes
    }

    /// Watermark state derived from capacities.
    #[inline]
    pub fn watermark(&self) -> &WatermarkState {
        &self.watermark
    }
}

/// A single-producer, single-consumer queue contract.
///
/// The `Item` type is typically a [`Message<P>`](Message) with some payload `P`,
/// but the trait is generic and can be used for other types as needed.
pub trait Edge {
    /// The type of items stored in the queue.
    type Item;

    /// Attempt to push an item onto the queue using the given edge policy.
    ///
    /// Implementations may evict an existing item if `DropOldest` is configured.
    fn try_push(&mut self, item: Self::Item, policy: &EdgePolicy) -> EnqueueResult;

    /// Attempt to pop an item from the queue.
    fn try_pop(&mut self) -> Result<Self::Item, QueueError>;

    /// Return a snapshot of occupancy used for telemetry and admission.
    ///
    /// Implementations should avoid blocking. If a concurrent backend might fail
    /// to sample (e.g., poisoned lock), provide a fallible path in the backend and
    /// map that to `GraphError::OccupancySampleFailed` in your `GraphApi` impl.
    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy;

    /// Return `true` if the queue is empty.
    fn is_empty(&self) -> bool {
        matches!(self.try_peek(), Err(QueueError::Empty))
    }

    /// Peek at the front item without removing it. Default implementation
    /// uses `try_pop` and re-insert is left to concrete impls; many queues
    /// will override this to be non-destructive.
    fn try_peek(&self) -> Result<&Self::Item, QueueError>;

    /// std-only helper: copy the front item without removing it (cheap for `Copy`).
    ///
    /// Default implementation calls `try_peek()` and clones the result.
    /// Concurrent implementations that cannot return `&Item` across lock
    /// guards should override this to avoid returning `Unsupported`.
    #[cfg(feature = "std")]
    fn try_peek_copied(&self) -> Result<Self::Item, QueueError>
    where
        Self::Item: Copy,
    {
        self.try_peek().copied()
    }

    /// std-only helper: clone the front item without removing it (heavier than copy).
    ///
    /// Default implementation calls `try_peek()` and clones the result.
    /// Concurrent implementations that cannot return `&Item` across lock
    /// guards should override this to avoid returning `Unsupported`.
    #[cfg(feature = "std")]
    fn try_peek_cloned(&self) -> Result<Self::Item, QueueError>
    where
        Self::Item: Clone,
    {
        self.try_peek().cloned()
    }
}

/// Convenience helper to enqueue a message using policy-derived admission logic.
pub fn enqueue_with_admission<P: Payload, Q: Edge<Item = Message<P>>>(
    queue: &mut Q,
    policy: &EdgePolicy,
    msg: Message<P>,
) -> EnqueueResult {
    let occ = queue.occupancy(policy);
    match policy.decide(
        occ.items,
        occ.bytes,
        *msg.header().deadline_ns(),
        *msg.header().qos(),
    ) {
        AdmissionDecision::Admit => queue.try_push(msg, policy),
        AdmissionDecision::Reject => EnqueueResult::Rejected,
    }
}

/// A no-op queue implementation used for phantom inputs and outputs.
///
/// `NoQueue` acts as a placeholder in the graph where a queue is required by
/// type but no actual buffering or message transfer is desired. All enqueue
/// attempts are rejected, and all dequeue or peek attempts return empty.
///
/// This is primarily useful for:
/// - Phantom or unconnected ports in a graph.
/// - Simplifying generic code that expects a queue type, without allocating
///   unnecessary resources.
/// - Static analysis or testing scenarios where message flow is disabled.
///
/// # Type Parameters
/// - `P`: Payload type of the [`Message`] carried by this queue.
///
/// # Behavior
/// - [`SpscQueue::try_push`] always returns [`EnqueueResult::Rejected`].
/// - [`SpscQueue::try_pop`] always returns [`QueueError::Empty`].
/// - [`SpscQueue::try_peek`] always returns [`QueueError::Empty`].
/// - [`SpscQueue::occupancy`] always reports zero items, zero bytes, and
///   [`WatermarkState::AtOrAboveHard`] (fully saturated, disallowing admission).
pub struct NoQueue<P: Payload>(core::marker::PhantomData<P>);

impl<P: Payload> Edge for NoQueue<P> {
    type Item = Message<P>;
    #[inline]
    fn try_push(&mut self, _item: Self::Item, _policy: &EdgePolicy) -> EnqueueResult {
        EnqueueResult::Rejected
    }
    #[inline]
    fn try_pop(&mut self) -> Result<Self::Item, QueueError> {
        Err(QueueError::Empty)
    }
    #[inline]
    fn occupancy(&self, _policy: &EdgePolicy) -> EdgeOccupancy {
        EdgeOccupancy {
            items: 0,
            bytes: 0,
            watermark: WatermarkState::AtOrAboveHard,
        }
    }
    #[inline]
    fn try_peek(&self) -> Result<&Self::Item, QueueError> {
        Err(QueueError::Empty)
    }
}
