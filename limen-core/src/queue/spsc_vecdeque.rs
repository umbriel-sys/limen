//! Heap-backed SPSC ring buffer for P1 (no_std + alloc), **safe version**.
//!
//! Uses VecDeque as the backing ring; we enforce a fixed logical capacity and
//! keep byte occupancy accounting for admission / watermark decisions.

extern crate alloc;

use alloc::collections::VecDeque;

use crate::errors::QueueError;
use crate::message::{payload::Payload, Message};
use crate::policy::{AdmissionPolicy, EdgePolicy, WatermarkState};
use crate::queue::{EnqueueResult, QueueOccupancy, SpscQueue};

/// Heap ring with fixed item capacity.
pub struct HeapRing<T> {
    buf: VecDeque<T>,
    cap: usize,
    bytes: usize,
}

impl<T> HeapRing<T> {
    /// Create a new ring with the given fixed capacity in items.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap),
            cap,
            bytes: 0,
        }
    }

    #[inline]
    fn len(&self) -> usize {
        self.buf.len()
    }
    #[inline]
    fn is_full(&self) -> bool {
        self.len() >= self.cap
    }
}

impl<P: Payload + Clone> SpscQueue for HeapRing<Message<P>> {
    type Item = Message<P>;

    fn try_push(&mut self, item: Self::Item, policy: &EdgePolicy) -> EnqueueResult {
        let items = self.len();
        let bytes = self.bytes;

        // Hard cap + full => reject
        if policy.caps.at_or_above_hard(items, bytes) && self.is_full() {
            return EnqueueResult::Rejected;
        }

        // Between soft & hard: DropOldest eviction
        match policy.watermark(items, bytes) {
            WatermarkState::BetweenSoftAndHard
                if matches!(policy.admission, AdmissionPolicy::DropOldest)
                    && !self.buf.is_empty() =>
            {
                if let Some(ev) = self.buf.pop_front() {
                    self.bytes = self.bytes.saturating_sub(ev.header.payload_size_bytes);
                }
            }
            _ => {}
        }

        if self.is_full() {
            return EnqueueResult::Rejected;
        }

        self.bytes = self.bytes.saturating_add(item.header.payload_size_bytes);
        self.buf.push_back(item);
        EnqueueResult::Enqueued
    }

    fn try_pop(&mut self) -> Result<Self::Item, QueueError> {
        match self.buf.pop_front() {
            Some(item) => {
                self.bytes = self.bytes.saturating_sub(item.header.payload_size_bytes);
                Ok(item)
            }
            None => Err(QueueError::Empty),
        }
    }

    fn occupancy(&self, policy: &EdgePolicy) -> QueueOccupancy {
        let items = self.len();
        let bytes = self.bytes;
        let watermark = policy.watermark(items, bytes);
        QueueOccupancy {
            items,
            bytes,
            watermark,
        }
    }

    fn try_peek(&self) -> Result<&Self::Item, QueueError> {
        self.buf.front().ok_or(QueueError::Empty)
    }
}
