//! Lock-free SPSC ring buffer using atomics (P2).
//!
//! This queue is safe for **one producer thread** and **one consumer thread**.
//! It implements `limen_core::queue::SpscQueue` for `Message<P>` so byte-level
//! occupancy can be tracked for admission purposes.
#![allow(unsafe_code)]

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

use limen_core::errors::QueueError;
use limen_core::message::{Message, Payload};
use limen_core::policy::WatermarkState;
use limen_core::policy::{AdmissionPolicy, EdgePolicy};
use limen_core::queue::{EnqueueResult, QueueOccupancy, SpscQueue};

/// A lock-free SPSC ring buffer.
pub struct LockFreeRing<T> {
    buf: Box<[MaybeUninit<T>]>,

    cap: usize,
    head: AtomicUsize, // consumer index
    tail: AtomicUsize, // producer index

    // Non-atomic because each field is mutated by a single role:
    // - bytes_in_queue is updated by both producer and consumer, so it must be atomic.
    // - len is derived from head/tail; we compute when needed.
    bytes_in_queue: AtomicUsize,

    // Allow interior element mutation; accesses are synchronized by SPSC discipline.
    storage: UnsafeCell<()>,
}

// Safety: SPSC discipline allows Send because only one producer and one consumer operate.
// T is required to be Send to move across threads.
unsafe impl<T: Send> Send for LockFreeRing<T> {}
unsafe impl<T: Send> Sync for LockFreeRing<T> {}

impl<T> LockFreeRing<T> {
    /// Create a new ring with the given capacity in items (must be power of two for fast mask indexing).
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two(),
            "capacity must be a power of two"
        );
        let mut v: Vec<MaybeUninit<T>> = Vec::with_capacity(capacity);
        // SAFETY: set_len to capacity to allow indexed writes; elements remain uninitialized.
        unsafe {
            v.set_len(capacity);
        }
        Self {
            buf: v.into_boxed_slice(),
            cap: capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            bytes_in_queue: AtomicUsize::new(0),
            storage: UnsafeCell::new(()),
        }
    }

    #[inline]
    fn mask(&self) -> usize {
        self.cap - 1
    }

    #[inline]
    fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        tail.wrapping_sub(head) & self.mask()
    }

    #[inline]
    fn is_full(&self) -> bool {
        self.len() == self.cap - 1
    }

    #[inline]
    fn push_raw(&self, item: T) {
        let tail = self.tail.load(Ordering::Relaxed);
        // SAFETY: slot is free because producer ensured !is_full()
        unsafe {
            self.buf[tail & self.mask()]
                .as_ptr()
                .cast::<T>()
                .write(item)
        };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
    }

    #[inline]
    fn pop_raw(&self) -> T {
        let head = self.head.load(Ordering::Relaxed);
        // SAFETY: slot is initialized because consumer ensured len() > 0
        let item = unsafe { self.buf[head & self.mask()].as_ptr().cast::<T>().read() };
        self.head.store(head.wrapping_add(1), Ordering::Release);
        item
    }
}

impl<T> Drop for LockFreeRing<T> {
    fn drop(&mut self) {
        // Drain initialized items to drop them safely.
        while self.len() > 0 {
            let _ = self.pop_raw();
        }
    }
}

impl<P: Payload> SpscQueue for LockFreeRing<Message<P>> {
    type Item = Message<P>;

    fn try_push(&mut self, item: Self::Item, policy: &EdgePolicy) -> EnqueueResult {
        let items = self.len();
        let bytes = self.bytes_in_queue.load(Ordering::Acquire);

        if policy.caps.at_or_above_hard(items, bytes) && self.is_full() {
            return EnqueueResult::Rejected;
        }

        let wm = policy.watermark(items, bytes);
        if matches!(wm, WatermarkState::BetweenSoftAndHard) {
            if matches!(policy.admission, AdmissionPolicy::DropOldest) && self.len() > 0 {
                let evicted = self.pop_raw();
                self.bytes_in_queue
                    .fetch_sub(evicted.header.payload_size_bytes, Ordering::AcqRel);
            }
        }

        if self.is_full() {
            return EnqueueResult::Rejected;
        }

        self.bytes_in_queue
            .fetch_add(item.header.payload_size_bytes, Ordering::AcqRel);
        self.push_raw(item);
        EnqueueResult::Enqueued
    }

    fn try_pop(&mut self) -> Result<Self::Item, QueueError> {
        if self.len() == 0 {
            return Err(QueueError::Empty);
        }
        let item = self.pop_raw();
        self.bytes_in_queue
            .fetch_sub(item.header.payload_size_bytes, Ordering::AcqRel);
        Ok(item)
    }

    fn occupancy(&self, policy: &EdgePolicy) -> QueueOccupancy {
        let items = self.len();
        let bytes = self.bytes_in_queue.load(Ordering::Acquire);
        let watermark = policy.watermark(items, bytes);
        QueueOccupancy {
            items,
            bytes,
            watermark,
        }
    }

    fn try_peek(&self) -> Result<&Self::Item, QueueError> {
        if self.len() == 0 {
            return Err(QueueError::Empty);
        }
        let head = self.head.load(Ordering::Acquire);
        let ptr = self.buf[head & self.mask()].as_ptr().cast::<Self::Item>();
        // SAFETY: element is initialized and will not be overwritten by producer until consumed.
        Ok(unsafe { &*ptr })
    }
}
