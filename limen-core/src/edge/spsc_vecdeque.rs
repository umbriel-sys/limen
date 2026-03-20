//! Heap-backed SPSC ring buffer for P1 (no_std + alloc), with optional physical capacity.
//!
//! - If `cap = Some(n)`: bounded mode, fixed backing buffer, zero-copy borrowed
//!   `BatchView` results are guaranteed.
//! - If `cap = None`: unbounded mode, backing buffer is allowed to grow; batch
//!   results are returned as owned `Vec<MessageToken>` to avoid returning
//!   borrowed slices that could be invalidated by reallocation.

use alloc::vec::Vec;

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::QueueError;
use crate::policy::{AdmissionDecision, EdgePolicy};
use crate::prelude::{BatchView, HeaderStore};
use crate::types::MessageToken;

use core::mem;

/// Heap ring storing `MessageToken`s. `cap` is `Some(n)` for bounded mode, or
/// `None` to allow unbounded growth.
pub struct HeapRing {
    buf: Vec<MessageToken>,
    head: usize,
    tail: usize,
    len: usize,
    cap: Option<usize>,
    /// Running byte total, updated on push/pop via HeaderStore lookups.
    bytes: usize,
}

impl HeapRing {
    /// Create a new bounded ring with the given fixed capacity in items.
    pub fn with_capacity(cap: usize) -> Self {
        let mut v = Vec::with_capacity(cap);
        // Initialize with default tokens to avoid unsafe and allow mem::replace.
        v.resize_with(cap, MessageToken::default);
        Self {
            buf: v,
            head: 0,
            tail: 0,
            len: 0,
            cap: Some(cap),
            bytes: 0,
        }
    }

    /// Create a new *unbounded* ring. Backing buffer grows on demand.
    /// Note: in unbounded mode `try_pop_batch` returns owned batches.
    pub fn unbounded() -> Self {
        Self {
            buf: Vec::new(),
            head: 0,
            tail: 0,
            len: 0,
            cap: None,
            bytes: 0,
        }
    }

    /// Current logical length (number of live tokens).
    #[inline]
    fn len(&self) -> usize {
        self.len
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `true` when the ring is physically full (only meaningful in bounded mode).
    #[inline]
    fn is_full(&self) -> bool {
        match self.cap {
            Some(c) => self.len >= c,
            None => false,
        }
    }

    /// Effective physical capacity (configured cap for bounded mode,
    /// current backing length for unbounded mode).
    #[inline]
    fn physical_capacity(&self) -> usize {
        match self.cap {
            Some(c) => c,
            None => self.buf.len(),
        }
    }

    /// Ensure there is space to push one token. In bounded mode this is a no-op
    /// (caller should check is_full). In unbounded mode this resizes the buffer
    /// when necessary to provide room and preserve mem::replace semantics.
    fn ensure_capacity_for_push(&mut self) {
        if self.cap.is_some() {
            // bounded: buffer was pre-sized at creation; nothing to do.
            return;
        }

        // unbounded: ensure buf has at least some capacity and at least one free slot.
        if self.buf.is_empty() {
            // choose a modest starting capacity
            let start = 4usize;
            self.buf.resize_with(start, MessageToken::default);
            self.head = 0;
            self.tail = 0;
            return;
        }

        // If buffer is "full" (len == buf.len()), grow by doubling.
        if self.len >= self.buf.len() {
            let new_cap = core::cmp::max(4, self.buf.len() * 2);
            self.buf.resize_with(new_cap, MessageToken::default);
            // No reordering required; we extended the buffer with default placeholders.
        }
    }

    /// Internal: push a token at tail (assumes capacity available or `ensure_capacity_for_push` called).
    #[inline]
    fn push_raw(&mut self, token: MessageToken) {
        if self.cap.is_none() {
            self.ensure_capacity_for_push();
        }

        let cap = self.physical_capacity();
        // write into slot (slot should exist and be default placeholder)
        self.buf[self.tail] = token;
        self.tail = (self.tail + 1) % cap;
        self.len += 1;
    }

    /// Internal: pop a token from head (assumes len > 0).
    #[inline]
    fn pop_raw(&mut self) -> MessageToken {
        let tok = mem::replace(&mut self.buf[self.head], MessageToken::default());
        let cap = self.physical_capacity();
        self.head = (self.head + 1) % cap;
        self.len -= 1;
        tok
    }

    /// Normalize live items so they are contiguous at buf[0..len] in order.
    /// This is required to safely return borrowed `&mut [MessageToken]` slices.
    fn normalize(&mut self) {
        if self.len == 0 {
            self.head = 0;
            self.tail = 0;
            return;
        }

        let cap = self.physical_capacity();
        if self.head == 0 {
            self.tail = (self.head + self.len) % cap;
            return;
        }

        // Move live items to the beginning in order, replacing old slots with defaults.
        for i in 0..self.len {
            let src_idx = (self.head + i) % cap;
            let tmp = mem::replace(&mut self.buf[src_idx], MessageToken::default());
            self.buf[i] = tmp;
        }

        // Ensure remaining slots are defaulted.
        for i in self.len..cap {
            let _ = mem::replace(&mut self.buf[i], MessageToken::default());
        }

        self.head = 0;
        self.tail = (self.head + self.len) % cap;
    }
}

impl Default for HeapRing {
    fn default() -> Self {
        Self::with_capacity(16)
    }
}

impl Edge for HeapRing {
    fn try_push<H: HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult {
        // Pure admission decision using header metadata.
        let decision = self.get_admission_decision(policy, token, headers);

        // Look up the incoming token's byte size via HeaderStore.
        let item_bytes = headers
            .peek_header(token)
            .map(|h| *h.payload_size_bytes())
            .unwrap_or(0);

        match decision {
            AdmissionDecision::Admit => {
                // Ensure physical capacity (bounded) and logical hard-cap.
                if self.is_full() || policy.caps.at_or_above_hard(self.len(), self.bytes) {
                    return EnqueueResult::Rejected;
                }

                // In unbounded mode, ensure we have backing slots to write into.
                if self.cap.is_none() {
                    self.ensure_capacity_for_push();
                }

                self.bytes = self.bytes.saturating_add(item_bytes);
                self.push_raw(token);
                EnqueueResult::Enqueued
            }

            AdmissionDecision::DropNewest => EnqueueResult::DroppedNewest,

            AdmissionDecision::Reject => EnqueueResult::Rejected,

            AdmissionDecision::Block => {
                // This P1 heap ring cannot block in this design; translate to Rejected.
                EnqueueResult::Rejected
            }

            AdmissionDecision::Evict(n) => {
                // Evict up to n oldest tokens; record last evicted.
                let mut last_evicted = MessageToken::INVALID;
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
                    last_evicted = ev;
                }

                // After eviction, ensure we can accept the item.
                if self.is_full() || policy.caps.at_or_above_hard(self.len(), self.bytes) {
                    return EnqueueResult::Rejected;
                }

                if self.cap.is_none() {
                    self.ensure_capacity_for_push();
                }

                self.bytes = self.bytes.saturating_add(item_bytes);
                self.push_raw(token);

                if last_evicted.is_invalid() {
                    EnqueueResult::Enqueued
                } else {
                    EnqueueResult::Evicted(last_evicted)
                }
            }

            AdmissionDecision::EvictUntilBelowHard => {
                let mut last_evicted = MessageToken::INVALID;
                while policy.caps.at_or_above_hard(self.len(), self.bytes) && self.len > 0 {
                    let ev = self.pop_raw();
                    let ev_bytes = headers
                        .peek_header(ev)
                        .map(|h| *h.payload_size_bytes())
                        .unwrap_or(0);
                    self.bytes = self.bytes.saturating_sub(ev_bytes);
                    last_evicted = ev;
                }

                // If the single item cannot fit even in an empty queue, reject.
                if policy.caps.at_or_above_hard(0, item_bytes) {
                    return EnqueueResult::Rejected;
                }

                if self.is_full() || policy.caps.at_or_above_hard(self.len(), self.bytes) {
                    return EnqueueResult::Rejected;
                }

                if self.cap.is_none() {
                    self.ensure_capacity_for_push();
                }

                self.bytes = self.bytes.saturating_add(item_bytes);
                self.push_raw(token);

                if last_evicted.is_invalid() {
                    EnqueueResult::Enqueued
                } else {
                    EnqueueResult::Evicted(last_evicted)
                }
            }
        }
    }

    fn try_pop<H: HeaderStore>(&mut self, headers: &H) -> Result<MessageToken, QueueError> {
        if self.len == 0 {
            return Err(QueueError::Empty);
        }

        // Peek header before popping to update byte accounting.
        let front_token = self.buf[self.head];
        let front_bytes = headers
            .peek_header(front_token)
            .map(|h| *h.payload_size_bytes())
            .unwrap_or(0);

        let token = self.pop_raw();
        self.bytes = self.bytes.saturating_sub(front_bytes);
        Ok(token)
    }

    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        let watermark = policy.watermark(self.len(), self.bytes);
        EdgeOccupancy::new(self.len(), self.bytes, watermark)
    }

    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        if self.len == 0 {
            return Err(QueueError::Empty);
        }
        Ok(self.buf[self.head])
    }

    fn try_peek_at(&self, index: usize) -> Result<MessageToken, QueueError> {
        if self.len == 0 || index >= self.len {
            return Err(QueueError::Empty);
        }
        let cap = self.physical_capacity();
        let pos = (self.head + index) % cap;
        Ok(self.buf[pos])
    }

    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        policy: &crate::policy::BatchingPolicy,
        headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        use crate::policy::WindowKind;

        if self.len == 0 {
            return Err(QueueError::Empty);
        }

        let bounded = self.cap.is_some();

        // For bounded mode we can normalize and return borrowed slices.
        if bounded {
            // Normalize into buf[0..len] so we can return a borrowed slice.
            self.normalize();
        } else {
            // In unbounded mode ensure backing buffer has slots for addressing.
            if self.buf.is_empty() {
                self.ensure_capacity_for_push();
            }
        }

        let old_len = self.len;
        let cap = self.physical_capacity();

        let fixed_opt = *policy.fixed_n();
        let delta_t_opt = *policy.max_delta_t();
        let window_kind = policy.window_kind();

        // If both caps are absent, treat as fixed_n = 1.
        let effective_fixed: Option<usize> = if fixed_opt.is_none() && delta_t_opt.is_none() {
            Some(1)
        } else {
            fixed_opt
        };

        // Delta-t check via HeaderStore.
        let mut delta_count = self.len;
        if let Some(cap_delta) = delta_t_opt {
            // need to read front token's creation tick via HeaderStore
            if let Ok(front_header) = headers.peek_header(self.buf[self.head]) {
                let front_ticks = *front_header.creation_tick();
                let mut c = 0usize;
                for i in 0..self.len {
                    let pos = (self.head + i) % cap;
                    if let Ok(h) = headers.peek_header(self.buf[pos]) {
                        let tick = *h.creation_tick();
                        let delta = tick.saturating_sub(front_ticks);
                        if delta <= cap_delta {
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

        let apply_fixed = |limit: usize| -> usize {
            if let Some(n) = effective_fixed {
                core::cmp::min(limit, n)
            } else {
                limit
            }
        };

        // --- Disjoint windows
        if let WindowKind::Disjoint = window_kind {
            let take_n = apply_fixed(core::cmp::min(self.len, delta_count));
            if take_n == 0 {
                return Err(QueueError::Empty);
            }

            if bounded {
                // Update byte tracking for popped items.
                let mut dropped_bytes = 0usize;
                for i in 0..take_n {
                    if let Ok(h) = headers.peek_header(self.buf[i]) {
                        dropped_bytes = dropped_bytes.saturating_add(*h.payload_size_bytes());
                    }
                }
                self.bytes = self.bytes.saturating_sub(dropped_bytes);

                // Advance logical head/len/tail.
                let new_head = take_n % cap;
                self.len = old_len - take_n;
                self.head = new_head;
                self.tail = (self.head + self.len) % cap;

                // Return a borrowed slice covering the popped items (zero-copy).
                let slice = &mut self.buf[..take_n];
                return Ok(BatchView::from_borrowed(slice, take_n));
            } else {
                // Unbounded: pop take_n tokens into owned Vec and return.
                let mut out: Vec<MessageToken> = Vec::with_capacity(take_n);
                let mut popped_bytes = 0usize;
                for _ in 0..take_n {
                    if self.len == 0 {
                        break;
                    }
                    let tok = self.pop_raw();
                    if let Ok(h) = headers.peek_header(tok) {
                        popped_bytes = popped_bytes.saturating_add(*h.payload_size_bytes());
                    }
                    out.push(tok);
                }
                self.bytes = self.bytes.saturating_sub(popped_bytes);
                return Ok(BatchView::from_owned(out));
            }
        }

        // --- Sliding windows
        if let WindowKind::Sliding(sw) = window_kind {
            let stride = *sw.stride();
            let size = effective_fixed.unwrap_or(1);

            let mut max_present = core::cmp::min(self.len, size);
            max_present = apply_fixed(core::cmp::min(max_present, delta_count));
            let stride_to_pop = core::cmp::min(stride, self.len);

            if max_present == 0 {
                return Err(QueueError::Empty);
            }

            if bounded {
                // Update byte tracking for popped items.
                let mut popped_bytes = 0usize;
                for i in 0..stride_to_pop {
                    if let Ok(h) = headers.peek_header(self.buf[i]) {
                        popped_bytes = popped_bytes.saturating_add(*h.payload_size_bytes());
                    }
                }
                self.bytes = self.bytes.saturating_sub(popped_bytes);

                // Advance logical head/len/tail.
                let new_head = stride_to_pop % cap;
                self.len = old_len - stride_to_pop;
                self.head = new_head;
                self.tail = (self.head + self.len) % cap;

                let slice = &mut self.buf[..max_present];
                return Ok(BatchView::from_borrowed(slice, max_present));
            } else {
                // Unbounded: pop stride_to_pop tokens into out (these are removed),
                // then copy `need_more` tokens from the new front for presentation.
                let mut out: Vec<MessageToken> = Vec::with_capacity(max_present);
                let mut popped_bytes = 0usize;

                // Pop stride_to_pop tokens.
                for _ in 0..stride_to_pop {
                    if self.len == 0 {
                        break;
                    }
                    let tok = self.pop_raw();
                    if let Ok(h) = headers.peek_header(tok) {
                        popped_bytes = popped_bytes.saturating_add(*h.payload_size_bytes());
                    }
                    out.push(tok);
                }

                // After popping, we need to include more tokens (peek) to reach max_present.
                let need_more = max_present.saturating_sub(out.len());
                if need_more > 0 {
                    let cap_after = self.physical_capacity();
                    for i in 0..need_more {
                        if i >= self.len {
                            break;
                        }
                        let pos = (self.head + i) % cap_after;
                        // MessageToken is small and copyable — copy the token for owned presentation.
                        out.push(self.buf[pos]);
                    }
                }

                self.bytes = self.bytes.saturating_sub(popped_bytes);
                return Ok(BatchView::from_owned(out));
            }
        }

        // --- Default (non-sliding, non-disjoint)
        let mut take_n = core::cmp::min(self.len, delta_count);
        take_n = apply_fixed(take_n);
        if take_n == 0 {
            return Err(QueueError::Empty);
        }

        if bounded {
            let mut dropped_bytes = 0usize;
            for i in 0..take_n {
                if let Ok(h) = headers.peek_header(self.buf[i]) {
                    dropped_bytes = dropped_bytes.saturating_add(*h.payload_size_bytes());
                }
            }
            self.bytes = self.bytes.saturating_sub(dropped_bytes);

            let new_head = take_n % cap;
            self.len = old_len - take_n;
            self.head = new_head;
            self.tail = (self.head + self.len) % cap;

            let slice = &mut self.buf[..take_n];
            return Ok(BatchView::from_borrowed(slice, take_n));
        }

        // Unbounded (owned) default path: pop `take_n` tokens into owned Vec and return.
        let mut out: Vec<MessageToken> = Vec::with_capacity(take_n);
        let mut popped_bytes = 0usize;
        for _ in 0..take_n {
            if self.len == 0 {
                break;
            }
            let tok = self.pop_raw();
            if let Ok(h) = headers.peek_header(tok) {
                popped_bytes = popped_bytes.saturating_add(*h.payload_size_bytes());
            }
            out.push(tok);
        }
        self.bytes = self.bytes.saturating_sub(popped_bytes);
        Ok(BatchView::from_owned(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::run_edge_contract_tests!(heap_ring_contract_bounded, || {
        HeapRing::with_capacity(16)
    });

    crate::run_edge_contract_tests!(heap_ring_contract_unbounded, || { HeapRing::unbounded() });
}
