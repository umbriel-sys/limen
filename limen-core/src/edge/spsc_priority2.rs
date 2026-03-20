//! Two-lane priority wrapper for SPSC queues (feature: `priority_lanes`).
//!
//! This composes two underlying SPSC queues (hi/lo) that store the same
//! `MessageToken` handles and routes `try_push` by inspecting the message
//! header's QoS class. `try_pop` always prefers the high-priority lane when
//! available.

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::QueueError;
use crate::policy::EdgePolicy;
use crate::prelude::BatchView;
use crate::types::MessageToken;
use crate::types::QoSClass;

extern crate alloc;

/// Two-lane priority queue. The lanes store `MessageToken` values and the
/// priority decision is made using the provided `HeaderStore`.
pub struct Priority2<QHi, QLo>
where
    QHi: Edge,
    QLo: Edge,
{
    hi: QHi,
    lo: QLo,
}

impl<QHi, QLo> Priority2<QHi, QLo>
where
    QHi: Edge,
    QLo: Edge,
{
    /// Build a two-lane priority queue from hi/lo queues.
    pub fn new(hi: QHi, lo: QLo) -> Self {
        Self { hi, lo }
    }
}

impl<QHi, QLo> Edge for Priority2<QHi, QLo>
where
    QHi: Edge,
    QLo: Edge,
{
    fn try_push<H: crate::prelude::HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult {
        // Resolve QoS via HeaderStore; default to BestEffort if header missing.
        let qos = headers
            .peek_header(token)
            .map(|h| *h.qos())
            .unwrap_or(QoSClass::BestEffort);

        match qos {
            QoSClass::LatencyCritical => self.hi.try_push(token, policy, headers),
            _ => self.lo.try_push(token, policy, headers),
        }
    }

    fn try_pop<H: crate::prelude::HeaderStore>(
        &mut self,
        headers: &H,
    ) -> Result<MessageToken, QueueError> {
        match self.hi.try_pop(headers) {
            Ok(tok) => Ok(tok),
            Err(QueueError::Empty) => self.lo.try_pop(headers),
            Err(e) => Err(e),
        }
    }

    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        let hi = self.hi.occupancy(policy);
        let lo = self.lo.occupancy(policy);
        let items = hi.items() + lo.items();
        let bytes = hi.bytes() + lo.bytes();
        let watermark = policy.watermark(items, bytes);
        EdgeOccupancy::new(items, bytes, watermark)
    }

    fn is_empty(&self) -> bool {
        self.hi.is_empty() && self.lo.is_empty()
    }

    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        match self.hi.try_peek() {
            Ok(tok) => Ok(tok),
            Err(QueueError::Empty) => self.lo.try_peek(),
            Err(e) => Err(e),
        }
    }

    fn try_peek_at(&self, index: usize) -> Result<MessageToken, QueueError> {
        match self.hi.try_peek_at(index) {
            Ok(tok) => Ok(tok),
            Err(QueueError::Empty) => self.lo.try_peek_at(index),
            Err(e) => Err(e),
        }
    }

    fn try_pop_batch<H: crate::prelude::HeaderStore>(
        &mut self,
        policy: &crate::policy::BatchingPolicy,
        headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        match self.hi.try_pop_batch(policy, headers) {
            Ok(batch) => Ok(batch),
            Err(QueueError::Empty) => self.lo.try_pop_batch(policy, headers),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::edge::bench::TestSpscRingBuf;
    use crate::memory::manager::MemoryManager;
    use crate::memory::static_manager::StaticMemoryManager;
    use crate::message::{Message, MessageHeader};
    use crate::policy::{AdmissionPolicy, BatchingPolicy, EdgePolicy, OverBudgetAction, QueueCaps};
    use crate::prelude::HeaderStore as _;
    use crate::types::{QoSClass, Ticks};

    const POLICY: EdgePolicy = EdgePolicy::new(
        QueueCaps::new(8, 6, None, None),
        AdmissionPolicy::DropNewest,
        OverBudgetAction::Drop,
    );

    const MGR_DEPTH: usize = 32;

    fn make_msg(tick: u64, qos: QoSClass) -> Message<u32> {
        let mut h = MessageHeader::empty();
        h.set_creation_tick(Ticks::new(tick));
        h.set_qos(qos);
        Message::new(h, 0u32)
    }

    fn store(mgr: &mut StaticMemoryManager<u32, MGR_DEPTH>, msg: Message<u32>) -> MessageToken {
        mgr.store(msg).expect("memory manager store failed")
    }

    // --- 1) Run the full Edge contract suite against Priority2 ---
    //
    // The contract tests push messages with default QoS (BestEffort), so all
    // traffic routes to the lo lane. This validates that Priority2 satisfies
    // the Edge contract for the common single-lane path.
    crate::run_edge_contract_tests!(priority2_edge_contract, || {
        let hi = TestSpscRingBuf::<16>::new();
        let lo = TestSpscRingBuf::<16>::new();
        Priority2::new(hi, lo)
    });

    // --- 2) Priority-specific behaviour tests ---

    #[test]
    fn routes_latency_critical_to_hi_and_others_to_lo() {
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let mut q = Priority2::new(TestSpscRingBuf::<16>::new(), TestSpscRingBuf::<16>::new());

        let t_hi = store(&mut mgr, make_msg(1, QoSClass::LatencyCritical));
        let t_lo = store(&mut mgr, make_msg(2, QoSClass::BestEffort));

        assert_eq!(q.try_push(t_hi, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert_eq!(q.try_push(t_lo, &POLICY, &mgr), EnqueueResult::Enqueued);

        // `try_pop` should return hi first (tick=1), then lo (tick=2)
        let a = q.try_pop(&mgr).expect("pop hi");
        let b = q.try_pop(&mgr).expect("pop lo");

        let ha = mgr.peek_header(a).expect("ha");
        let hb = mgr.peek_header(b).expect("hb");
        assert_eq!(*ha.creation_tick(), Ticks::new(1));
        assert_eq!(*hb.creation_tick(), Ticks::new(2));
    }

    #[test]
    fn peek_prefers_hi_when_both_non_empty() {
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let mut q = Priority2::new(TestSpscRingBuf::<16>::new(), TestSpscRingBuf::<16>::new());

        // Push lo first, then hi — peek should still return hi.
        let t_lo = store(&mut mgr, make_msg(10, QoSClass::BestEffort));
        let t_hi = store(&mut mgr, make_msg(20, QoSClass::LatencyCritical));

        assert_eq!(q.try_push(t_lo, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert_eq!(q.try_push(t_hi, &POLICY, &mgr), EnqueueResult::Enqueued);

        let peek_tok = q.try_peek().expect("peek");
        let ph = mgr.peek_header(peek_tok).expect("peek header");
        assert_eq!(*ph.creation_tick(), Ticks::new(20));

        // Ensure peek did not remove it.
        let popped = q.try_pop(&mgr).expect("pop");
        assert_eq!(popped, peek_tok);
        let popped_h = mgr.peek_header(popped).expect("popped header");
        assert_eq!(*popped_h.creation_tick(), Ticks::new(20));
    }

    #[test]
    fn pop_batch_prefers_hi_when_non_empty() {
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let mut q = Priority2::new(TestSpscRingBuf::<16>::new(), TestSpscRingBuf::<16>::new());

        // lo: ticks 1,2,3
        let mut lo_tokens = [MessageToken::INVALID; 3];
        for (i, t) in (1u64..=3u64).enumerate() {
            lo_tokens[i] = store(&mut mgr, make_msg(t, QoSClass::BestEffort));
            assert_eq!(
                q.try_push(lo_tokens[i], &POLICY, &mgr),
                EnqueueResult::Enqueued
            );
        }

        // hi: ticks 100,101
        let mut hi_tokens = [MessageToken::INVALID; 2];
        for (i, t) in (100u64..=101u64).enumerate() {
            hi_tokens[i] = store(&mut mgr, make_msg(t, QoSClass::LatencyCritical));
            assert_eq!(
                q.try_push(hi_tokens[i], &POLICY, &mgr),
                EnqueueResult::Enqueued
            );
        }

        let batch_policy = BatchingPolicy::fixed(4);

        // Should batch from hi lane first (only 2 items available there).
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

        // Remaining should be lo items in order.
        let ra = q.try_pop(&mgr).expect("lo-1");
        let rb = q.try_pop(&mgr).expect("lo-2");
        let rc = q.try_pop(&mgr).expect("lo-3");
        assert_eq!(*mgr.peek_header(ra).unwrap().creation_tick(), Ticks::new(1));
        assert_eq!(*mgr.peek_header(rb).unwrap().creation_tick(), Ticks::new(2));
        assert_eq!(*mgr.peek_header(rc).unwrap().creation_tick(), Ticks::new(3));
    }

    #[test]
    fn occupancy_is_sum_of_lanes() {
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let mut q = Priority2::new(TestSpscRingBuf::<16>::new(), TestSpscRingBuf::<16>::new());

        let t1 = store(&mut mgr, make_msg(1, QoSClass::LatencyCritical));
        let t2 = store(&mut mgr, make_msg(2, QoSClass::BestEffort));

        assert_eq!(q.try_push(t1, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert_eq!(q.try_push(t2, &POLICY, &mgr), EnqueueResult::Enqueued);

        let occ = q.occupancy(&POLICY);
        assert_eq!(*occ.items(), 2usize);

        // bytes should reflect stored payload sizes (u32 = 4 bytes each).
        assert!(*occ.bytes() > 0usize);
    }

    #[test]
    fn is_empty_reflects_both_lanes() {
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let mut q = Priority2::new(TestSpscRingBuf::<16>::new(), TestSpscRingBuf::<16>::new());

        assert!(q.is_empty());

        // Push to hi lane only.
        let t_hi = store(&mut mgr, make_msg(1, QoSClass::LatencyCritical));
        assert_eq!(q.try_push(t_hi, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert!(!q.is_empty());

        // Drain hi.
        let _ = q.try_pop(&mgr).expect("pop hi");
        assert!(q.is_empty());

        // Push to lo lane only.
        let t_lo = store(&mut mgr, make_msg(2, QoSClass::BestEffort));
        assert_eq!(q.try_push(t_lo, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert!(!q.is_empty());

        // Drain lo.
        let _ = q.try_pop(&mgr).expect("pop lo");
        assert!(q.is_empty());
    }

    #[test]
    fn background_qos_routes_to_lo() {
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let mut q = Priority2::new(TestSpscRingBuf::<16>::new(), TestSpscRingBuf::<16>::new());

        let t_bg = store(&mut mgr, make_msg(1, QoSClass::Background));
        let t_be = store(&mut mgr, make_msg(2, QoSClass::BestEffort));

        assert_eq!(q.try_push(t_bg, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert_eq!(q.try_push(t_be, &POLICY, &mgr), EnqueueResult::Enqueued);

        // Both should be in lo lane; hi is empty.
        // Pop order should be FIFO within lo: tick 1, then tick 2.
        let a = q.try_pop(&mgr).expect("pop bg");
        let b = q.try_pop(&mgr).expect("pop be");
        assert_eq!(*mgr.peek_header(a).unwrap().creation_tick(), Ticks::new(1));
        assert_eq!(*mgr.peek_header(b).unwrap().creation_tick(), Ticks::new(2));
        assert!(q.is_empty());
    }

    #[test]
    fn lane_specific_admission_interactions_smoke_test() {
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let mut q = Priority2::new(TestSpscRingBuf::<2>::new(), TestSpscRingBuf::<2>::new());

        let small_caps_policy = EdgePolicy::new(
            QueueCaps::new(2, 1, None, None),
            AdmissionPolicy::DropOldest,
            OverBudgetAction::Drop,
        );

        let t1 = store(&mut mgr, make_msg(1, QoSClass::LatencyCritical));
        let t2 = store(&mut mgr, make_msg(2, QoSClass::LatencyCritical));

        assert_eq!(
            q.try_push(t1, &small_caps_policy, &mgr),
            EnqueueResult::Enqueued
        );
        // With DropOldest and soft_items=1, pushing 2nd item evicts oldest in hi lane.
        let res = q.try_push(t2, &small_caps_policy, &mgr);
        assert_eq!(res, EnqueueResult::Evicted(t1));

        // Only the newest (tick=2) should remain.
        let popped = q.try_pop(&mgr).expect("pop");
        assert_eq!(
            *mgr.peek_header(popped).unwrap().creation_tick(),
            Ticks::new(2)
        );
        assert!(q.is_empty());
    }

    #[test]
    fn hi_drains_before_lo_across_multiple_pops() {
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let mut q = Priority2::new(TestSpscRingBuf::<16>::new(), TestSpscRingBuf::<16>::new());

        // Interleave pushes: lo, hi, lo, hi
        let t_lo1 = store(&mut mgr, make_msg(1, QoSClass::BestEffort));
        let t_hi1 = store(&mut mgr, make_msg(2, QoSClass::LatencyCritical));
        let t_lo2 = store(&mut mgr, make_msg(3, QoSClass::BestEffort));
        let t_hi2 = store(&mut mgr, make_msg(4, QoSClass::LatencyCritical));

        assert_eq!(q.try_push(t_lo1, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert_eq!(q.try_push(t_hi1, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert_eq!(q.try_push(t_lo2, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert_eq!(q.try_push(t_hi2, &POLICY, &mgr), EnqueueResult::Enqueued);

        // Pop order: hi lane FIFO first (tick 2, 4), then lo lane FIFO (tick 1, 3).
        let p1 = q.try_pop(&mgr).expect("pop 1");
        let p2 = q.try_pop(&mgr).expect("pop 2");
        let p3 = q.try_pop(&mgr).expect("pop 3");
        let p4 = q.try_pop(&mgr).expect("pop 4");

        assert_eq!(*mgr.peek_header(p1).unwrap().creation_tick(), Ticks::new(2));
        assert_eq!(*mgr.peek_header(p2).unwrap().creation_tick(), Ticks::new(4));
        assert_eq!(*mgr.peek_header(p3).unwrap().creation_tick(), Ticks::new(1));
        assert_eq!(*mgr.peek_header(p4).unwrap().creation_tick(), Ticks::new(3));
        assert!(q.is_empty());
    }
}
