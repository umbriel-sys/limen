//! Limen single-producer single-consumer edge trait and related types.

use crate::errors::QueueError;
use crate::message::AdmissionInfo;
use crate::message::{payload::Payload, Message};
use crate::policy::{AdmissionDecision, BatchingPolicy, EdgePolicy, WatermarkState};
use crate::prelude::BatchView;

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

#[cfg(any(test, feature = "bench"))]
pub mod bench;

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
    fn is_empty(&self) -> bool
    where
        Self::Item: Payload,
    {
        matches!(self.try_peek(), Err(QueueError::Empty))
    }

    /// Peek at the front item without removing it.
    ///
    /// Returns a `MessagePeek<'_, Self::Item>`. Implementations should prefer
    /// returning `MessagePeek::Borrowed(&Self::Item)` for zero-copy paths.
    fn try_peek(&self) -> Result<PeekResponse<'_, Self::Item>, QueueError>
    where
        Self::Item: Payload;

    /// Attempt to pop a batch of items according to the provided batching policy.
    ///
    /// The returned `BatchView<'_, Self::Item>` is allowed to borrow from `self`
    /// (for zero-copy / heapless implementations) or to be an owned collection
    /// (when allocation is available). The lifetime of the `BatchView` is tied
    /// to `&mut self`, so callers must not outlive the borrow.
    ///
    /// Implementations MUST honour the semantics of `BatchingPolicy` (fixed-N,
    /// max-Δt partial batches, and windowing style). Error handling mirrors
    /// `try_pop` and should return a `QueueError` on failure (including empty).
    fn try_pop_batch(
        &mut self,
        policy: &BatchingPolicy,
    ) -> Result<BatchView<'_, Self::Item>, QueueError>
    where
        Self::Item: Payload;

    /// Return an `AdmissionDecision` for the provided item or batch according
    /// to `policy` and the current occupancy snapshot.
    ///
    /// This method is *pure*: it does not mutate the queue. Queue implementors
    /// should call this, then implement the side-effecting behavior required
    /// by the returned `AdmissionDecision` (evictions, push, reject, block).
    ///
    /// Works for both single `Message<P>` and `BatchView<'_, Message<P>>` because
    /// both implement `AdmissionInfo`.
    fn get_admission_decision<I>(&self, policy: &EdgePolicy, item: &I) -> AdmissionDecision
    where
        I: AdmissionInfo,
    {
        let occ = self.occupancy(policy);
        policy.decide(
            occ.items,
            occ.bytes,
            item.item_bytes(),
            item.deadline(),
            item.qos(),
        )
    }
}

/// Unified single-item peek result returned by `Edge::try_peek`.
///
/// Generic over the *item* type `I` stored in the edge. Two variants only:
/// - `Borrowed(&'a I)` for zero-copy/no-alloc SPSC paths.
/// - `Owned(I)` for alloc-enabled fallbacks / concurrent queues.
///
/// Callers should use `as_ref()` to get a `&I` regardless of variant.
/// When `I = Message<P>` additional conveniences are provided (header/payload
/// access and `into_owned()`).
#[derive(Debug)]
pub enum PeekResponse<'a, I: 'a> {
    /// Borrowed, zero-alloc view into the queue (SPSC / no-alloc).
    Borrowed(&'a I),

    /// Owned item returned by the queue (alloc required).
    #[cfg(feature = "alloc")]
    Owned(I),
}

impl<'a, I: 'a> AsRef<I> for PeekResponse<'a, I> {
    #[inline]
    fn as_ref(&self) -> &I {
        match self {
            PeekResponse::Borrowed(r) => r,
            #[cfg(feature = "alloc")]
            PeekResponse::Owned(o) => o,
        }
    }
}

/// Convenience methods for the common case where the item is a `Message<P>`.
impl<'a, P: crate::message::payload::Payload + 'a> PeekResponse<'a, crate::message::Message<P>> {
    /// Convenience: return the header reference.
    #[inline]
    pub fn header(&self) -> &crate::message::MessageHeader {
        self.as_ref().header()
    }

    /// Convenience: return the payload reference.
    #[inline]
    pub fn payload(&self) -> &P {
        self.as_ref().payload()
    }

    /// Convert into an owned `Message<P>`.
    ///
    /// - Available when `P: Clone`. This method is **not** gated on `alloc`.
    /// - If this enum is `Owned` (alloc), the owned value is returned directly.
    /// - If `Borrowed`, the message is cloned into an owned `Message<P>`.
    #[inline]
    pub fn into_owned(self) -> crate::message::Message<P>
    where
        P: Clone,
    {
        match self {
            PeekResponse::Borrowed(b) => (*b).clone(),
            #[cfg(feature = "alloc")]
            PeekResponse::Owned(o) => o,
        }
    }
}

/// Generic `Clone` impl: clones the owned item when present, otherwise copies the borrow.
///
/// This requires `I: Clone` so that the `Owned(I)` variant can be cloned.
/// Borrowed(&I) is cheap to clone (copies the reference).
impl<'a, I: Clone + 'a> Clone for PeekResponse<'a, I> {
    fn clone(&self) -> Self {
        match self {
            PeekResponse::Borrowed(r) => PeekResponse::Borrowed(r),
            #[cfg(feature = "alloc")]
            PeekResponse::Owned(o) => PeekResponse::Owned(o.clone()),
        }
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
    fn try_peek(&self) -> Result<PeekResponse<'_, Self::Item>, QueueError>
    where
        Self::Item: Payload,
    {
        Err(QueueError::Empty)
    }

    #[inline]
    fn try_pop_batch(
        &mut self,
        _policy: &BatchingPolicy,
    ) -> Result<BatchView<'_, Self::Item>, QueueError>
    where
        Self::Item: Payload,
    {
        Err(QueueError::Empty)
    }
}

#[cfg(any(test, feature = "bench"))]
pub mod contract_tests {
    //! `Edge` contract tests.
    //!
    //! This module defines the **contract test suite** that every `Edge`
    //! implementation is expected to run. Passing these tests indicates the
    //! queue behaves as required by the runtime (single-item semantics, batching
    //! semantics, and admission-policy behavior under watermarks).
    //!
    //! What is validated
    //! -----------------
    //! The fixtures exercise:
    //! - **Single-item operations**: `try_push`, `try_peek`, `try_pop`, `is_empty`,
    //!   and `occupancy` sanity.
    //! - **FIFO ordering**: items must be observed in the same order they were
    //!   enqueued (unless admission causes eviction).
    //! - **Batching semantics** via `try_pop_batch`:
    //!   - fixed-N batches (`fixed_n`)
    //!   - Δt-limited batches (`max_delta_t`, relative to the front item)
    //!   - combined fixed-N + Δt (stop when either limit is reached)
    //!   - sliding windows (present `size` items, but only pop/advance `stride`)
    //!   - default policy behavior (`fixed_n = 1` when both caps are absent)
    //! - **Admission policies** under pressure (BetweenSoftAndHard):
    //!   - `DropNewest` → returns `DroppedNewest` and preserves existing items
    //!   - `DropOldest` → evicts oldest (when possible) and enqueues newest
    //!   - `Block` → treated as `Rejected` in core (no blocking in the queue contract)
    //!   - `DeadlineAndQoSAware` → admitted between soft/hard per current core policy
    //!
    //! How to use
    //! ----------
    //! Consumers provide a constructor closure that produces a **fresh queue**
    //! instance (empty) per test:
    //!
    //! ```ignore
    //! use crate::edge::contract_tests;
    //!
    //! contract_tests::run_edge_contract_tests!(static_ring_contract, {
    //!     StaticRing::<Message<u32>, 16>::new()
    //! });
    //! ```
    //!
    //! The `run_edge_contract_tests!` macro expands to a submodule containing one
    //! `#[test]` per fixture, which makes failures easy to localize.
    //!
    //! Notes
    //! -----
    //! - Fixtures assume the queue starts empty. Always construct a new queue for
    //!   each test/fixture (the macro does this by calling the constructor each time).
    //! - These tests are intentionally implementation-agnostic: they work for
    //!   heapless (borrowed batch views) and alloc-backed (owned batch views) queues.
    //! - If an implementation wraps internal mutability (e.g., mutex), it must ensure
    //!   returned views do not borrow from a temporary guard.

    use super::*;
    use crate::message::{Message, MessageHeader};
    use crate::policy::{AdmissionPolicy, BatchingPolicy, EdgePolicy, OverBudgetAction, QueueCaps};
    use crate::types::{DeadlineNs, Ticks};

    const TEST_EDGE_POLICY: EdgePolicy = EdgePolicy::new(
        QueueCaps::new(8, 6, None, None),
        AdmissionPolicy::DropNewest,
        OverBudgetAction::Drop,
    );

    /// Build a simple test message with a creation tick and default header fields.
    fn make_msg_u32(tick: u64) -> Message<u32> {
        let mut h = MessageHeader::empty();
        h.set_creation_tick(Ticks::new(tick));
        Message::new(h, 0u32)
    }

    /// Define a set of contract tests for an `Edge` implementer.
    ///
    /// Usage:
    ///
    /// ```rust
    /// contract_tests::define_edge_contract_tests!(static_ring_tests, || {
    ///     crate::spsc_array::StaticRing::<crate::message::Message<u32>, 16>::new()
    /// });
    /// ```
    ///
    /// The macro expands to a submodule named by the first identifier and emits
    /// several `#[test]` functions that run each fixture separately (so CI shows
    /// which part failed).
    #[macro_export]
    macro_rules! run_edge_contract_tests {
        // Accept: test module name, constructor expression (as a closure-like expr).
        ($mod_name:ident, $make:expr) => {
            // Emit a module to contain the tests (so names don't clash).
            #[cfg(test)]
            mod $mod_name {
                use super::*;

                use $crate::edge::contract_tests as fixtures;

                // #[test]
                // fn all_contracts() {
                //     fixtures::run_all_tests(|| $make());
                // }

                #[test]
                fn basic_push_pop() {
                    fixtures::run_basic_push_pop(|| $make());
                }

                #[test]
                fn fifo_order() {
                    fixtures::run_fifo_order(|| $make());
                }

                #[test]
                fn occupancy_and_empty() {
                    fixtures::run_occupancy_and_empty(|| $make());
                }

                #[test]
                fn batch_fixed_n() {
                    fixtures::run_batch_fixed_n(|| $make());
                }

                #[test]
                fn batch_delta_t() {
                    fixtures::run_batch_delta_t(|| $make());
                }

                #[test]
                fn batch_fixed_and_delta() {
                    fixtures::run_batch_fixed_and_delta(|| $make());
                }

                #[test]
                fn batch_sliding() {
                    fixtures::run_batch_sliding(|| $make());
                }

                #[test]
                fn batch_default_one() {
                    fixtures::run_batch_default_one(|| $make());
                }

                #[test]
                fn admission_policies() {
                    fixtures::run_admission_policies(|| $make());
                }

                #[test]
                fn batch_get_admission_drop_newest_between_soft_and_hard() {
                    fixtures::batch_get_admission_drop_newest_between_soft_and_hard(|| $make());
                }

                #[test]
                fn batch_get_admission_evict_until_below_hard() {
                    fixtures::batch_get_admission_evict_until_below_hard(|| $make());
                }

                #[test]
                fn batch_item_bytes_and_deadline_semantics() {
                    fixtures::batch_item_bytes_and_deadline_semantics(|| $make());
                }
            }
        };
    }

    /// Convenience: run all contract tests for a queue produced by `make`.
    ///
    /// `make` must produce a fresh queue instance each time it is called.
    pub fn run_all_tests<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        run_basic_push_pop(&mut make);
        run_fifo_order(&mut make);
        run_occupancy_and_empty(&mut make);
        run_batch_fixed_n(&mut make);
        run_batch_delta_t(&mut make);
        run_batch_fixed_and_delta(&mut make);
        run_batch_sliding(&mut make);
        run_batch_default_one(&mut make);
        run_admission_policies(&mut make);
        batch_get_admission_drop_newest_between_soft_and_hard(&mut make);
        batch_get_admission_evict_until_below_hard(&mut make);
        batch_item_bytes_and_deadline_semantics(&mut make)
    }

    /// Basic push / peek / pop / is_empty invariants.
    pub fn run_basic_push_pop<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let mut q = make();
        let policy = TEST_EDGE_POLICY;

        // empty behaviour
        assert!(matches!(q.try_pop(), Err(QueueError::Empty)));
        assert!(matches!(q.try_peek(), Err(QueueError::Empty)));
        assert!(q.is_empty());

        // push
        let m = make_msg_u32(1);
        assert_eq!(q.try_push(m, &policy), EnqueueResult::Enqueued);

        // peek sees same front
        let peek = q.try_peek().expect("peek after push");
        assert_eq!(
            *peek.as_ref().header().creation_tick(),
            *m.header().creation_tick()
        );

        // not empty
        assert!(!q.is_empty());

        // pop returns same
        let got = q.try_pop().expect("pop after push");
        assert_eq!(*got.header().creation_tick(), *m.header().creation_tick());

        // back to empty
        assert!(matches!(q.try_pop(), Err(QueueError::Empty)));
        assert!(q.is_empty());
    }

    /// FIFO ordering with multiple items.
    pub fn run_fifo_order<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let mut q = make();
        let policy = TEST_EDGE_POLICY;

        for t in 1u64..6u64 {
            let m = make_msg_u32(t);
            assert_eq!(q.try_push(m, &policy), EnqueueResult::Enqueued);
        }

        // pop in order
        for expected in 1u64..6u64 {
            let m = q.try_pop().expect("pop");
            assert_eq!((*m.header().creation_tick()).as_u64(), &expected);
        }
        assert!(matches!(q.try_pop(), Err(QueueError::Empty)));
    }

    /// Occupancy snapshot sanity and is_empty.
    pub fn run_occupancy_and_empty<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let mut q = make();
        let policy = TEST_EDGE_POLICY;

        let occ0 = q.occupancy(&policy);
        assert_eq!(*occ0.items(), 0usize);
        // bytes may be zero, and watermark is valid enum — don't assert exact watermark.

        let m = make_msg_u32(1);
        assert_eq!(q.try_push(m, &policy), EnqueueResult::Enqueued);

        let occ1 = q.occupancy(&policy);
        assert_eq!(*occ1.items(), 1usize);

        // drain
        let _ = q.try_pop().expect("pop");
        let occ2 = q.occupancy(&policy);
        assert_eq!(*occ2.items(), 0usize);
    }

    /// Disjoint windows: fixed-N semantics (pop exactly N when available).
    pub fn run_batch_fixed_n<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let mut q = make();
        let policy = TEST_EDGE_POLICY;

        // push 5 items: 1..5
        for t in 1u64..=5u64 {
            let m = make_msg_u32(t);
            assert_eq!(q.try_push(m, &policy), EnqueueResult::Enqueued);
        }

        let batch_policy = BatchingPolicy::fixed(3);
        let batch = q.try_pop_batch(&batch_policy).expect("batch");
        let batch_ref = batch.as_batch();
        assert_eq!(batch_ref.len(), 3);
        let mut iter = batch_ref.iter();
        let a = iter.next().expect("batch[0]");
        let b = iter.next().expect("batch[1]");
        let c = iter.next().expect("batch[2]");
        assert_eq!((*a.header().creation_tick()).as_u64(), &1u64);
        assert_eq!((*b.header().creation_tick()).as_u64(), &2u64);
        assert_eq!((*c.header().creation_tick()).as_u64(), &3u64);
        assert!(iter.next().is_none());

        // remaining 2 should still be present
        let a = q.try_pop().expect("rem1");
        let b = q.try_pop().expect("rem2");
        assert_eq!((*a.header().creation_tick()).as_u64(), &4u64);
        assert_eq!((*b.header().creation_tick()).as_u64(), &5u64);
        assert!(matches!(q.try_pop(), Err(QueueError::Empty)));
    }

    /// Disjoint windows: delta-t semantics.
    pub fn run_batch_delta_t<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let mut q = make();
        let policy = TEST_EDGE_POLICY;

        // ticks 10,11,12,30
        for t in [10u64, 11u64, 12u64, 30u64].iter() {
            let m = make_msg_u32(*t);
            assert_eq!(q.try_push(m, &policy), EnqueueResult::Enqueued);
        }

        let batch_policy = BatchingPolicy::delta_t(Ticks::new(2u64));
        let batch = q.try_pop_batch(&batch_policy).expect("batch");
        let batch_ref = batch.as_batch();
        assert_eq!(batch_ref.len(), 3);
        let mut iter = batch_ref.iter();
        let a = iter.next().expect("batch[0]");
        let b = iter.next().expect("batch[1]");
        let c = iter.next().expect("batch[2]");
        assert_eq!((*a.header().creation_tick()).as_u64(), &10u64);
        assert_eq!((*b.header().creation_tick()).as_u64(), &11u64);
        assert_eq!((*c.header().creation_tick()).as_u64(), &12u64);
        assert!(iter.next().is_none());

        // remaining is 30
        let last = q.try_pop().expect("remaining");
        assert_eq!((*last.header().creation_tick()).as_u64(), &30u64);
    }

    /// Combined fixed-N and delta-t.
    pub fn run_batch_fixed_and_delta<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let mut q = make();
        let policy = TEST_EDGE_POLICY;

        // ticks: 100,101,102,110
        for t in [100u64, 101u64, 102u64, 110u64].iter() {
            let m = make_msg_u32(*t);
            assert_eq!(q.try_push(m, &policy), EnqueueResult::Enqueued);
        }

        // fixed 2, delta cap 3: should return only first 2 despite delta permitting more
        let batch_policy = BatchingPolicy::fixed_and_delta_t(2, Ticks::new(5u64));
        let batch = q.try_pop_batch(&batch_policy).expect("batch");
        let batch_ref = batch.as_batch();
        assert_eq!(batch_ref.len(), 2);
        let mut iter = batch_ref.iter();
        let a = iter.next().expect("batch[0]");
        let b = iter.next().expect("batch[1]");
        assert_eq!((*a.header().creation_tick()).as_u64(), &100u64);
        assert_eq!((*b.header().creation_tick()).as_u64(), &101u64);
        assert!(iter.next().is_none());

        // remaining are 102 and 110
        let a = q.try_pop().expect("a");
        assert_eq!((*a.header().creation_tick()).as_u64(), &102u64);
        let b = q.try_pop().expect("b");
        assert_eq!((*b.header().creation_tick()).as_u64(), &110u64);
    }

    /// Sliding windows semantics: present `size` but pop `stride`.
    pub fn run_batch_sliding<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let mut q = make();
        let policy = TEST_EDGE_POLICY;

        // push ticks 1..6
        for t in 1u64..=6u64 {
            let m = make_msg_u32(t);
            assert_eq!(q.try_push(m, &policy), EnqueueResult::Enqueued);
        }

        // sliding window: size=4 stride=2  => should return items [1,2,3,4] but only pop 2 (1,2).
        let sw = crate::policy::WindowKind::Sliding(crate::policy::SlidingWindow::new(4, 2));
        let batch_policy = crate::policy::BatchingPolicy::with_window(Some(4), None, sw);
        let batch = q.try_pop_batch(&batch_policy).expect("batch");
        let batch_ref = batch.as_batch();
        assert_eq!(batch_ref.len(), 4);
        let mut iter = batch_ref.iter();
        let a = iter.next().expect("batch[0]");
        let b = iter.next().expect("batch[1]");
        let c = iter.next().expect("batch[2]");
        let d = iter.next().expect("batch[3]");
        assert_eq!((*a.header().creation_tick()).as_u64(), &1u64);
        assert_eq!((*b.header().creation_tick()).as_u64(), &2u64);
        assert_eq!((*c.header().creation_tick()).as_u64(), &3u64);
        assert_eq!((*d.header().creation_tick()).as_u64(), &4u64);
        assert!(iter.next().is_none());

        // after popping stride=2 elements, queue should still contain items starting from 3:
        // remaining items should be 3,4,5,6 (4 items) but because we popped 2, len should be 4
        // verify next pop returns 3
        let next = q.try_pop().expect("next after sliding");
        assert_eq!((*next.header().creation_tick()).as_u64(), &3u64);
    }

    /// Default behaviour: when neither fixed_n nor delta_t set, we treat as fixed_n=1.
    pub fn run_batch_default_one<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let mut q = make();
        let policy = TEST_EDGE_POLICY;

        for t in [1u64, 2u64, 3u64].iter() {
            let m = make_msg_u32(*t);
            assert_eq!(q.try_push(m, &policy), EnqueueResult::Enqueued);
        }

        let batch_policy = BatchingPolicy::default();
        let batch = q.try_pop_batch(&batch_policy).expect("batch");
        let batch_ref = batch.as_batch();
        assert_eq!(batch_ref.len(), 1);
        assert_eq!(
            (*batch_ref.iter().next().unwrap().header().creation_tick()).as_u64(),
            &1u64
        );

        // remaining pops yield 2 and 3
        let a = q.try_pop().expect("a");
        let b = q.try_pop().expect("b");
        assert_eq!((*a.header().creation_tick()).as_u64(), &2u64);
        assert_eq!((*b.header().creation_tick()).as_u64(), &3u64);
        assert!(matches!(q.try_pop(), Err(QueueError::Empty)));
    }

    /// Run admission policy tests for DropNewest / DropOldest / Block / DeadlineAndQoSAware.
    ///
    /// Uses caps: max_items=3, soft_items=1 so pushing 2nd item puts queue BetweenSoftAndHard.
    pub fn run_admission_policies<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let caps = QueueCaps::new(3, 1, None, None);

        // --- DropNewest: second push should be dropped (queue retains first item).
        {
            let mut q = make();
            let policy = EdgePolicy::new(caps, AdmissionPolicy::DropNewest, OverBudgetAction::Drop);

            // first item OK
            let a = make_msg_u32(1);
            assert_eq!(q.try_push(a, &policy), EnqueueResult::Enqueued);

            // second push enters BetweenSoftAndHard -> DropNewest expected
            let b = make_msg_u32(2);
            let res = q.try_push(b, &policy);
            assert_eq!(res, EnqueueResult::DroppedNewest);

            // queue should still contain only `a`
            let peek = q.try_peek().expect("peek after drop-newest");
            assert_eq!(
                *peek.as_ref().header().creation_tick(),
                *a.header().creation_tick()
            );
            // drain
            let popped = q.try_pop().expect("pop a");
            assert_eq!(
                *popped.header().creation_tick(),
                *a.header().creation_tick()
            );
            assert!(matches!(q.try_pop(), Err(QueueError::Empty)));
        }

        // --- DropOldest: second push should evict oldest and succeed.
        {
            let mut q = make();
            let policy = EdgePolicy::new(caps, AdmissionPolicy::DropOldest, OverBudgetAction::Drop);

            let a = make_msg_u32(1);
            assert_eq!(q.try_push(a, &policy), EnqueueResult::Enqueued);

            let b = make_msg_u32(2);
            let res = q.try_push(b, &policy);
            assert_eq!(res, EnqueueResult::Enqueued);

            // The queue should now contain only `b` (oldest `a` evicted).
            let peek = q.try_peek().expect("peek after drop-oldest");
            assert_eq!(
                *peek.as_ref().header().creation_tick(),
                *b.header().creation_tick()
            );

            // drain and verify
            let popped = q.try_pop().expect("pop b");
            assert_eq!(
                *popped.header().creation_tick(),
                *b.header().creation_tick()
            );
            assert!(matches!(q.try_pop(), Err(QueueError::Empty)));
        }

        // --- Block: core cannot block so we expect Rejected when BetweenSoftAndHard
        {
            let mut q = make();
            let policy = EdgePolicy::new(caps, AdmissionPolicy::Block, OverBudgetAction::Drop);

            let a = make_msg_u32(1);
            assert_eq!(q.try_push(a, &policy), EnqueueResult::Enqueued);

            let b = make_msg_u32(2);
            let res = q.try_push(b, &policy);
            assert_eq!(res, EnqueueResult::Rejected);

            // queue should still contain only `a`
            let popped = q.try_pop().expect("pop after block");
            assert_eq!(
                *popped.header().creation_tick(),
                *a.header().creation_tick()
            );
            assert!(matches!(q.try_pop(), Err(QueueError::Empty)));
        }

        // --- DeadlineAndQoSAware: in core policy.decide this resolves to Admit between soft/hard.
        {
            let mut q = make();
            let policy = EdgePolicy::new(
                caps,
                AdmissionPolicy::DeadlineAndQoSAware,
                OverBudgetAction::Drop,
            );

            let a = make_msg_u32(1);
            assert_eq!(q.try_push(a, &policy), EnqueueResult::Enqueued);

            let b = make_msg_u32(2);
            let res = q.try_push(b, &policy);
            // core's EdgePolicy::decide returns Admit for DeadlineAndQoSAware between soft/hard
            assert_eq!(res, EnqueueResult::Enqueued);

            // both should be present in FIFO order.
            let x = q.try_pop().expect("pop a");
            let y = q.try_pop().expect("pop b");
            assert_eq!(*x.header().creation_tick(), *a.header().creation_tick());
            assert_eq!(*y.header().creation_tick(), *b.header().creation_tick());
            assert!(matches!(q.try_pop(), Err(QueueError::Empty)));
        }
    }

    /// Batch admission: DropNewest between soft and hard should be DropNewest.
    ///
    /// Uses borrowed BatchView (no `alloc`) so test is `no_std` compatible.
    pub fn batch_get_admission_drop_newest_between_soft_and_hard<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let caps = QueueCaps::new(4, 2, None, None);
        let mut q = make();
        let policy_drop_newest =
            EdgePolicy::new(caps, AdmissionPolicy::DropNewest, OverBudgetAction::Drop);

        // push two items so the queue sits BetweenSoftAndHard for our caps.
        // For QueueCaps::new(4,2,...) soft == 2, so having 2 items places the queue
        // between soft and hard as intended by this test.
        let m1 = make_msg_u32(1);
        assert_eq!(q.try_push(m1, &policy_drop_newest), EnqueueResult::Enqueued);
        let m2 = make_msg_u32(2);
        assert_eq!(q.try_push(m2, &policy_drop_newest), EnqueueResult::Enqueued);

        // Build a small borrowed array of two messages.
        let mut arr: [Message<u32>; 2] = core::array::from_fn(|i| {
            let mut h = MessageHeader::empty();
            h.set_creation_tick(Ticks::new((i as u64) + 2));
            Message::new(h, (i as u32) + 2)
        });

        // Borrow first 2 entries as a BatchView (no alloc).
        let batch_view = BatchView::from_borrowed(&mut arr, 2);

        // Ask the queue for an admission decision for the batch.
        let decision = q.get_admission_decision(&policy_drop_newest, &batch_view);
        assert_eq!(decision, AdmissionDecision::DropNewest);
    }

    /// Batch admission: At-or-above-hard + DropOldest -> EvictUntilBelowHard; but
    /// if the batch's single item_bytes alone exceed hard cap -> Reject.
    ///
    /// All batch values are borrowed (no alloc).
    pub fn batch_get_admission_evict_until_below_hard<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        // Add a hard byte cap so we can test Reject when a batch alone exceeds bytes.
        let caps = QueueCaps::new(4, 2, Some(1024), Some(512));

        // Create a queue and fill it so occupancy reports AtOrAboveHard.
        // Use a fill policy that will not evict while we grow the queue,
        // otherwise DropOldest would evict and prevent the queue reaching hard.
        let mut q = make();
        let policy_fill = EdgePolicy::new(
            caps,
            AdmissionPolicy::DeadlineAndQoSAware,
            OverBudgetAction::Drop,
        );

        // push enough single messages to reach the configured max_items/hard state.
        for _ in 0..*caps.max_items() {
            let m = make_msg_u32(10);
            let _ = q.try_push(m, &policy_fill);
        }

        // Now construct the DropOldest policy which we will query against.
        let policy_drop_oldest =
            EdgePolicy::new(caps, AdmissionPolicy::DropOldest, OverBudgetAction::Drop);

        // Small batch (1 message) using borrowed array: should prompt EvictUntilBelowHard.
        let mut small: [Message<u32>; 1] = core::array::from_fn(|_| {
            let mut h = MessageHeader::empty();
            h.set_creation_tick(Ticks::new(20u64));
            Message::new(h, 42u32)
        });
        let batch_small = BatchView::from_borrowed(&mut small, 1);

        let decision_small = q.get_admission_decision(&policy_drop_oldest, &batch_small);
        assert_eq!(decision_small, AdmissionDecision::EvictUntilBelowHard);

        // Now craft a "large" borrowed batch that by itself exceeds the hard byte cap.
        // We simulate this by creating an array with many messages (more than caps.max_items()*4)
        // so its total bytes are large relative to caps.
        const LARGE_N: usize = 64;
        // Construct an array with LARGE_N messages (default headers), then take a large prefix.
        let mut large_arr: [Message<u32>; LARGE_N] = core::array::from_fn(|_| Message::default());
        // Fill headers with consistent ticks.
        for m in &mut large_arr[..LARGE_N] {
            let h = m.header_mut();
            h.set_creation_tick(Ticks::new(30u64));
            // Optionally set a larger payload by manipulating header.payload_size_bytes
            // (we rely on Message::default() being valid and we can adjust header directly).
            h.set_payload_size_bytes(1024); // inflate per-message bytes to ensure batch is huge.
        }

        // Use a borrowed batch of LARGE_N items.
        let batch_large = BatchView::from_borrowed(&mut large_arr, LARGE_N);

        let decision_large = q.get_admission_decision(&policy_drop_oldest, &batch_large);
        // Because the batch's item_bytes alone exceed the hard cap even on an empty queue,
        // EdgePolicy::decide should return Reject.
        assert_eq!(decision_large, AdmissionDecision::Reject);
    }

    /// Batch AdmissionInfo semantics: item_bytes sums headers+payloads and
    /// deadline is taken from the LAST message in the batch.
    ///
    /// Uses borrowed BatchView and no allocation.
    pub fn batch_item_bytes_and_deadline_semantics<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge<Item = Message<u32>>,
    {
        let caps = QueueCaps::new(100, 50, None, None);
        let q = make();

        // Create a borrowed array with two messages; last has later deadline.
        let mut arr: [Message<u32>; 2] = core::array::from_fn(|i| {
            let mut h = MessageHeader::empty();
            h.set_creation_tick(Ticks::new((i as u64) + 1));
            Message::new(h, (i as u32) + 1)
        });

        // Set deadlines explicitly: first smaller, last larger.
        arr[0]
            .header_mut()
            .set_deadline_ns(Some(DeadlineNs::new(1000)));
        arr[1]
            .header_mut()
            .set_deadline_ns(Some(DeadlineNs::new(2000)));

        let batch = BatchView::from_borrowed(&mut arr, 2);

        let policy = EdgePolicy::new(
            caps,
            AdmissionPolicy::DeadlineAndQoSAware,
            OverBudgetAction::Drop,
        );

        // Ensure the batch.total bytes are visible to the policy via get_admission_decision.
        let decision = q.get_admission_decision(&policy, &batch);
        assert_eq!(decision, AdmissionDecision::Admit);

        // Additionally assert that AdmissionInfo returns the batch deadline from the last message.
        let occ = q.occupancy(&policy);
        let decision2 = policy.decide(
            occ.items,
            occ.bytes,
            batch.item_bytes(),
            batch.deadline(),
            batch.qos(),
        );
        assert_eq!(decision2, AdmissionDecision::Admit);

        // And confirm that deadline() indeed matches the last header we set (2000).
        assert_eq!(batch.deadline(), Some(DeadlineNs::new(2000)));
    }
}
