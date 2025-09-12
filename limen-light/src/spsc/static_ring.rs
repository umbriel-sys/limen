//! Static SPSC ring buffer for P0 (no_std + no_alloc), **safe version**.
//!
//! Uses a fixed-size [Option<T>; N] ring. No unsafe; no heap.
//! Tracks both item and byte occupancy; supports DropOldest between watermarks.

use limen_core::errors::QueueError;
use limen_core::message::{Message, Payload};
use limen_core::policy::{AdmissionPolicy, EdgePolicy, WatermarkState};
use limen_core::queue::{EnqueueResult, QueueOccupancy, SpscQueue};

/// A fixed-capacity ring buffer for single-producer/single-consumer usage.
pub struct StaticRing<T, const N: usize> {
    buf: [Option<T>; N],
    head: usize,
    tail: usize,
    len: usize,
    bytes: usize,
}

impl<T, const N: usize> StaticRing<T, N> {
    /// Create a new empty ring.
    pub fn new() -> Self {
        Self {
            // Build [None; N] without requiring T: Copy
            buf: core::array::from_fn(|_| None),
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
    fn push_raw(&mut self, item: T) {
        debug_assert!(self.buf[self.tail].is_none());
        self.buf[self.tail] = Some(item);
        self.tail = (self.tail + 1) % N;
        self.len += 1;
    }

    #[inline]
    fn pop_raw(&mut self) -> T {
        let h = self.head;
        let item = self.buf[h].take().expect("pop_raw called with len > 0");
        self.head = (self.head + 1) % N;
        self.len -= 1;
        item
    }
}

impl<P: Payload, const N: usize> SpscQueue for StaticRing<Message<P>, N> {
    type Item = Message<P>;

    fn try_push(&mut self, item: Self::Item, policy: &EdgePolicy) -> EnqueueResult {
        let items = self.len;
        let bytes = self.bytes;

        // Hard cap + full => reject
        if policy.caps.at_or_above_hard(items, bytes) && self.is_full() {
            return EnqueueResult::Rejected;
        }

        // Between soft & hard: DropOldest eviction
        match policy.watermark(items, bytes) {
            WatermarkState::BetweenSoftAndHard
                if matches!(policy.admission, AdmissionPolicy::DropOldest) && self.len > 0 =>
            {
                let evicted = self.pop_raw();
                self.bytes = self.bytes.saturating_sub(evicted.header.payload_size_bytes);
            }
            _ => {}
        }

        if self.is_full() {
            return EnqueueResult::Rejected;
        }

        self.bytes = self.bytes.saturating_add(item.header.payload_size_bytes);
        self.push_raw(item);
        EnqueueResult::Enqueued
    }

    fn try_pop(&mut self) -> Result<Self::Item, QueueError> {
        if self.len == 0 {
            return Err(QueueError::Empty);
        }
        let item = self.pop_raw();
        self.bytes = self.bytes.saturating_sub(item.header.payload_size_bytes);
        Ok(item)
    }

    fn occupancy(&self, policy: &EdgePolicy) -> QueueOccupancy {
        let items = self.len;
        let bytes = self.bytes;
        let watermark = policy.watermark(items, bytes);
        QueueOccupancy {
            items,
            bytes,
            watermark,
        }
    }

    fn try_peek(&self) -> Result<&Self::Item, QueueError> {
        if self.len == 0 {
            return Err(QueueError::Empty);
        }
        self.buf[self.head].as_ref().ok_or(QueueError::Empty)
    }
}
