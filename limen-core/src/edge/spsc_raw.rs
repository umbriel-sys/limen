//! **WARNING** **This module is currently an unfinished stub and should not be
//! used in its current form, it will either be updated or removed before
//! release.**
//! SpscAtomicRing: high-performance SPSC ring using atomics (P2 high-perf).
//!
//! **Unsafe implementation** gated behind `ring_unsafe`.
//! Uses a power-of-two capacity array of `MaybeUninit<T>` with atomic head/tail.

#![allow(unsafe_code)]

use core::ptr;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::QueueError;
use crate::policy::EdgePolicy;
use crate::prelude::{AdmissionDecision, BatchView, HeaderStore};
use crate::types::MessageToken;

/// A high-performance, bounded, single-producer single-consumer ring buffer.
///
/// Stores `MessageToken` elements in a power-of-two array of `MaybeUninit<T>`
/// and advances head and tail indices with atomic operations. Also tracks a
/// byte counter for admission control policies. This type assumes strict
/// single-producer single-consumer usage throughout its lifetime.
pub struct SpscAtomicRing {
    buf: Box<[MaybeUninit<MessageToken>]>,
    cap: usize,
    head: AtomicUsize, // consumer index
    tail: AtomicUsize, // producer index
    bytes_in_queue: AtomicUsize,
}

impl SpscAtomicRing {
    /// Creates a ring with the given item capacity.
    ///
    /// The capacity must be a power of two; indices wrap using a bit mask.
    ///
    /// # Safety
    ///
    /// See module docs — this constructor relies on strict SPSC usage.
    pub unsafe fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two(),
            "capacity must be a power of two"
        );
        let mut v: Vec<MaybeUninit<MessageToken>> = Vec::with_capacity(capacity);
        unsafe {
            v.set_len(capacity);
        }
        Self {
            buf: v.into_boxed_slice(),
            cap: capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            bytes_in_queue: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn mask(&self) -> usize {
        self.cap - 1
    }

    #[inline]
    fn len(&self) -> usize {
        let h = self.head.load(Ordering::Acquire);
        let t = self.tail.load(Ordering::Acquire);
        t.wrapping_sub(h) & self.mask()
    }

    #[inline]
    fn is_full(&self) -> bool {
        self.len() == self.cap - 1
    }

    #[inline]
    fn push_raw(&self, token: MessageToken) {
        // Compute a *mut MessageToken to the target slot and write without dropping the old value.
        let t = self.tail.load(Ordering::Relaxed);
        let idx = t & self.mask();
        let base: *mut MaybeUninit<MessageToken> =
            self.buf.as_ptr() as *mut MaybeUninit<MessageToken>;
        let slot: *mut MessageToken = unsafe { base.add(idx) as *mut MessageToken };
        unsafe { ptr::write(slot, token) };
        self.tail.store(t.wrapping_add(1), Ordering::Release);
    }

    #[inline]
    fn pop_raw(&self) -> MessageToken {
        // Compute a *const MessageToken to the current head slot and read it by value.
        let h = self.head.load(Ordering::Relaxed);
        let idx = h & self.mask();
        let base: *const MaybeUninit<MessageToken> = self.buf.as_ptr();
        let slot: *const MessageToken = unsafe { base.add(idx) as *const MessageToken };
        let token = unsafe { ptr::read(slot) };
        self.head.store(h.wrapping_add(1), Ordering::Release);
        token
    }

    /// Borrow a reference to the token at `head + offset` without advancing head.
    ///
    /// # Safety
    /// Requires SPSC discipline. The returned reference must not outlive `&self`
    /// and is only valid while the corresponding slot remains logically occupied.
    #[inline]
    fn peek_ref_at_offset(&self, offset: usize) -> &MessageToken {
        let h = self.head.load(Ordering::Acquire);
        let idx = h.wrapping_add(offset) & self.mask();
        let base: *const MaybeUninit<MessageToken> = self.buf.as_ptr();
        let slot: *const MessageToken = unsafe { base.add(idx) as *const MessageToken };
        unsafe { &*slot }
    }
}

impl Drop for SpscAtomicRing {
    fn drop(&mut self) {
        // Drain any initialized tokens (if any) to avoid leaking owned resources
        // if MessageToken owns something. Most token types are small/copy, but
        // drop safely anyway.
        while self.len() > 0 {
            let _ = self.pop_raw();
        }
    }
}

impl Edge for SpscAtomicRing {
    fn try_push<H: HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult {
        // Compute a pure admission decision using the HeaderStore.
        let decision = self.get_admission_decision(policy, token, headers);

        // Look up incoming token's bytes via HeaderStore.
        let item_bytes = headers
            .peek_header(token)
            .map(|h| *h.payload_size_bytes())
            .unwrap_or(0);

        match decision {
            AdmissionDecision::Admit => {
                let items = self.len();
                let bytes = self.bytes_in_queue.load(Ordering::Acquire);
                if self.is_full() || policy.caps.at_or_above_hard(items, bytes) {
                    return EnqueueResult::Rejected;
                }

                self.bytes_in_queue.fetch_add(item_bytes, Ordering::AcqRel);
                self.push_raw(token);
                EnqueueResult::Enqueued
            }
            AdmissionDecision::DropNewest => EnqueueResult::DroppedNewest,
            AdmissionDecision::Reject => EnqueueResult::Rejected,
            AdmissionDecision::Block => EnqueueResult::Rejected,
            AdmissionDecision::Evict(_) | AdmissionDecision::EvictUntilBelowHard => {
                // Eviction is the caller's responsibility.
                let items = self.len();
                let bytes = self.bytes_in_queue.load(Ordering::Acquire);
                if self.is_full() || policy.caps.at_or_above_hard(items, bytes) {
                    return EnqueueResult::Rejected;
                }
                self.bytes_in_queue.fetch_add(item_bytes, Ordering::AcqRel);
                self.push_raw(token);
                EnqueueResult::Enqueued
            }
        }
    }

    fn try_pop<H: HeaderStore>(&mut self, headers: &H) -> Result<MessageToken, QueueError> {
        if self.len() == 0 {
            return Err(QueueError::Empty);
        }
        let token = self.pop_raw();
        let tok_bytes = headers
            .peek_header(token)
            .map(|h| *h.payload_size_bytes())
            .unwrap_or(0);
        self.bytes_in_queue.fetch_sub(tok_bytes, Ordering::AcqRel);
        Ok(token)
    }

    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        let items = self.len();
        let bytes = self.bytes_in_queue.load(Ordering::Acquire);
        let watermark = policy.watermark(items, bytes);
        EdgeOccupancy::new(items, bytes, watermark)
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        if self.len() == 0 {
            return Err(QueueError::Empty);
        }
        // Return a copy of the token at head.
        let t = self.peek_ref_at_offset(0);
        Ok(*t)
    }

    fn try_peek_at(&self, index: usize) -> Result<MessageToken, QueueError> {
        let available = self.len();
        if index >= available {
            return Err(QueueError::Empty);
        }
        let t = self.peek_ref_at_offset(index);
        Ok(*t)
    }

    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        policy: &crate::policy::BatchingPolicy,
        headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        use crate::policy::WindowKind;

        let available = self.len();
        if available == 0 {
            return Err(QueueError::Empty);
        }

        let fixed_opt = *policy.fixed_n();
        let delta_t_opt = *policy.max_delta_t();
        let window_kind = policy.window_kind();

        // If both caps are absent, treat as fixed_n = 1.
        let effective_fixed: Option<usize> = if fixed_opt.is_none() && delta_t_opt.is_none() {
            Some(1)
        } else {
            fixed_opt
        };

        // Compute how many items are within max_delta_t relative to the front, if any.
        let mut delta_count = available;
        if let Some(cap) = delta_t_opt {
            // Use HeaderStore to read creation ticks.
            if let Ok(front_header) = headers.peek_header(*self.peek_ref_at_offset(0)) {
                let front_ticks = *front_header.creation_tick();
                let mut c = 0usize;
                while c < available {
                    if let Ok(h) = headers.peek_header(*self.peek_ref_at_offset(c)) {
                        let tick = *h.creation_tick();
                        let delta = tick.saturating_sub(front_ticks);
                        if delta <= cap {
                            c += 1;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                delta_count = c;
            }
        }

        // Helper to apply effective fixed-N cap (if present).
        let apply_fixed = |limit: usize| -> usize {
            if let Some(n) = effective_fixed {
                core::cmp::min(limit, n)
            } else {
                limit
            }
        };

        // --- Disjoint windows: pop up to fixed / delta_count.
        if let WindowKind::Disjoint = window_kind {
            let take_n = apply_fixed(core::cmp::min(available, delta_count));
            if take_n == 0 {
                return Err(QueueError::Empty);
            }

            let mut out: alloc::vec::Vec<MessageToken> = alloc::vec::Vec::with_capacity(take_n);
            for _ in 0..take_n {
                let tok = self.pop_raw();
                let tok_bytes = headers
                    .peek_header(tok)
                    .map(|h| *h.payload_size_bytes())
                    .unwrap_or(0);
                self.bytes_in_queue.fetch_sub(tok_bytes, Ordering::AcqRel);
                out.push(tok);
            }

            return Ok(BatchView::from_owned(out));
        }

        // --- Sliding windows: present `size` but pop `stride`.
        if let WindowKind::Sliding(sw) = window_kind {
            let stride = *sw.stride();
            let size = effective_fixed.unwrap_or(1);

            let mut max_present = core::cmp::min(available, size);
            max_present = apply_fixed(core::cmp::min(max_present, delta_count));

            if max_present == 0 {
                return Err(QueueError::Empty);
            }

            let stride_to_pop = core::cmp::min(stride, available);

            let mut out: alloc::vec::Vec<MessageToken> =
                alloc::vec::Vec::with_capacity(max_present);

            // Pop (move) the first `stride_to_pop` tokens.
            for _ in 0..stride_to_pop {
                let tok = self.pop_raw();
                let tok_bytes = headers
                    .peek_header(tok)
                    .map(|h| *h.payload_size_bytes())
                    .unwrap_or(0);
                self.bytes_in_queue.fetch_sub(tok_bytes, Ordering::AcqRel);
                out.push(tok);
            }

            // For the remainder, clone tokens from the queue without advancing head.
            for i in stride_to_pop..max_present {
                let t = *self.peek_ref_at_offset(i - stride_to_pop);
                out.push(t);
            }

            return Ok(BatchView::from_owned(out));
        }

        // --- Fixed-N and/or max_delta_t (non-sliding, non-disjoint).
        let mut take_n = core::cmp::min(available, delta_count);
        take_n = apply_fixed(take_n);

        if take_n == 0 {
            return Err(QueueError::Empty);
        }

        let mut out: alloc::vec::Vec<MessageToken> = alloc::vec::Vec::with_capacity(take_n);
        for _ in 0..take_n {
            let tok = self.pop_raw();
            let tok_bytes = headers
                .peek_header(tok)
                .map(|h| *h.payload_size_bytes())
                .unwrap_or(0);
            self.bytes_in_queue.fetch_sub(tok_bytes, Ordering::AcqRel);
            out.push(tok);
        }

        Ok(BatchView::from_owned(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::static_manager::StaticMemoryManager;
    use crate::message::{Message, MessageHeader};
    use crate::policy::{AdmissionPolicy, EdgePolicy, OverBudgetAction, QueueCaps};
    use crate::prelude::{HeaderStore, MemoryManager as _};
    use crate::types::Ticks;

    // Helper to construct a MessageHeader with a creation tick and payload size.
    fn mk_header(tick: u64) -> MessageHeader {
        let mut h = MessageHeader::empty();
        h.set_creation_tick(Ticks::new(tick));
        h.set_payload_size_bytes(8usize);
        h
    }

    // Helper to construct a Message<u32> with a header.
    fn make_msg_with_tick(tick: u64) -> Message<u32> {
        let h = mk_header(tick);
        Message::new(h, 0u32)
    }

    // Ring constructor used by the contract harness.
    fn make_ring() -> SpscAtomicRing {
        // Needs to be power-of-two; usable capacity is cap - 1.
        const CAPACITY: usize = 32;
        unsafe { SpscAtomicRing::with_capacity(CAPACITY) }
    }

    const POLICY: EdgePolicy = EdgePolicy::new(
        QueueCaps::new(8, 6, None, None),
        AdmissionPolicy::DropNewest,
        OverBudgetAction::Drop,
    );

    // Run full edge contract suite. The contract macro will create rings using
    // the closure below; the ring-level tests below exercise header/byte logic.
    crate::run_edge_contract_tests!(spsc_atomic_ring_contract, || make_ring());

    #[test]
    fn pushes_and_pops_tokens_with_byte_accounting() {
        // Use a static memory manager to allocate Message<u32> instances and
        // obtain MessageToken values. The manager also implements HeaderStore.
        const MGR_DEPTH: usize = 64;
        let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();
        let mut ring = make_ring();

        // Store two messages and obtain tokens.
        let t1 = mgr.store(make_msg_with_tick(1)).expect("store t1");
        let t2 = mgr.store(make_msg_with_tick(2)).expect("store t2");

        // Push tokens into the ring with the policy and the manager as HeaderStore.
        assert_eq!(ring.try_push(t1, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert_eq!(ring.try_push(t2, &POLICY, &mgr), EnqueueResult::Enqueued);

        // Occupancy should report 2 items and bytes > 0.
        let occ = ring.occupancy(&POLICY);
        assert_eq!(*occ.items(), 2usize);
        assert!(*occ.bytes() > 0usize);

        // Pop tokens and check headers through the manager.
        let p1 = ring.try_pop(&mgr).expect("pop p1");
        let p2 = ring.try_pop(&mgr).expect("pop p2");

        let h1 = mgr.peek_header(p1).expect("h1");
        let h2 = mgr.peek_header(p2).expect("h2");

        assert_eq!(*h1.creation_tick().as_u64(), 1u64);
        assert_eq!(*h2.creation_tick().as_u64(), 2u64);
    }

    // Additional parity tests: exhaustion, eviction, etc., can reuse the manager
    // pattern above if desired. The contract macro already covers the queue API.
}
