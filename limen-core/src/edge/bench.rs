//! (Work)bench [test] Queue implementation.
//!
//! A small, safe, no-alloc SPSC ring buffer intended for tests. This
//! implementation stores `MessageToken` directly in a `[MessageToken; N]`
//! array so it can return contiguous `&mut [MessageToken]` batch slices
//! without unsafe code. Byte-accounting is done via `HeaderStore` lookups.
//!
//! This test queue mirrors the semantics of `StaticRing` (the new static
//! MessageToken-based ring) so it can be exercised by the same Edge contract
//! tests.

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::QueueError;
use crate::policy::{AdmissionDecision, EdgePolicy};
use crate::prelude::{BatchView, HeaderStore};
use crate::types::MessageToken;

use core::mem;

/// A simple, no-alloc, no-unsafe SPSC ring buffer for tests that honors `EdgePolicy`.
///
/// Notes:
/// - Byte accounting is provided by the `HeaderStore` lookups (payload_size_bytes).
/// - `AdmissionPolicy::Block` returns `Rejected` in this test queue (no blocking).
pub struct TestSpscRingBuf<const N: usize> {
    /// Backing storage: always initialized with `MessageToken::default()`.
    buf: [MessageToken; N],

    /// Index of the front element (logical head).
    head: usize,

    /// Index one past the last live element (logical tail = (head + len) % N).
    tail: usize,

    /// Number of live elements currently in the ring.
    len: usize,

    /// Aggregate bytes currently stored (sum of message header+payload sizes).
    bytes: usize,
}

impl<const N: usize> TestSpscRingBuf<N> {
    /// Create a new empty ring.
    #[inline]
    pub fn new() -> Self {
        Self {
            buf: core::array::from_fn(|_| MessageToken::default()),
            head: 0,
            tail: 0,
            len: 0,
            bytes: 0,
        }
    }

    /// `true` when the ring is full.
    #[inline]
    fn is_full(&self) -> bool {
        self.len == N
    }

    /// Internal: push an item into the tail (assumes capacity available).
    ///
    /// Overwrites the slot at `tail` (which should be a default placeholder).
    #[inline]
    fn push_raw(&mut self, item: MessageToken) {
        self.buf[self.tail] = item;
        self.tail = (self.tail + 1) % N;
        self.len += 1;
    }

    /// Internal: pop an item from the head (assumes len > 0).
    ///
    /// Replaces the vacated slot with `MessageToken::default()` and returns the previous
    /// value stored there.
    #[inline]
    fn pop_raw(&mut self) -> MessageToken {
        let item = mem::replace(&mut self.buf[self.head], MessageToken::default());
        self.head = (self.head + 1) % N;
        self.len -= 1;
        item
    }

    /// Normalize the ring so the live region is contiguous at `buf[0..len]`.
    ///
    /// Moves items in-place using `mem::replace`, leaving default placeholders
    /// in unused slots. After normalization `head == 0` and `tail == (head + len) % N`.
    fn normalize(&mut self) {
        if self.len == 0 {
            // Empty ring: canonicalize indices.
            self.head = 0;
            self.tail = 0;
            return;
        }
        if self.head == 0 {
            // Already contiguous at start.
            self.tail = (self.head + self.len) % N;
            return;
        }

        // Move live items to the beginning in order.
        for i in 0..self.len {
            let src_idx = (self.head + i) % N;
            // Extract src value into tmp, leaving default in src slot.
            let tmp = mem::replace(&mut self.buf[src_idx], MessageToken::default());
            // Place the extracted value into destination slot `i`.
            self.buf[i] = tmp;
        }

        // Ensure remaining slots are defaulted (not strictly required but clearer).
        for i in self.len..N {
            let _ = mem::replace(&mut self.buf[i], MessageToken::default());
        }

        self.head = 0;
        self.tail = (self.head + self.len) % N;
    }
}

impl<const N: usize> Edge for TestSpscRingBuf<N> {
    fn try_push<H: HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult {
        // Compute a pure admission decision from the policy.
        let decision = self.get_admission_decision(policy, token, headers);

        // Helper: size of incoming item in bytes via HeaderStore.
        let item_bytes = headers
            .peek_header(token)
            .map(|h| *h.payload_size_bytes())
            .unwrap_or(0);

        match decision {
            AdmissionDecision::Admit => {
                // Ensure we have capacity in the ring and caps are satisfied.
                if self.is_full() || policy.caps.at_or_above_hard(self.len, self.bytes) {
                    return EnqueueResult::Rejected;
                }

                self.bytes = self.bytes.saturating_add(item_bytes);
                self.push_raw(token);
                EnqueueResult::Enqueued
            }

            AdmissionDecision::DropNewest => {
                // Do not mutate queue; indicate incoming item dropped.
                EnqueueResult::DroppedNewest
            }

            AdmissionDecision::Reject => EnqueueResult::Rejected,

            AdmissionDecision::Block => {
                // Test queue cannot block; translate to Rejected.
                EnqueueResult::Rejected
            }

            AdmissionDecision::Evict(n) => {
                // Evict up to n oldest items (or fewer if queue empties).
                for _ in 0..n {
                    if self.len == 0 {
                        break;
                    }
                    let ev = self.pop_raw();
                    let ev_bytes = headers
                        .peek_header(ev)
                        .map(|h| *h.payload_size_bytes())
                        .unwrap_or(0);
                    self.bytes = self.bytes.saturating_sub(ev_bytes);
                }

                // After evicting, ensure hard caps / fullness are satisfied.
                if policy.caps.at_or_above_hard(self.len, self.bytes) || self.is_full() {
                    return EnqueueResult::Rejected;
                }

                // Now try to enqueue.
                self.bytes = self.bytes.saturating_add(item_bytes);
                self.push_raw(token);
                EnqueueResult::Enqueued
            }

            AdmissionDecision::EvictUntilBelowHard => {
                // Evict until the caps report below-hard or queue empties.
                while policy.caps.at_or_above_hard(self.len, self.bytes) && self.len > 0 {
                    let ev = self.pop_raw();
                    let ev_bytes = headers
                        .peek_header(ev)
                        .map(|h| *h.payload_size_bytes())
                        .unwrap_or(0);
                    self.bytes = self.bytes.saturating_sub(ev_bytes);
                }

                // If the single item cannot fit even into an empty queue, reject.
                if policy.caps.at_or_above_hard(0, item_bytes) {
                    return EnqueueResult::Rejected;
                }

                // If still full (ring capacity) then reject.
                if self.is_full() || policy.caps.at_or_above_hard(self.len, self.bytes) {
                    return EnqueueResult::Rejected;
                }

                // Accept the item now that we've made room.
                self.bytes = self.bytes.saturating_add(item_bytes);
                self.push_raw(token);
                EnqueueResult::Enqueued
            }
        }
    }

    /// Try to pop a single token.
    fn try_pop<H: HeaderStore>(&mut self, headers: &H) -> Result<MessageToken, QueueError> {
        if self.len == 0 {
            return Err(QueueError::Empty);
        }
        let token = self.pop_raw();
        let tok_bytes = headers
            .peek_header(token)
            .map(|h| *h.payload_size_bytes())
            .unwrap_or(0);
        self.bytes = self.bytes.saturating_sub(tok_bytes);
        Ok(token)
    }

    /// Snapshot occupancy for telemetry / admission decisions.
    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        let items = self.len;
        let bytes = self.bytes;
        let watermark = policy.watermark(items, bytes);
        EdgeOccupancy {
            items,
            bytes,
            watermark,
        }
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Peek at the front token without removing it.
    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        if self.len == 0 {
            return Err(QueueError::Empty);
        }
        Ok(self.buf[self.head])
    }

    #[inline]
    fn try_peek_at(&self, index: usize) -> Result<MessageToken, QueueError> {
        if self.len == 0 || index >= self.len {
            return Err(QueueError::Empty);
        }

        let idx = (self.head + index) % N;
        Ok(self.buf[idx])
    }

    /// Pop a batch of tokens according to `BatchingPolicy`.
    ///
    /// - Disjoint windows: pop up to `fixed_n` or `max_delta_t`.
    /// - Sliding windows: return `size` items but only pop/advance `stride` items.
    /// - `fixed_n` and `max_delta_t` combine: stop when either limit reached.
    /// - No `fixed_n` or `max_delta_t`: treat as `fixed_n` = 1.
    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        policy: &crate::policy::BatchingPolicy,
        headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        use crate::policy::WindowKind;

        if self.len == 0 {
            return Err(QueueError::Empty);
        }

        // Normalize to make live items contiguous at buf[0..len].
        self.normalize();

        // Keep old len for later arithmetic.
        let old_len = self.len;

        // Extract policy knobs.
        let fixed_opt = *policy.fixed_n();
        let delta_t_opt = *policy.max_delta_t();
        let window_kind = policy.window_kind();

        // If both caps are absent, treat as fixed_n = 1 (per tightened semantics).
        let effective_fixed: Option<usize> = if fixed_opt.is_none() && delta_t_opt.is_none() {
            Some(1)
        } else {
            fixed_opt
        };

        // Compute how many items are within max_delta_t relative to the front, if any.
        let mut delta_count = self.len;
        if let Some(cap) = delta_t_opt {
            // Use creation_tick for age calculation via HeaderStore.
            if let Ok(front_header) = headers.peek_header(self.buf[0]) {
                let front_ticks = *front_header.creation_tick();
                let mut c = 0usize;
                while c < self.len {
                    if let Ok(h) = headers.peek_header(self.buf[c]) {
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
            let take_n = apply_fixed(core::cmp::min(self.len, delta_count));
            if take_n == 0 {
                return Err(QueueError::Empty);
            }

            // Advance logical state first (normalized head == 0).
            let new_head = take_n % N;
            self.len = old_len - take_n;
            self.head = new_head;
            self.tail = (self.head + self.len) % N;

            // Subtract bytes for the popped items.
            let mut dropped_bytes = 0usize;
            for i in 0..take_n {
                if let Ok(h) = headers.peek_header(self.buf[i]) {
                    dropped_bytes = dropped_bytes.saturating_add(*h.payload_size_bytes());
                }
            }
            self.bytes = self.bytes.saturating_sub(dropped_bytes);

            // Return a borrowed slice covering the popped items.
            let slice = &mut self.buf[..take_n];
            return Ok(BatchView::from_borrowed(slice, take_n));
        }

        // --- Sliding windows: present `size` but pop `stride`.
        if let WindowKind::Sliding(sw) = window_kind {
            let stride = *sw.stride();
            let size = effective_fixed.unwrap_or(1);

            // Determine how many items we can present, bounded by availability, size, delta_count, and fixed.
            let mut max_present = core::cmp::min(self.len, size);
            max_present = apply_fixed(core::cmp::min(max_present, delta_count));

            // How many to actually pop from the front (stride), bounded by availability.
            let stride_to_pop = core::cmp::min(stride, self.len);

            if max_present == 0 {
                return Err(QueueError::Empty);
            }

            // Advance logical state by stride_to_pop (we pop only stride).
            let new_head = stride_to_pop % N;
            self.len = old_len - stride_to_pop;
            self.head = new_head;
            self.tail = (self.head + self.len) % N;

            // Subtract bytes only for the popped items.
            let mut popped_bytes = 0usize;
            for i in 0..stride_to_pop {
                if let Ok(h) = headers.peek_header(self.buf[i]) {
                    popped_bytes = popped_bytes.saturating_add(*h.payload_size_bytes());
                }
            }
            self.bytes = self.bytes.saturating_sub(popped_bytes);

            // Return a borrowed slice of `max_present` items (first `stride_to_pop` are popped).
            let slice = &mut self.buf[..max_present];
            return Ok(BatchView::from_borrowed(slice, max_present));
        }

        // --- Fixed-N and/or max_delta_t (non-sliding, non-disjoint).
        let mut take_n = core::cmp::min(self.len, delta_count);
        take_n = apply_fixed(take_n);

        if take_n == 0 {
            return Err(QueueError::Empty);
        }

        // Advance logical state by take_n.
        let new_head = take_n % N;
        self.len = old_len - take_n;
        self.head = new_head;
        self.tail = (self.head + self.len) % N;

        // Subtract bytes for the popped items.
        let mut dropped_bytes = 0usize;
        for i in 0..take_n {
            if let Ok(h) = headers.peek_header(self.buf[i]) {
                dropped_bytes = dropped_bytes.saturating_add(*h.payload_size_bytes());
            }
        }
        self.bytes = self.bytes.saturating_sub(dropped_bytes);

        let slice = &mut self.buf[..take_n];
        Ok(BatchView::from_borrowed(slice, take_n))
    }
}

impl<const N: usize> Default for TestSpscRingBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Runs the full Edge contract suite against TestSpscRingBuf<MessageToken, 16>.
    crate::run_edge_contract_tests!(test_spsc_ring_buf_contract, || {
        TestSpscRingBuf::<16>::new()
    });
}
