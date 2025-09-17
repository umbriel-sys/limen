//! Concurrent ring buffer queue (SPSC semantics) built on the [`ringbuf`] crate.
//!
//! This implementation wraps [`HeapRb`] and exposes it through the [`SpscQueue`] trait
//! with `Arc<Mutex<...>>` guards on the producer, consumer, and byte counters.
//!
//! It is intended for **P2 concurrent runtimes** where multiple threads may push, pop,
//! and inspect queue occupancy safely.  Lock poisoning is reported as
//! [`QueueError::Poisoned`] instead of panicking.
//!
//! Notes:
//! - `try_peek` cannot return a borrowed reference across a `Mutex` guard and will
//!   always return [`QueueError::Empty`].
//! - Use [`SpscQueue::try_peek_cloned`] (enabled under the `std` feature) to clone the
//!   front item without removing it.
//! - For single-threaded P0/P1 runtimes, prefer [`SpscRingbuf`] for zero-lock overhead.
//!
//! This module is feature-gated behind `std` and `queue_ringbuf`.

use core::fmt;
use std::sync::{Arc, Mutex};

use ringbuf::traits::{
    consumer::Consumer as _, observer::Observer as _, producer::Producer as _, Split as _,
};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use crate::errors::QueueError;
use crate::message::{payload::Payload, Message};
use crate::policy::{AdmissionPolicy, EdgePolicy, WatermarkState};
use crate::queue::{EnqueueResult, QueueOccupancy, SpscQueue};

/// Thread-safe ring buffer queue using `ringbuf` with interior `Arc<Mutex<...>>`.
///
/// This is intended for P2 concurrent runtimes where multiple threads may
/// attempt to push/pop/inspect occupancy. `try_peek` (borrowing) is not
/// possible through a lock guard; override `try_peek_cloned` instead.
#[derive(Clone)]
pub struct SpscConcurrentRingbuf<T> {
    prod: Arc<Mutex<HeapProd<T>>>,
    cons: Arc<Mutex<HeapCons<T>>>,
    cap: usize,
    bytes: Arc<Mutex<usize>>,
}

impl<T> SpscConcurrentRingbuf<T> {
    /// Create with item capacity (rounded up to next power-of-two for the underlying rb).
    pub fn with_capacity(capacity: usize) -> Self {
        let rb = HeapRb::<T>::new(capacity.next_power_of_two());
        let (prod, cons) = rb.split();
        Self {
            prod: Arc::new(Mutex::new(prod)),
            cons: Arc::new(Mutex::new(cons)),
            cap: capacity,
            bytes: Arc::new(Mutex::new(0)),
        }
    }

    #[inline]
    fn len_internal(&self) -> usize {
        match self.cons.lock() {
            Ok(guard) => guard.occupied_len(),
            Err(_) => 0,
        }
    }

    #[inline]
    fn is_full(&self) -> bool {
        let logical_full = self.len_internal() >= self.cap;
        if logical_full {
            return true;
        }
        match self.prod.lock() {
            Ok(guard) => guard.is_full(),
            Err(_) => true,
        }
    }
}

impl<P> SpscQueue for SpscConcurrentRingbuf<Message<P>>
where
    P: Payload + Clone,
{
    type Item = Message<P>;

    fn try_push(&mut self, item: Self::Item, policy: &EdgePolicy) -> EnqueueResult {
        let items = self.len_internal();
        let bytes = match self.bytes.lock() {
            Ok(b) => *b,
            Err(_) => return EnqueueResult::Rejected,
        };

        if policy.caps.at_or_above_hard(items, bytes) && self.is_full() {
            return EnqueueResult::Rejected;
        }

        // Apply DropOldest if within soft..hard and configured
        if let Ok(mut cons) = self.cons.lock() {
            match policy.watermark(items, bytes) {
                WatermarkState::BetweenSoftAndHard
                    if matches!(policy.admission, AdmissionPolicy::DropOldest)
                        && cons.occupied_len() > 0 =>
                {
                    if let Some(ev) = cons.try_pop() {
                        if let Ok(mut b) = self.bytes.lock() {
                            *b = b.saturating_sub(ev.header.payload_size_bytes);
                        }
                    }
                }
                _ => {}
            }
        }

        if self.is_full() {
            return EnqueueResult::Rejected;
        }

        let payload_bytes = item.header.payload_size_bytes;

        // Enqueue
        match self.prod.lock() {
            Ok(mut prod) => {
                if prod.try_push(item).is_err() {
                    return EnqueueResult::Rejected;
                }
            }
            Err(_) => return EnqueueResult::Rejected,
        }

        // Update bytes
        if let Ok(mut b) = self.bytes.lock() {
            *b = b.saturating_add(payload_bytes);
        }

        EnqueueResult::Enqueued
    }

    fn try_pop(&mut self) -> Result<Self::Item, QueueError> {
        match self.cons.lock() {
            Ok(mut cons) => match cons.try_pop() {
                Some(item) => {
                    if let Ok(mut b) = self.bytes.lock() {
                        *b = b.saturating_sub(item.header.payload_size_bytes);
                    }
                    Ok(item)
                }
                None => Err(QueueError::Empty),
            },
            Err(_) => Err(QueueError::Poisoned),
        }
    }

    fn occupancy(&self, policy: &EdgePolicy) -> QueueOccupancy {
        let items = self.len_internal();
        let bytes = match self.bytes.lock() {
            Ok(b) => *b,
            Err(_) => 0,
        };
        let watermark = policy.watermark(items, bytes);
        QueueOccupancy {
            items,
            bytes,
            watermark,
        }
    }

    fn try_peek(&self) -> Result<&Self::Item, QueueError> {
        // Borrowed peek cannot be returned across a Mutex guard boundary.
        Err(QueueError::Empty)
    }

    // std-only owned peek that clones the front item without removing it.
    #[inline]
    fn try_peek_cloned(&self) -> Result<Self::Item, QueueError> {
        match self.cons.lock() {
            Ok(cons) => match cons.try_peek() {
                Some(item_ref) => Ok(item_ref.clone()),
                None => Err(QueueError::Empty),
            },
            Err(_) => Err(QueueError::Poisoned),
        }
    }
}

impl<T> fmt::Debug for SpscConcurrentRingbuf<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let items = match self.cons.lock() {
            Ok(guard) => guard.occupied_len(),
            Err(_) => 0,
        };
        let bytes = match self.bytes.lock() {
            Ok(b) => *b,
            Err(_) => 0,
        };

        f.debug_struct("SpscConcurrentRingbuf")
            .field("cap", &self.cap)
            .field("items", &items)
            .field("bytes", &bytes)
            .finish()
    }
}
