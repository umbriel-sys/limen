//! Limen single-producer single-consumer edge trait and related types.

use crate::errors::QueueError;
use crate::policy::{AdmissionDecision, BatchingPolicy, EdgePolicy, WatermarkState};
use crate::prelude::{BatchView, HeaderStore};
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
    /// An older item was evicted (DropOldest). The evicted token is returned
    /// so the caller can free it from the memory manager.
    Evicted(MessageToken),
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
    /// Returns [`EnqueueResult::Evicted`] if DropOldest evicted an item —
    /// the caller is responsible for freeing the evicted token from the manager.
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
    //!     StaticRing::<16>::new()
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
    //! - Each fixture creates its own `StaticMemoryManager` to store messages and
    //!   act as the `HeaderStore` for edge operations.
    //! - These tests are intentionally implementation-agnostic: they work for
    //!   heapless (borrowed batch views) and alloc-backed (owned batch views) queues.
    //! - If an implementation wraps internal mutability (e.g., mutex), it must ensure
    //!   returned views do not borrow from a temporary guard.

    use super::*;
    use crate::memory::manager::MemoryManager;
    use crate::memory::static_manager::StaticMemoryManager;
    use crate::message::{Message, MessageHeader};
    use crate::policy::{AdmissionPolicy, BatchingPolicy, EdgePolicy, OverBudgetAction, QueueCaps};
    use crate::types::{DeadlineNs, MessageToken, Ticks};

    const TEST_EDGE_POLICY: EdgePolicy = EdgePolicy::new(
        QueueCaps::new(8, 6, None, None),
        AdmissionPolicy::DropNewest,
        OverBudgetAction::Drop,
    );

    /// Memory manager depth for most tests. Must be >= max items stored.
    const MGR_DEPTH: usize = 32;

    /// Build a simple test message with a creation tick and default header fields.
    fn make_msg_u32(tick: u64) -> Message<u32> {
        let mut h = MessageHeader::empty();
        h.set_creation_tick(Ticks::new(tick));
        Message::new(h, 0u32)
    }

    /// Store a message in the manager, returning its token. Panics on failure.
    fn store(mgr: &mut StaticMemoryManager<u32, MGR_DEPTH>, msg: Message<u32>) -> MessageToken {
        mgr.store(msg).expect("memory manager store failed")
    }

    /// Define a set of contract tests for an `Edge` implementer.
    ///
    /// Usage:
    ///
    /// ```rust
    /// limen_core::run_edge_contract_tests!(static_ring_tests, || {
    ///     crate::spsc_array::StaticRing::<16>::new()
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
                fn admission_drop_newest_between_soft_and_hard() {
                    fixtures::run_admission_drop_newest_between_soft_and_hard(|| $make());
                }

                #[test]
                fn admission_evict_until_below_hard() {
                    fixtures::run_admission_evict_until_below_hard(|| $make());
                }

                #[test]
                fn admission_item_bytes_and_deadline_semantics() {
                    fixtures::run_admission_item_bytes_and_deadline_semantics(|| $make());
                }

                #[test]
                fn try_peek_at() {
                    fixtures::run_try_peek_at(|| $make());
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
        Q: Edge,
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
        run_admission_drop_newest_between_soft_and_hard(&mut make);
        run_admission_evict_until_below_hard(&mut make);
        run_admission_item_bytes_and_deadline_semantics(&mut make);
        run_try_peek_at(&mut make);
    }

    /// Basic push / peek / pop / is_empty invariants.
    pub fn run_basic_push_pop<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy = TEST_EDGE_POLICY;

        // empty behaviour
        assert!(matches!(q.try_pop(&mgr), Err(QueueError::Empty)));
        assert!(matches!(q.try_peek(), Err(QueueError::Empty)));
        assert!(q.is_empty());

        // push
        let m = make_msg_u32(1);
        let token = store(&mut mgr, m);
        assert_eq!(q.try_push(token, &policy, &mgr), EnqueueResult::Enqueued);

        // peek sees same front token
        let peek_token = q.try_peek().expect("peek after push");
        assert_eq!(peek_token, token);

        // verify header via manager
        {
            let peek_header = mgr.peek_header(peek_token).expect("peek header");
            assert_eq!(*peek_header.creation_tick(), Ticks::new(1));
        }

        // not empty
        assert!(!q.is_empty());

        // pop returns same token
        let got_token = q.try_pop(&mgr).expect("pop after push");
        assert_eq!(got_token, token);

        // verify popped token's header
        {
            let got_header = mgr.peek_header(got_token).expect("got header");
            assert_eq!(*got_header.creation_tick(), Ticks::new(1));
        }

        // back to empty
        assert!(matches!(q.try_pop(&mgr), Err(QueueError::Empty)));
        assert!(q.is_empty());

        // clean up
        mgr.free(got_token).expect("free");
    }

    /// FIFO ordering with multiple items.
    pub fn run_fifo_order<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy = TEST_EDGE_POLICY;

        let mut tokens = [MessageToken::INVALID; 5];
        for (i, t) in (1u64..6u64).enumerate() {
            let m = make_msg_u32(t);
            tokens[i] = store(&mut mgr, m);
            assert_eq!(
                q.try_push(tokens[i], &policy, &mgr),
                EnqueueResult::Enqueued
            );
        }

        // pop in order
        for (i, expected) in (1u64..6u64).enumerate() {
            let popped = q.try_pop(&mgr).expect("pop");
            assert_eq!(popped, tokens[i]);
            let h = mgr.peek_header(popped).expect("header");
            assert_eq!(*h.creation_tick().as_u64(), expected);
        }
        assert!(matches!(q.try_pop(&mgr), Err(QueueError::Empty)));
    }

    /// Occupancy snapshot sanity and is_empty.
    pub fn run_occupancy_and_empty<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy = TEST_EDGE_POLICY;

        let occ0 = q.occupancy(&policy);
        assert_eq!(*occ0.items(), 0usize);

        let m = make_msg_u32(1);
        let token = store(&mut mgr, m);
        assert_eq!(q.try_push(token, &policy, &mgr), EnqueueResult::Enqueued);

        let occ1 = q.occupancy(&policy);
        assert_eq!(*occ1.items(), 1usize);

        // drain
        let _ = q.try_pop(&mgr).expect("pop");
        let occ2 = q.occupancy(&policy);
        assert_eq!(*occ2.items(), 0usize);
    }

    /// Disjoint windows: fixed-N semantics (pop exactly N when available).
    pub fn run_batch_fixed_n<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy = TEST_EDGE_POLICY;

        // push 5 items: ticks 1..=5
        let mut tokens = [MessageToken::INVALID; 5];
        for (i, t) in (1u64..=5u64).enumerate() {
            let m = make_msg_u32(t);
            tokens[i] = store(&mut mgr, m);
            assert_eq!(
                q.try_push(tokens[i], &policy, &mgr),
                EnqueueResult::Enqueued
            );
        }

        let batch_policy = BatchingPolicy::fixed(3);
        let batch = q.try_pop_batch(&batch_policy, &mgr).expect("batch");
        assert_eq!(batch.len(), 3);
        let mut iter = batch.iter();
        let a = iter.next().expect("batch[0]");
        let b = iter.next().expect("batch[1]");
        let c = iter.next().expect("batch[2]");
        assert_eq!(*mgr.peek_header(*a).unwrap().creation_tick(), Ticks::new(1));
        assert_eq!(*mgr.peek_header(*b).unwrap().creation_tick(), Ticks::new(2));
        assert_eq!(*mgr.peek_header(*c).unwrap().creation_tick(), Ticks::new(3));
        assert!(iter.next().is_none());

        // remaining 2 should still be present
        let ra = q.try_pop(&mgr).expect("rem1");
        let rb = q.try_pop(&mgr).expect("rem2");
        assert_eq!(*mgr.peek_header(ra).unwrap().creation_tick(), Ticks::new(4));
        assert_eq!(*mgr.peek_header(rb).unwrap().creation_tick(), Ticks::new(5));
        assert!(matches!(q.try_pop(&mgr), Err(QueueError::Empty)));
    }

    /// Disjoint windows: delta-t semantics.
    pub fn run_batch_delta_t<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy = TEST_EDGE_POLICY;

        // ticks 10,11,12,30
        for t in [10u64, 11u64, 12u64, 30u64].iter() {
            let m = make_msg_u32(*t);
            let token = store(&mut mgr, m);
            assert_eq!(q.try_push(token, &policy, &mgr), EnqueueResult::Enqueued);
        }

        let batch_policy = BatchingPolicy::delta_t(Ticks::new(2u64));
        let batch = q.try_pop_batch(&batch_policy, &mgr).expect("batch");
        assert_eq!(batch.len(), 3);
        let mut iter = batch.iter();
        let a = iter.next().expect("batch[0]");
        let b = iter.next().expect("batch[1]");
        let c = iter.next().expect("batch[2]");
        assert_eq!(
            *mgr.peek_header(*a).unwrap().creation_tick(),
            Ticks::new(10)
        );
        assert_eq!(
            *mgr.peek_header(*b).unwrap().creation_tick(),
            Ticks::new(11)
        );
        assert_eq!(
            *mgr.peek_header(*c).unwrap().creation_tick(),
            Ticks::new(12)
        );
        assert!(iter.next().is_none());

        // remaining is 30
        let last = q.try_pop(&mgr).expect("remaining");
        assert_eq!(
            *mgr.peek_header(last).unwrap().creation_tick(),
            Ticks::new(30)
        );
    }

    /// Combined fixed-N and delta-t.
    pub fn run_batch_fixed_and_delta<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy = TEST_EDGE_POLICY;

        // ticks: 100,101,102,110
        for t in [100u64, 101u64, 102u64, 110u64].iter() {
            let m = make_msg_u32(*t);
            let token = store(&mut mgr, m);
            assert_eq!(q.try_push(token, &policy, &mgr), EnqueueResult::Enqueued);
        }

        // fixed 2, delta cap 5: should return only first 2 despite delta permitting more
        let batch_policy = BatchingPolicy::fixed_and_delta_t(2, Ticks::new(5u64));
        let batch = q.try_pop_batch(&batch_policy, &mgr).expect("batch");
        assert_eq!(batch.len(), 2);
        let mut iter = batch.iter();
        let a = iter.next().expect("batch[0]");
        let b = iter.next().expect("batch[1]");
        assert_eq!(
            *mgr.peek_header(*a).unwrap().creation_tick(),
            Ticks::new(100)
        );
        assert_eq!(
            *mgr.peek_header(*b).unwrap().creation_tick(),
            Ticks::new(101)
        );
        assert!(iter.next().is_none());

        // remaining are 102 and 110
        let ra = q.try_pop(&mgr).expect("a");
        assert_eq!(
            *mgr.peek_header(ra).unwrap().creation_tick(),
            Ticks::new(102)
        );
        let rb = q.try_pop(&mgr).expect("b");
        assert_eq!(
            *mgr.peek_header(rb).unwrap().creation_tick(),
            Ticks::new(110)
        );
    }

    /// Sliding windows semantics: present `size` but pop `stride`.
    pub fn run_batch_sliding<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy = TEST_EDGE_POLICY;

        // push ticks 1..=6
        for t in 1u64..=6u64 {
            let m = make_msg_u32(t);
            let token = store(&mut mgr, m);
            assert_eq!(q.try_push(token, &policy, &mgr), EnqueueResult::Enqueued);
        }

        // sliding window: size=4 stride=2 => should return items [1,2,3,4] but only pop 2 (1,2).
        let sw = crate::policy::WindowKind::Sliding(crate::policy::SlidingWindow::new(2));
        let batch_policy = crate::policy::BatchingPolicy::with_window(Some(4), None, sw);
        let batch = q.try_pop_batch(&batch_policy, &mgr).expect("batch");
        assert_eq!(batch.len(), 4);
        let mut iter = batch.iter();
        let a = iter.next().expect("batch[0]");
        let b = iter.next().expect("batch[1]");
        let c = iter.next().expect("batch[2]");
        let d = iter.next().expect("batch[3]");
        assert_eq!(*mgr.peek_header(*a).unwrap().creation_tick(), Ticks::new(1));
        assert_eq!(*mgr.peek_header(*b).unwrap().creation_tick(), Ticks::new(2));
        assert_eq!(*mgr.peek_header(*c).unwrap().creation_tick(), Ticks::new(3));
        assert_eq!(*mgr.peek_header(*d).unwrap().creation_tick(), Ticks::new(4));
        assert!(iter.next().is_none());

        // after popping stride=2, queue should still contain items starting from 3:
        // verify next pop returns 3
        let next = q.try_pop(&mgr).expect("next after sliding");
        assert_eq!(
            *mgr.peek_header(next).unwrap().creation_tick(),
            Ticks::new(3)
        );
    }

    /// Default behaviour: when neither fixed_n nor delta_t set, we treat as fixed_n=1.
    pub fn run_batch_default_one<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy = TEST_EDGE_POLICY;

        for t in [1u64, 2u64, 3u64].iter() {
            let m = make_msg_u32(*t);
            let token = store(&mut mgr, m);
            assert_eq!(q.try_push(token, &policy, &mgr), EnqueueResult::Enqueued);
        }

        let batch_policy = BatchingPolicy::default();
        let batch = q.try_pop_batch(&batch_policy, &mgr).expect("batch");
        assert_eq!(batch.len(), 1);
        let first_token = batch.iter().next().unwrap();
        assert_eq!(
            *mgr.peek_header(*first_token).unwrap().creation_tick(),
            Ticks::new(1)
        );

        // remaining pops yield 2 and 3
        let a = q.try_pop(&mgr).expect("a");
        let b = q.try_pop(&mgr).expect("b");
        assert_eq!(*mgr.peek_header(a).unwrap().creation_tick(), Ticks::new(2));
        assert_eq!(*mgr.peek_header(b).unwrap().creation_tick(), Ticks::new(3));
        assert!(matches!(q.try_pop(&mgr), Err(QueueError::Empty)));
    }

    /// Run admission policy tests for DropNewest / DropOldest / Block / DeadlineAndQoSAware.
    ///
    /// Uses caps: max_items=3, soft_items=1 so pushing 2nd item puts queue BetweenSoftAndHard.
    pub fn run_admission_policies<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let caps = QueueCaps::new(3, 1, None, None);

        // --- DropNewest: second push should be dropped (queue retains first item).
        {
            let mut q = make();
            let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
            let policy = EdgePolicy::new(caps, AdmissionPolicy::DropNewest, OverBudgetAction::Drop);

            let a_msg = make_msg_u32(1);
            let a_token = store(&mut mgr, a_msg);
            assert_eq!(q.try_push(a_token, &policy, &mgr), EnqueueResult::Enqueued);

            // second push enters BetweenSoftAndHard -> DropNewest expected
            let b_msg = make_msg_u32(2);
            let b_token = store(&mut mgr, b_msg);
            let res = q.try_push(b_token, &policy, &mgr);
            assert_eq!(res, EnqueueResult::DroppedNewest);

            // queue should still contain only `a`
            let peek_token = q.try_peek().expect("peek after drop-newest");
            assert_eq!(peek_token, a_token);

            // drain
            let popped = q.try_pop(&mgr).expect("pop a");
            assert_eq!(popped, a_token);
            assert!(matches!(q.try_pop(&mgr), Err(QueueError::Empty)));
        }

        // --- DropOldest: second push should evict oldest and succeed.
        {
            let mut q = make();
            let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
            let policy = EdgePolicy::new(caps, AdmissionPolicy::DropOldest, OverBudgetAction::Drop);

            let a_msg = make_msg_u32(1);
            let a_token = store(&mut mgr, a_msg);
            assert_eq!(q.try_push(a_token, &policy, &mgr), EnqueueResult::Enqueued);

            let b_msg = make_msg_u32(2);
            let b_token = store(&mut mgr, b_msg);
            let res = q.try_push(b_token, &policy, &mgr);
            // DropOldest evicts `a`, enqueues `b`, returns Evicted(a_token)
            assert_eq!(res, EnqueueResult::Evicted(a_token));

            // The queue should now contain only `b` (oldest `a` evicted).
            let peek_token = q.try_peek().expect("peek after drop-oldest");
            assert_eq!(peek_token, b_token);

            // drain and verify
            let popped = q.try_pop(&mgr).expect("pop b");
            assert_eq!(popped, b_token);
            assert!(matches!(q.try_pop(&mgr), Err(QueueError::Empty)));
        }

        // --- Block: core cannot block so we expect Rejected when BetweenSoftAndHard
        {
            let mut q = make();
            let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
            let policy = EdgePolicy::new(caps, AdmissionPolicy::Block, OverBudgetAction::Drop);

            let a_msg = make_msg_u32(1);
            let a_token = store(&mut mgr, a_msg);
            assert_eq!(q.try_push(a_token, &policy, &mgr), EnqueueResult::Enqueued);

            let b_msg = make_msg_u32(2);
            let b_token = store(&mut mgr, b_msg);
            let res = q.try_push(b_token, &policy, &mgr);
            assert_eq!(res, EnqueueResult::Rejected);

            // queue should still contain only `a`
            let popped = q.try_pop(&mgr).expect("pop after block");
            assert_eq!(popped, a_token);
            assert!(matches!(q.try_pop(&mgr), Err(QueueError::Empty)));
        }

        // --- DeadlineAndQoSAware: in core policy.decide this resolves to Admit between soft/hard.
        {
            let mut q = make();
            let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
            let policy = EdgePolicy::new(
                caps,
                AdmissionPolicy::DeadlineAndQoSAware,
                OverBudgetAction::Drop,
            );

            let a_msg = make_msg_u32(1);
            let a_token = store(&mut mgr, a_msg);
            assert_eq!(q.try_push(a_token, &policy, &mgr), EnqueueResult::Enqueued);

            let b_msg = make_msg_u32(2);
            let b_token = store(&mut mgr, b_msg);
            let res = q.try_push(b_token, &policy, &mgr);
            // core's EdgePolicy::decide returns Admit for DeadlineAndQoSAware between soft/hard
            assert_eq!(res, EnqueueResult::Enqueued);

            // both should be present in FIFO order.
            let x = q.try_pop(&mgr).expect("pop a");
            let y = q.try_pop(&mgr).expect("pop b");
            assert_eq!(x, a_token);
            assert_eq!(y, b_token);
            assert!(matches!(q.try_pop(&mgr), Err(QueueError::Empty)));
        }
    }

    /// Admission: DropNewest between soft and hard should be DroppedNewest.
    ///
    /// Uses the `get_admission_decision` default method to verify the pure
    /// decision path via HeaderStore lookup.
    pub fn run_admission_drop_newest_between_soft_and_hard<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let caps = QueueCaps::new(4, 2, None, None);
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy_drop_newest =
            EdgePolicy::new(caps, AdmissionPolicy::DropNewest, OverBudgetAction::Drop);

        // push two items so the queue sits BetweenSoftAndHard for our caps.
        let m1 = make_msg_u32(1);
        let t1 = store(&mut mgr, m1);
        assert_eq!(
            q.try_push(t1, &policy_drop_newest, &mgr),
            EnqueueResult::Enqueued
        );
        let m2 = make_msg_u32(2);
        let t2 = store(&mut mgr, m2);
        assert_eq!(
            q.try_push(t2, &policy_drop_newest, &mgr),
            EnqueueResult::Enqueued
        );

        // Create a new token to test admission decision against.
        let m3 = make_msg_u32(3);
        let t3 = store(&mut mgr, m3);

        // Ask the queue for an admission decision for the new token.
        let decision = q.get_admission_decision(&policy_drop_newest, t3, &mgr);
        assert_eq!(decision, crate::policy::AdmissionDecision::DropNewest);
    }

    /// Admission: At-or-above-hard + DropOldest -> EvictUntilBelowHard; but
    /// if a single item's bytes alone exceed hard cap -> Reject.
    pub fn run_admission_evict_until_below_hard<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        // Add a hard byte cap so we can test Reject when item alone exceeds bytes.
        let caps = QueueCaps::new(4, 2, Some(1024), Some(512));

        // Create a queue and fill it so occupancy reports AtOrAboveHard.
        // Use a fill policy that will not evict while we grow the queue.
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy_fill = EdgePolicy::new(
            caps,
            AdmissionPolicy::DeadlineAndQoSAware,
            OverBudgetAction::Drop,
        );

        // push enough single messages to reach the configured max_items/hard state.
        for _ in 0..*caps.max_items() {
            let m = make_msg_u32(10);
            let token = store(&mut mgr, m);
            let _ = q.try_push(token, &policy_fill, &mgr);
        }

        // Now construct the DropOldest policy which we will query against.
        let policy_drop_oldest =
            EdgePolicy::new(caps, AdmissionPolicy::DropOldest, OverBudgetAction::Drop);

        // Small token: should prompt EvictUntilBelowHard.
        let small_msg = make_msg_u32(20);
        let small_token = store(&mut mgr, small_msg);
        let decision_small = q.get_admission_decision(&policy_drop_oldest, small_token, &mgr);
        assert_eq!(
            decision_small,
            crate::policy::AdmissionDecision::EvictUntilBelowHard
        );

        // Now craft a token whose header reports a huge payload_size_bytes
        // that by itself exceeds the hard byte cap.
        let mut large_msg = make_msg_u32(30);
        large_msg.header_mut().set_payload_size_bytes(2048);
        let large_token = store(&mut mgr, large_msg);

        let decision_large = q.get_admission_decision(&policy_drop_oldest, large_token, &mgr);
        // Because the item's bytes alone exceed the hard cap even on an empty queue,
        // EdgePolicy::decide should return Reject.
        assert_eq!(decision_large, crate::policy::AdmissionDecision::Reject);
    }

    /// Admission semantics: item_bytes from header and deadline from header
    /// are correctly used by get_admission_decision.
    pub fn run_admission_item_bytes_and_deadline_semantics<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let caps = QueueCaps::new(100, 50, None, None);
        let q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();

        // Create a token with a specific deadline.
        let mut m = make_msg_u32(1);
        m.header_mut().set_deadline_ns(Some(DeadlineNs::new(2000)));
        let token = store(&mut mgr, m);

        let policy = EdgePolicy::new(
            caps,
            AdmissionPolicy::DeadlineAndQoSAware,
            OverBudgetAction::Drop,
        );

        // Ensure the queue admits this token (queue is empty, well below soft).
        let decision = q.get_admission_decision(&policy, token, &mgr);
        assert_eq!(decision, crate::policy::AdmissionDecision::Admit);

        // Verify the header's deadline is visible through the manager.
        let h = mgr.peek_header(token).unwrap();
        assert_eq!(*h.deadline_ns(), Some(DeadlineNs::new(2000)));
    }

    /// `try_peek_at` semantics:
    /// - empty queue => `Err(QueueError::Empty)`
    /// - valid indices [0..len) return the correct tokens without removing them
    /// - out-of-range index => `Err(QueueError::Empty)`
    pub fn run_try_peek_at<Q, F>(mut make: F)
    where
        F: FnMut() -> Q,
        Q: Edge,
    {
        let mut q = make();
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let policy = TEST_EDGE_POLICY;

        // empty behaviour
        assert!(matches!(q.try_peek_at(0), Err(QueueError::Empty)));

        // push ticks 1..=4
        let mut tokens = [MessageToken::INVALID; 4];
        for (i, t) in (1u64..=4u64).enumerate() {
            let m = make_msg_u32(t);
            tokens[i] = store(&mut mgr, m);
            assert_eq!(
                q.try_push(tokens[i], &policy, &mgr),
                EnqueueResult::Enqueued
            );
        }

        // in-range peeks
        for (idx, expected_tick) in (0usize..4usize).zip(1u64..=4u64) {
            let peek_token = q.try_peek_at(idx).expect("peek_at in range");
            assert_eq!(peek_token, tokens[idx]);
            let h = mgr.peek_header(peek_token).expect("header");
            assert_eq!(*h.creation_tick().as_u64(), expected_tick);
        }

        // out-of-range peek
        assert!(matches!(q.try_peek_at(4), Err(QueueError::Empty)));

        // ensure peeks did not remove anything (FIFO still intact)
        for (i, expected_tick) in (0usize..4usize).zip(1u64..=4u64) {
            let popped = q.try_pop(&mgr).expect("pop after peek_at");
            assert_eq!(popped, tokens[i]);
            let h = mgr.peek_header(popped).expect("header");
            assert_eq!(*h.creation_tick().as_u64(), expected_tick);
        }
        assert!(matches!(q.try_pop(&mgr), Err(QueueError::Empty)));
    }
}
