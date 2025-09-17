//! SpscAtomicRing: high-performance SPSC ring using atomics (P2 high-perf).
//!
//! **Unsafe implementation** gated behind `ring_unsafe`.
//! Uses a power-of-two capacity array of `MaybeUninit<T>` with atomic head/tail.

#![allow(unsafe_code)]

use core::ptr;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::errors::QueueError;
use crate::message::{payload::Payload, Message};
use crate::policy::{AdmissionPolicy, EdgePolicy, WatermarkState};
use crate::queue::{EnqueueResult, QueueOccupancy, SpscQueue};

/// A high-performance, bounded, single-producer single-consumer ring buffer.
///
/// Stores elements in a power-of-two array of `MaybeUninit<T>` and advances
/// head and tail indices with atomic operations. Also tracks a byte counter
/// for admission control policies. This type assumes strict single-producer
/// single-consumer usage throughout its lifetime.
pub struct SpscAtomicRing<T> {
    buf: Box<[MaybeUninit<T>]>,
    cap: usize,
    head: AtomicUsize, // consumer index
    tail: AtomicUsize, // producer index
    bytes_in_queue: AtomicUsize,
}

impl<T> SpscAtomicRing<T> {
    /// Creates a ring with the given item capacity.
    ///
    /// The capacity must be a power of two; indices wrap using a bit mask.
    ///
    /// # Safety
    ///
    /// This constructor initializes unfilled storage via `Vec::set_len` and
    /// returns a queue that relies on strict usage invariants which the caller
    /// must uphold for the entire lifetime of the queue:
    ///
    /// - **Single producer, single consumer discipline**: exactly one producer
    ///   thread may call the enqueue operations and exactly one consumer thread
    ///   may call the dequeue and peek operations. No other threads may access
    ///   the queue concurrently.
    /// - **Do not read uninitialized slots**: only dequeue or peek when the
    ///   queue is known to be non-empty. A referenced item obtained from `peek`
    ///   becomes invalid as soon as the consumer advances the head index.
    /// - **Do not overwrite live elements**: only enqueue when the queue is
    ///   not full. The producer must write the element to the slot before it
    ///   advances the tail index; the consumer must read the element before it
    ///   advances the head index.
    /// - **Capacity constraint**: `capacity` must be a power of two. This
    ///   function asserts that condition; violating it would break index
    ///   masking assumptions elsewhere.
    /// - **Element drop behavior**: any elements left in the queue at drop
    ///   time will be read and dropped during the queue’s `Drop` implementation.
    ///   If `T` has side effects or can panic on drop, those effects will occur
    ///   at that time.
    pub unsafe fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two(),
            "capacity must be a power of two"
        );
        let mut v: Vec<MaybeUninit<T>> = Vec::with_capacity(capacity);
        unsafe {
            v.set_len(capacity);
        }
        Self {
            buf: v.into_boxed_slice(),
            cap: capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            bytes_in_queue: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn mask(&self) -> usize {
        self.cap - 1
    }

    #[inline]
    fn len(&self) -> usize {
        let h = self.head.load(Ordering::Acquire);
        let t = self.tail.load(Ordering::Acquire);
        t.wrapping_sub(h) & self.mask()
    }

    #[inline]
    fn is_full(&self) -> bool {
        self.len() == self.cap - 1
    }

    #[inline]
    fn push_raw(&self, item: T) {
        // Compute a *mut T to the target slot and write without dropping the old value.
        let t = self.tail.load(Ordering::Relaxed);
        let idx = t & self.mask();
        let base: *mut MaybeUninit<T> = self.buf.as_ptr() as *mut MaybeUninit<T>;
        let slot: *mut T = unsafe { base.add(idx) as *mut T };
        unsafe { ptr::write(slot, item) };
        self.tail.store(t.wrapping_add(1), Ordering::Release);
    }

    #[inline]
    fn pop_raw(&self) -> T {
        // Compute a *const T to the current head slot and read it by value.
        let h = self.head.load(Ordering::Relaxed);
        let idx = h & self.mask();
        let base: *const MaybeUninit<T> = self.buf.as_ptr();
        let slot: *const T = unsafe { base.add(idx) as *const T };
        let item = unsafe { ptr::read(slot) };
        self.head.store(h.wrapping_add(1), Ordering::Release);
        item
    }
}

impl<T> Drop for SpscAtomicRing<T> {
    fn drop(&mut self) {
        // Drain any initialized items to drop them safely.
        while self.len() > 0 {
            let _ = self.pop_raw();
        }
    }
}

impl<P: Payload + std::clone::Clone> SpscQueue for SpscAtomicRing<Message<P>> {
    type Item = Message<P>;

    fn try_push(&mut self, item: Self::Item, policy: &EdgePolicy) -> EnqueueResult {
        let items = self.len();
        let bytes = self.bytes_in_queue.load(Ordering::Acquire);

        if policy.caps.at_or_above_hard(items, bytes) && self.is_full() {
            return EnqueueResult::Rejected;
        }

        match policy.watermark(items, bytes) {
            WatermarkState::BetweenSoftAndHard
                if matches!(policy.admission, AdmissionPolicy::DropOldest) && self.len() > 0 =>
            {
                let ev = self.pop_raw();
                self.bytes_in_queue
                    .fetch_sub(ev.header.payload_size_bytes, Ordering::AcqRel);
            }
            _ => {}
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
        let h = self.head.load(Ordering::Acquire);
        let idx = h & self.mask();
        let base: *const MaybeUninit<Self::Item> = self.buf.as_ptr();
        let slot: *const Self::Item = unsafe { base.add(idx) as *const Self::Item };
        // SAFETY: Producer writes only after consumer advances `head`.
        Ok(unsafe { &*slot })
    }
}
