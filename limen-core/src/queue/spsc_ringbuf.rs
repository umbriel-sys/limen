//! SpscRingbuf: safe wrapper over the `ringbuf` crate (P2 default).
//!
//! Feature gates: `std` and `queue_ringbuf` (enables optional `ringbuf` dep).

use ringbuf::traits::{
    consumer::Consumer as _, observer::Observer as _, producer::Producer as _, Split as _,
};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use crate::errors::QueueError;
use crate::message::{payload::Payload, Message};
use crate::policy::{AdmissionPolicy, EdgePolicy, WatermarkState};
use crate::queue::{EnqueueResult, QueueOccupancy, SpscQueue};

/// A single-producer single-consumer (SPSC) queue backed by the [`ringbuf`] crate.
///
/// This implementation wraps [`HeapRb`] to provide a safe, bounded ring buffer with
/// additional accounting for item count and payload size in bytes. It enforces
/// capacity constraints and admission policies defined by [`EdgePolicy`].
///
/// Intended as the default SPSC queue for Limen (P2).
pub struct SpscRingbuf<T> {
    prod: HeapProd<T>,
    cons: HeapCons<T>,
    cap: usize,
    bytes: usize,
}

impl<T> SpscRingbuf<T> {
    /// Create with capacity (items).
    /// The underlying `HeapRb` does not require power-of-two,
    /// but we keep `next_power_of_two()` to align with other queues.
    pub fn with_capacity(capacity: usize) -> Self {
        let rb = HeapRb::<T>::new(capacity.next_power_of_two());
        let (prod, cons) = rb.split();
        Self {
            prod,
            cons,
            cap: capacity,
            bytes: 0,
        }
    }

    #[inline]
    fn len_internal(&self) -> usize {
        // Number of elements available to consume
        self.cons.occupied_len()
    }

    #[inline]
    fn is_full(&self) -> bool {
        // Logical full if we've hit configured cap or producer reports full.
        self.len_internal() >= self.cap || self.prod.is_full()
    }
}

impl<P: Payload + std::clone::Clone> SpscQueue for SpscRingbuf<Message<P>> {
    type Item = Message<P>;

    fn try_push(&mut self, item: Self::Item, policy: &EdgePolicy) -> EnqueueResult {
        let items = self.len_internal();
        let bytes = self.bytes;

        if policy.caps.at_or_above_hard(items, bytes) && self.is_full() {
            return EnqueueResult::Rejected;
        }

        match policy.watermark(items, bytes) {
            WatermarkState::BetweenSoftAndHard
                if matches!(policy.admission, AdmissionPolicy::DropOldest)
                    && self.len_internal() > 0 =>
            {
                if let Some(ev) = self.cons.try_pop() {
                    self.bytes = self.bytes.saturating_sub(ev.header.payload_size_bytes);
                }
            }
            _ => {}
        }

        if self.is_full() {
            return EnqueueResult::Rejected;
        }

        let payload_bytes = item.header.payload_size_bytes;
        if let Err(_item_back) = self.prod.try_push(item) {
            return EnqueueResult::Rejected;
        }
        self.bytes = self.bytes.saturating_add(payload_bytes);
        EnqueueResult::Enqueued
    }

    fn try_pop(&mut self) -> Result<Self::Item, QueueError> {
        match self.cons.try_pop() {
            Some(item) => {
                self.bytes = self.bytes.saturating_sub(item.header.payload_size_bytes);
                Ok(item)
            }
            None => Err(QueueError::Empty),
        }
    }

    fn occupancy(&self, policy: &EdgePolicy) -> QueueOccupancy {
        let items = self.len_internal();
        let bytes = self.bytes;
        let watermark = policy.watermark(items, bytes);
        QueueOccupancy {
            items,
            bytes,
            watermark,
        }
    }

    fn try_peek(&self) -> Result<&Self::Item, QueueError> {
        self.cons.try_peek().ok_or(QueueError::Empty)
    }

    /// Clone the front item without removing it.
    #[inline]
    fn try_peek_cloned(&self) -> Result<Message<P>, QueueError> {
        match self.try_peek() {
            Ok(m) => Ok(m.clone()),
            Err(e) => Err(e),
        }
    }
}
