//! Static SPSC ring buffer for P0 (no_std + no_alloc), **safe version**.
//!
//! Stores `MessageToken` values directly in a fixed-size `[MessageToken; N]`
//! ring. Header metadata is accessed via `HeaderStore` for admission and
//! batching decisions.

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::QueueError;
use crate::policy::{AdmissionDecision, EdgePolicy};
use crate::prelude::{BatchView, HeaderStore};
use crate::types::MessageToken;

use core::mem;

/// A fixed-capacity ring buffer storing [`MessageToken`] values.
pub struct StaticRing<const N: usize> {
    buf: [MessageToken; N],
    head: usize,
    tail: usize,
    len: usize,
    /// Running byte total, updated on push/pop via HeaderStore.
    bytes: usize,
}

impl<const N: usize> StaticRing<N> {
    /// Create a new empty ring.
    #[inline]
    pub fn new() -> Self {
        Self {
            buf: [MessageToken::default(); N],
            head: 0,
            tail: 0,
            len: 0,
            bytes: 0,
        }
    }

    #[inline]
    fn is_full(&self) -> bool {
        self.len == N
    }

    #[inline]
    fn push_raw(&mut self, token: MessageToken) {
        self.buf[self.tail] = token;
        self.tail = (self.tail + 1) % N;
        self.len += 1;
    }

    #[inline]
    fn pop_raw(&mut self) -> MessageToken {
        let token = mem::take(&mut self.buf[self.head]);
        self.head = (self.head + 1) % N;
        self.len -= 1;
        token
    }

    /// Normalize so live items are contiguous at buf[0..len].
    fn normalize(&mut self) {
        if self.len == 0 {
            self.head = 0;
            self.tail = 0;
            return;
        }
        if self.head == 0 {
            self.tail = (self.head + self.len) % N;
            return;
        }
        for i in 0..self.len {
            let src_idx = (self.head + i) % N;
            let tmp = mem::take(&mut self.buf[src_idx]);
            self.buf[i] = tmp;
        }
        for i in self.len..N {
            self.buf[i] = MessageToken::default();
        }
        self.head = 0;
        self.tail = (self.head + self.len) % N;
    }
}

impl<const N: usize> Default for StaticRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Edge for StaticRing<N> {
    fn try_push<H: HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult {
        let decision = self.get_admission_decision(policy, token, headers);

        // Look up the incoming token's byte size via HeaderStore.
        let item_bytes = headers
            .peek_header(token)
            .map(|h| *h.payload_size_bytes())
            .unwrap_or(0);

        match decision {
            AdmissionDecision::Admit => {
                if self.is_full() || policy.caps.at_or_above_hard(self.len, self.bytes) {
                    return EnqueueResult::Rejected;
                }
                self.bytes = self.bytes.saturating_add(item_bytes);
                self.push_raw(token);
                EnqueueResult::Enqueued
            }

            AdmissionDecision::DropNewest => EnqueueResult::DroppedNewest,

            AdmissionDecision::Reject => EnqueueResult::Rejected,

            AdmissionDecision::Block => EnqueueResult::Rejected,

            AdmissionDecision::Evict(n) => {
                // Evict up to n oldest tokens. Return the LAST evicted token
                // to the caller for freeing. Earlier evictions are also
                // returned via successive Evicted results if needed, but for
                // simplicity we collect them here and return the last one.
                // StepContext handles freeing.
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

                if policy.caps.at_or_above_hard(self.len, self.bytes) || self.is_full() {
                    return EnqueueResult::Rejected;
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
                while policy.caps.at_or_above_hard(self.len, self.bytes) && self.len > 0 {
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
                if self.is_full() || policy.caps.at_or_above_hard(self.len, self.bytes) {
                    return EnqueueResult::Rejected;
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
        // Peek header before popping to update byte tracking.
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
        let watermark = policy.watermark(self.len, self.bytes);
        EdgeOccupancy::new(self.len, self.bytes, watermark)
    }

    fn is_empty(&self) -> bool {
        self.len == 0
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
        let pos = (self.head + index) % N;
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

        self.normalize();
        let old_len = self.len;

        let fixed_opt = *policy.fixed_n();
        let delta_t_opt = *policy.max_delta_t();
        let window_kind = policy.window_kind();

        let effective_fixed: Option<usize> = if fixed_opt.is_none() && delta_t_opt.is_none() {
            Some(1)
        } else {
            fixed_opt
        };

        // Delta-t check via HeaderStore.
        let mut delta_count = self.len;
        if let Some(cap) = delta_t_opt {
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

            // Update byte tracking for popped items.
            let mut dropped_bytes = 0usize;
            for i in 0..take_n {
                if let Ok(h) = headers.peek_header(self.buf[i]) {
                    dropped_bytes = dropped_bytes.saturating_add(*h.payload_size_bytes());
                }
            }
            self.bytes = self.bytes.saturating_sub(dropped_bytes);

            let new_head = take_n % N;
            self.len = old_len - take_n;
            self.head = new_head;
            self.tail = (self.head + self.len) % N;

            let slice = &mut self.buf[..take_n];
            return Ok(BatchView::from_borrowed(slice, take_n));
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

            let mut popped_bytes = 0usize;
            for i in 0..stride_to_pop {
                if let Ok(h) = headers.peek_header(self.buf[i]) {
                    popped_bytes = popped_bytes.saturating_add(*h.payload_size_bytes());
                }
            }
            self.bytes = self.bytes.saturating_sub(popped_bytes);

            let new_head = stride_to_pop % N;
            self.len = old_len - stride_to_pop;
            self.head = new_head;
            self.tail = (self.head + self.len) % N;

            let slice = &mut self.buf[..max_present];
            return Ok(BatchView::from_borrowed(slice, max_present));
        }

        // --- Default (non-sliding, non-disjoint)
        let mut take_n = core::cmp::min(self.len, delta_count);
        take_n = apply_fixed(take_n);
        if take_n == 0 {
            return Err(QueueError::Empty);
        }

        let mut dropped_bytes = 0usize;
        for i in 0..take_n {
            if let Ok(h) = headers.peek_header(self.buf[i]) {
                dropped_bytes = dropped_bytes.saturating_add(*h.payload_size_bytes());
            }
        }
        self.bytes = self.bytes.saturating_sub(dropped_bytes);

        let new_head = take_n % N;
        self.len = old_len - take_n;
        self.head = new_head;
        self.tail = (self.head + self.len) % N;

        let slice = &mut self.buf[..take_n];
        Ok(BatchView::from_borrowed(slice, take_n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Runs the full Edge contract suite against StaticRing<Message<u32>, 16>.
    crate::run_edge_contract_tests!(static_ring_contract, || { StaticRing::<16>::new() });
}
