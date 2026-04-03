//! Thread-safe ring buffer implementing [`Edge`], backed by `Arc<Mutex<…>>`.
//!
//! `ConcurrentEdge` is the `std`-feature drop-in for `StaticRing<N>` in graphs
//! executed by `run_scoped()` on multiple threads. Cloning yields another handle
//! to the **same** underlying ring — all clones share one `Arc<Mutex<…>>`.
//!
//! # Locking discipline
//! Every method locks `inner` exactly once, does all work on the locked data, and
//! releases the lock before returning. No method calls another `Edge` method while
//! holding the lock — this prevents deadlocks. `try_pop_batch` materialises its
//! result tokens into an owned `Vec` before releasing, so no borrowed reference
//! escapes the critical section.

use alloc::vec::Vec;
use std::sync::{Arc, Mutex};

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::QueueError;
use crate::message::batch::BatchView;
use crate::policy::{AdmissionDecision, BatchingPolicy, EdgePolicy, WindowKind};
use crate::prelude::HeaderStore;
use crate::types::MessageToken;

// ---------------------------------------------------------------------------
// Inner state
// ---------------------------------------------------------------------------

struct ConcurrentEdgeInner {
    buf: Vec<MessageToken>,
    head: usize,
    tail: usize,
    len: usize,
    capacity: usize,
    bytes: usize,
}

impl ConcurrentEdgeInner {
    fn new(capacity: usize) -> Self {
        Self {
            buf: alloc::vec![MessageToken::default(); capacity],
            head: 0,
            tail: 0,
            len: 0,
            capacity,
            bytes: 0,
        }
    }

    #[inline]
    fn is_full(&self) -> bool {
        self.len == self.capacity
    }

    #[inline]
    fn push_raw(&mut self, token: MessageToken) {
        self.buf[self.tail] = token;
        self.tail = (self.tail + 1) % self.capacity;
        self.len += 1;
    }

    #[inline]
    fn pop_raw(&mut self) -> MessageToken {
        let token = core::mem::take(&mut self.buf[self.head]);
        self.head = (self.head + 1) % self.capacity;
        self.len -= 1;
        token
    }

    /// Rotate so live items are contiguous at `buf[0..len]`.
    fn normalize(&mut self) {
        if self.len == 0 {
            self.head = 0;
            self.tail = 0;
            return;
        }

        if self.head == 0 {
            self.tail = self.len % self.capacity;
            return;
        }

        let cap = self.capacity;
        let mut live_tokens = Vec::with_capacity(self.len);

        for i in 0..self.len {
            let src = (self.head + i) % cap;
            live_tokens.push(core::mem::take(&mut self.buf[src]));
        }

        self.buf[..self.len].copy_from_slice(&live_tokens[..self.len]);

        for i in self.len..cap {
            self.buf[i] = MessageToken::default();
        }

        self.head = 0;
        self.tail = self.len % cap;
    }
}

// ---------------------------------------------------------------------------
// Public handle
// ---------------------------------------------------------------------------

/// A thread-safe edge handle. All clones share the same underlying ring buffer.
///
/// Use `ConcurrentEdge::new(capacity)` to create; clone for each worker that
/// needs read or write access. Intended for use in codegen-generated `run_scoped()`
/// methods as the `std`-feature replacement for `StaticRing<N>`.
#[derive(Clone)]
pub struct ConcurrentEdge {
    inner: Arc<Mutex<ConcurrentEdgeInner>>,
}

impl ConcurrentEdge {
    /// Create a new ring buffer with the given item capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ConcurrentEdgeInner::new(capacity))),
        }
    }
}

// ---------------------------------------------------------------------------
// Edge impl
// ---------------------------------------------------------------------------

impl Edge for ConcurrentEdge {
    fn try_push<H: HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return EnqueueResult::Rejected,
        };

        // Inline get_admission_decision — do NOT call self.get_admission_decision()
        // as that calls self.occupancy() which would re-lock and deadlock.
        let (decision, item_bytes) = match headers.peek_header(token) {
            Ok(h) => {
                let b = *h.payload_size_bytes();
                let d = policy.decide(inner.len, inner.bytes, b, *h.deadline_ns(), *h.qos());
                (d, b)
            }
            Err(_) => return EnqueueResult::Rejected,
        };

        match decision {
            AdmissionDecision::Admit => {
                if inner.is_full() || policy.caps.at_or_above_hard(inner.len, inner.bytes) {
                    return EnqueueResult::Rejected;
                }
                inner.bytes = inner.bytes.saturating_add(item_bytes);
                inner.push_raw(token);
                EnqueueResult::Enqueued
            }
            AdmissionDecision::DropNewest => EnqueueResult::DroppedNewest,
            AdmissionDecision::Reject | AdmissionDecision::Block => EnqueueResult::Rejected,
            AdmissionDecision::Evict(_) | AdmissionDecision::EvictUntilBelowHard => {
                if inner.is_full() || policy.caps.at_or_above_hard(inner.len, inner.bytes) {
                    return EnqueueResult::Rejected;
                }
                inner.bytes = inner.bytes.saturating_add(item_bytes);
                inner.push_raw(token);
                EnqueueResult::Enqueued
            }
        }
    }

    fn try_pop<H: HeaderStore>(&mut self, headers: &H) -> Result<MessageToken, QueueError> {
        let mut inner = self.inner.lock().map_err(|_| QueueError::Poisoned)?;
        if inner.len == 0 {
            return Err(QueueError::Empty);
        }
        let front_token = inner.buf[inner.head];
        let front_bytes = headers
            .peek_header(front_token)
            .map(|h| *h.payload_size_bytes())
            .unwrap_or(0);
        let token = inner.pop_raw();
        inner.bytes = inner.bytes.saturating_sub(front_bytes);
        Ok(token)
    }

    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        match self.inner.lock() {
            Ok(inner) => {
                let watermark = policy.watermark(inner.len, inner.bytes);
                EdgeOccupancy::new(inner.len, inner.bytes, watermark)
            }
            Err(_) => EdgeOccupancy::new(0, 0, policy.watermark(0, 0)),
        }
    }

    fn is_empty(&self) -> bool {
        self.inner.lock().map(|g| g.len == 0).unwrap_or(true)
    }

    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        let inner = self.inner.lock().map_err(|_| QueueError::Poisoned)?;
        if inner.len == 0 {
            return Err(QueueError::Empty);
        }
        Ok(inner.buf[inner.head])
    }

    fn try_peek_at(&self, index: usize) -> Result<MessageToken, QueueError> {
        let inner = self.inner.lock().map_err(|_| QueueError::Poisoned)?;
        if inner.len == 0 || index >= inner.len {
            return Err(QueueError::Empty);
        }
        let pos = (inner.head + index) % inner.capacity;
        Ok(inner.buf[pos])
    }

    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        policy: &BatchingPolicy,
        headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        let mut inner = self.inner.lock().map_err(|_| QueueError::Poisoned)?;

        if inner.len == 0 {
            return Err(QueueError::Empty);
        }

        inner.normalize();
        let old_len = inner.len;

        let fixed_opt = *policy.fixed_n();
        let delta_t_opt = *policy.max_delta_t();
        let window_kind = policy.window_kind();

        let effective_fixed: Option<usize> = if fixed_opt.is_none() && delta_t_opt.is_none() {
            Some(1)
        } else {
            fixed_opt
        };

        // Delta-t window: count how many consecutive items fall within cap ticks.
        let mut delta_count = inner.len;
        if let Some(cap) = delta_t_opt {
            if let Ok(front_header) = headers.peek_header(inner.buf[0]) {
                let front_ticks = *front_header.creation_tick();
                let mut c = 0usize;
                while c < inner.len {
                    if let Ok(h) = headers.peek_header(inner.buf[c]) {
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

        // --- Disjoint window
        if let WindowKind::Disjoint = window_kind {
            let take_n = apply_fixed(core::cmp::min(inner.len, delta_count));
            if take_n == 0 {
                return Err(QueueError::Empty);
            }
            let mut dropped_bytes = 0usize;
            for i in 0..take_n {
                if let Ok(h) = headers.peek_header(inner.buf[i]) {
                    dropped_bytes = dropped_bytes.saturating_add(*h.payload_size_bytes());
                }
            }
            inner.bytes = inner.bytes.saturating_sub(dropped_bytes);
            inner.len = old_len - take_n;
            inner.head = take_n % inner.capacity;
            inner.tail = (inner.head + inner.len) % inner.capacity;

            let mut owned = Vec::with_capacity(take_n);
            for &tok in &inner.buf[..take_n] {
                owned.push(tok);
            }
            return Ok(BatchView::from_owned(owned));
        }

        // --- Sliding window
        if let WindowKind::Sliding(sw) = window_kind {
            let stride = *sw.stride();
            let size = effective_fixed.unwrap_or(1);
            let max_present =
                apply_fixed(core::cmp::min(core::cmp::min(inner.len, size), delta_count));
            let stride_to_pop = core::cmp::min(stride, inner.len);

            if max_present == 0 {
                return Err(QueueError::Empty);
            }

            let mut popped_bytes = 0usize;
            for i in 0..stride_to_pop {
                if let Ok(h) = headers.peek_header(inner.buf[i]) {
                    popped_bytes = popped_bytes.saturating_add(*h.payload_size_bytes());
                }
            }
            inner.bytes = inner.bytes.saturating_sub(popped_bytes);
            inner.len = old_len - stride_to_pop;
            inner.head = stride_to_pop % inner.capacity;
            inner.tail = (inner.head + inner.len) % inner.capacity;

            let mut owned = Vec::with_capacity(max_present);
            for &tok in &inner.buf[..max_present] {
                owned.push(tok);
            }
            return Ok(BatchView::from_owned(owned));
        }

        // --- Default (future WindowKind variants or non-exhaustive fallback)
        let take_n = apply_fixed(core::cmp::min(inner.len, delta_count));
        if take_n == 0 {
            return Err(QueueError::Empty);
        }
        let mut dropped_bytes = 0usize;
        for i in 0..take_n {
            if let Ok(h) = headers.peek_header(inner.buf[i]) {
                dropped_bytes = dropped_bytes.saturating_add(*h.payload_size_bytes());
            }
        }
        inner.bytes = inner.bytes.saturating_sub(dropped_bytes);
        inner.len = old_len - take_n;
        inner.head = take_n % inner.capacity;
        inner.tail = (inner.head + inner.len) % inner.capacity;

        let mut owned = Vec::with_capacity(take_n);
        for &tok in &inner.buf[..take_n] {
            owned.push(tok);
        }
        Ok(BatchView::from_owned(owned))
    }
}

#[cfg(feature = "std")]
impl crate::edge::ScopedEdge for ConcurrentEdge {
    type Handle<'a>
        = ConcurrentEdge
    where
        Self: 'a;

    fn scoped_handle<'a>(&'a self, _kind: crate::edge::EdgeHandleKind) -> Self::Handle<'a>
    where
        Self: 'a,
    {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::run_edge_contract_tests!(concurrent_edge_contract, || ConcurrentEdge::new(16));
}
