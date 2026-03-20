//! SpscRingbuf: std-friendly SPSC ring buffer (P2 default).
//!
//! This implementation uses a heap-allocated `Vec<MessageToken>` ring with an
//! optional physical capacity. In bounded mode (`cap = Some(n)`) the backing
//! buffer is fixed and `try_pop_batch` returns borrowed slices (zero-copy).
//! In unbounded mode (`cap = None`) the backing Vec grows and batches are
//! returned as owned `Vec<MessageToken>`.

use alloc::vec::Vec;
use core::mem;

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::QueueError;
use crate::policy::{AdmissionDecision, EdgePolicy, WindowKind};
use crate::prelude::{BatchView, HeaderStore};
use crate::types::MessageToken;

/// SPSC ring buffer storing `MessageToken`s. `cap` is `Some(n)` for bounded mode,
/// or `None` for unbounded mode.
pub struct SpscRingbuf {
    buf: Vec<MessageToken>,
    head: usize,
    tail: usize,
    len: usize,
    cap: Option<usize>,
    bytes: usize,
}

impl SpscRingbuf {
    /// Create a bounded ring with the given fixed item capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let mut v = Vec::with_capacity(capacity);
        v.resize_with(capacity, MessageToken::default);
        Self {
            buf: v,
            head: 0,
            tail: 0,
            len: 0,
            cap: Some(capacity),
            bytes: 0,
        }
    }

    /// Create an unbounded ring. Backing buffer grows on demand; batch ops
    /// return owned batches in this mode.
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

    #[inline]
    fn len(&self) -> usize {
        self.len
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    fn is_full(&self) -> bool {
        match self.cap {
            Some(c) => self.len >= c,
            None => false,
        }
    }

    #[inline]
    fn physical_capacity(&self) -> usize {
        match self.cap {
            Some(c) => c,
            None => self.buf.len(),
        }
    }

    fn ensure_capacity_for_push(&mut self) {
        if self.cap.is_some() {
            return;
        }

        if self.buf.is_empty() {
            let start = 4usize;
            self.buf.resize_with(start, MessageToken::default);
            self.head = 0;
            self.tail = 0;
            return;
        }

        if self.len >= self.buf.len() {
            let new_cap = core::cmp::max(4, self.buf.len() * 2);
            self.buf.resize_with(new_cap, MessageToken::default);
        }
    }

    #[inline]
    fn push_raw(&mut self, token: MessageToken) {
        if self.cap.is_none() {
            self.ensure_capacity_for_push();
        }
        let cap = self.physical_capacity();
        self.buf[self.tail] = token;
        self.tail = (self.tail + 1) % cap;
        self.len += 1;
    }

    #[inline]
    fn pop_raw(&mut self) -> MessageToken {
        let tok = mem::replace(&mut self.buf[self.head], MessageToken::default());
        let cap = self.physical_capacity();
        self.head = (self.head + 1) % cap;
        self.len -= 1;
        tok
    }

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
        for i in 0..self.len {
            let src_idx = (self.head + i) % cap;
            let tmp = mem::replace(&mut self.buf[src_idx], MessageToken::default());
            self.buf[i] = tmp;
        }
        for i in self.len..cap {
            let _ = mem::replace(&mut self.buf[i], MessageToken::default());
        }
        self.head = 0;
        self.tail = (self.head + self.len) % cap;
    }
}

impl Default for SpscRingbuf {
    fn default() -> Self {
        Self::with_capacity(16)
    }
}

impl Edge for SpscRingbuf {
    fn try_push<H: HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult {
        let decision = self.get_admission_decision(policy, token, headers);

        let item_bytes = headers
            .peek_header(token)
            .map(|h| *h.payload_size_bytes())
            .unwrap_or(0);

        match decision {
            AdmissionDecision::Admit => {
                if self.is_full() || policy.caps.at_or_above_hard(self.len(), self.bytes) {
                    return EnqueueResult::Rejected;
                }

                if self.cap.is_none() {
                    self.ensure_capacity_for_push();
                }

                self.bytes = self.bytes.saturating_add(item_bytes);
                self.push_raw(token);
                EnqueueResult::Enqueued
            }

            AdmissionDecision::DropNewest => EnqueueResult::DroppedNewest,

            AdmissionDecision::Reject => EnqueueResult::Rejected,

            AdmissionDecision::Block => EnqueueResult::Rejected,

            AdmissionDecision::Evict(n) => {
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
        if self.len == 0 {
            return Err(QueueError::Empty);
        }

        let bounded = self.cap.is_some();

        if bounded {
            self.normalize();
        } else if self.buf.is_empty() {
            self.ensure_capacity_for_push();
        }

        let old_len = self.len;
        let cap = self.physical_capacity();

        let fixed_opt = *policy.fixed_n();
        let delta_t_opt = *policy.max_delta_t();
        let window_kind = policy.window_kind();

        let effective_fixed: Option<usize> = if fixed_opt.is_none() && delta_t_opt.is_none() {
            Some(1)
        } else {
            fixed_opt
        };

        let mut delta_count = self.len;
        if let Some(cap_delta) = delta_t_opt {
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
            } else {
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
                let mut popped_bytes = 0usize;
                for i in 0..stride_to_pop {
                    if let Ok(h) = headers.peek_header(self.buf[i]) {
                        popped_bytes = popped_bytes.saturating_add(*h.payload_size_bytes());
                    }
                }
                self.bytes = self.bytes.saturating_sub(popped_bytes);

                let new_head = stride_to_pop % cap;
                self.len = old_len - stride_to_pop;
                self.head = new_head;
                self.tail = (self.head + self.len) % cap;

                let slice = &mut self.buf[..max_present];
                return Ok(BatchView::from_borrowed(slice, max_present));
            } else {
                let mut out: Vec<MessageToken> = Vec::with_capacity(max_present);
                let mut popped_bytes = 0usize;

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

                let need_more = max_present.saturating_sub(out.len());
                if need_more > 0 {
                    let cap_after = self.physical_capacity();
                    for i in 0..need_more {
                        if i >= self.len {
                            break;
                        }
                        let pos = (self.head + i) % cap_after;
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

    crate::run_edge_contract_tests!(spsc_ringbuf_std_bounded, || {
        SpscRingbuf::with_capacity(16)
    });

    crate::run_edge_contract_tests!(spsc_ringbuf_std_unbounded, || { SpscRingbuf::unbounded() });
}
