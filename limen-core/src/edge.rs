//! Limen single-producer single-consumer edge trait and related types.

use crate::errors::QueueError;
use crate::message::Message;
use crate::policy::{AdmissionDecision, BatchingPolicy, EdgePolicy, WatermarkState};
use crate::prelude::{BatchView, HeaderStore, Payload};
use crate::types::MessageToken;

pub mod link;

pub mod spsc_array;

#[cfg(feature = "alloc")]
pub mod spsc_vecdeque;

#[cfg(feature = "std")]
pub mod spsc_concurrent;

#[cfg(feature = "spsc_raw")]
pub mod spsc_raw;

pub mod spsc_priority2;

#[cfg(any(test, feature = "bench"))]
pub mod bench;

#[cfg(any(test, feature = "bench"))]
pub mod contract_tests;
#[cfg(any(test, feature = "bench"))]
pub use contract_tests::*;

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
/// Edges store [`MessageToken`] handles. Actual message data (header + payload)
/// resides in a [`MemoryManager`](crate::memory::manager::MemoryManager).
/// Edge methods that need header metadata (admission, batching, peek) receive
/// a `&impl HeaderStore` parameter — statically dispatched, no `dyn`.
pub trait Edge {
    /// Attempt to push a token onto the queue using the given edge policy.
    ///
    /// The implementation uses `headers` to look up the token's message header
    /// for admission decisions (byte size, QoS, deadline).
    ///
    /// For `DropOldest` policies, the caller must pre-evict via
    /// `get_admission_decision` + `try_pop` before calling `try_push`.
    /// `try_push` itself never evicts.
    fn try_push<H: HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult;

    /// Attempt to pop the front token from the queue.
    ///
    /// Uses `headers` to look up the popped token's byte size for internal
    /// byte tracking.
    fn try_pop<H: HeaderStore>(&mut self, headers: &H) -> Result<MessageToken, QueueError>;

    /// Return a snapshot of occupancy used for telemetry and admission.
    ///
    /// Uses internal counters (items + total_bytes) — no HeaderStore needed.
    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy;

    /// Return `true` if the queue is empty.
    fn is_empty(&self) -> bool;

    /// Peek at the front token without removing it.
    fn try_peek(&self) -> Result<MessageToken, QueueError>;

    /// Peek at the token at logical position `index` from the front.
    ///
    /// - `index = 0` is equivalent to `try_peek`.
    /// - Returns `QueueError::Empty` if `index` is out of range.
    fn try_peek_at(&self, index: usize) -> Result<MessageToken, QueueError>;

    /// Peek the front message header via `HeaderStore` (convenience).
    ///
    /// Returns the `HeaderGuard` associated to `H`, which dereferences to
    /// `MessageHeader`. This allows both single-threaded managers (which can
    /// return `&MessageHeader`) and concurrent managers (which return a
    /// guard holding a slot-level lock).
    ///
    /// The returned guard keeps the underlying header valid for the lifetime
    /// of the guard.
    fn peek_header<'h, H: HeaderStore>(
        &self,
        headers: &'h H,
    ) -> Result<<H as HeaderStore>::HeaderGuard<'h>, QueueError> {
        let token = self.try_peek()?;
        headers.peek_header(token).map_err(|_| QueueError::Empty)
    }

    /// Pop a batch of tokens according to the provided batching policy.
    ///
    /// Uses `headers` for delta-t readiness checks (peeks `creation_tick`
    /// on tokens in the queue via HeaderStore).
    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        policy: &BatchingPolicy,
        headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError>;

    /// Return an `AdmissionDecision` for the given token according to
    /// `policy` and the current occupancy snapshot.
    ///
    /// Pure: does not mutate the queue.
    fn get_admission_decision<H: HeaderStore>(
        &self,
        policy: &EdgePolicy,
        token: MessageToken,
        headers: &H,
    ) -> AdmissionDecision {
        let occ = self.occupancy(policy);
        match headers.peek_header(token) {
            Ok(h) => policy.decide(
                occ.items,
                occ.bytes,
                *h.payload_size_bytes(),
                *h.deadline_ns(),
                *h.qos(),
            ),
            Err(_) => AdmissionDecision::Reject,
        }
    }

    /// Return an `AdmissionDecision` for the given token according to
    /// `policy` and the current occupancy snapshot.
    ///
    /// Pure: does not mutate the queue.
    fn get_admission_decision_from_message<P: Payload>(
        &self,
        policy: &EdgePolicy,
        message: &Message<P>,
    ) -> AdmissionDecision {
        let occ = self.occupancy(policy);
        let h = message.header();
        policy.decide(
            occ.items,
            occ.bytes,
            *h.payload_size_bytes(),
            *h.deadline_ns(),
            *h.qos(),
        )
    }
}

/// Which role a scoped edge handle serves.
///
/// Arc-based edges (e.g. `ConcurrentEdge`) ignore this — the clone is
/// full-duplex. Future lock-free split-handle edges (e.g. `SpscAtomicRing`)
/// will return a producer-only or consumer-only handle depending on `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeHandleKind {
    /// Handle will be used to push messages (output side of a node).
    Producer,
    /// Handle will be used to pop messages (input side of a node).
    Consumer,
}

/// Scoped handle factory for edges used in concurrent execution.
///
/// The GAT `Handle<'a>` allows implementations to return either:
/// - An owned clone (Arc-based: `ConcurrentEdge`)
/// - A borrowed split handle (future lock-free: producer or consumer view)
///
/// The lifetime `'a` is tied to `std::thread::scope` — all handles are
/// guaranteed to be dropped before the scope exits.
#[cfg(feature = "std")]
pub trait ScopedEdge: Edge {
    /// Per-worker handle type. Must implement `Edge + Send` so it can be
    /// moved into a scoped thread and used for stepping.
    type Handle<'a>: Edge + Send + 'a
    where
        Self: 'a;

    /// Create a scoped handle for a worker thread.
    ///
    /// `kind` indicates whether the worker will use this handle as a
    /// producer (push) or consumer (pop). Arc-based implementations may
    /// ignore `kind`. Split-handle implementations use it to select the
    /// correct half.
    fn scoped_handle<'a>(&'a self, kind: EdgeHandleKind) -> Self::Handle<'a>
    where
        Self: 'a;
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
pub struct NoQueue;

impl Edge for NoQueue {
    #[inline]
    fn try_push<H: HeaderStore>(
        &mut self,
        _token: MessageToken,
        _policy: &EdgePolicy,
        _headers: &H,
    ) -> EnqueueResult {
        EnqueueResult::Rejected
    }

    #[inline]
    fn try_pop<H: HeaderStore>(&mut self, _headers: &H) -> Result<MessageToken, QueueError> {
        Err(QueueError::Empty)
    }

    #[inline]
    fn occupancy(&self, _policy: &EdgePolicy) -> EdgeOccupancy {
        EdgeOccupancy::new(0, 0, WatermarkState::AtOrAboveHard)
    }

    fn is_empty(&self) -> bool {
        true
    }

    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        Err(QueueError::Empty)
    }

    fn try_peek_at(&self, _index: usize) -> Result<MessageToken, QueueError> {
        Err(QueueError::Empty)
    }

    #[inline]
    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        _policy: &BatchingPolicy,
        _headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        Err(QueueError::Empty)
    }
}
