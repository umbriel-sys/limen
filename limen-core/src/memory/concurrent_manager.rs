//! Concurrent heap-backed memory manager with lock-free freelist.
//!
//! This module implements a production-ready concurrent memory manager that
//! satisfies the C1a requirements:
//!
//! - Per-slot locking: reads/writes only lock the targeted slot using an
//!   `RwLock`, giving fine-grained isolation and good concurrency for reads.
//! - No whole-manager lock: allocation and free are lock-free using a Treiber
//!   stack free-list implemented with atomics + a 32-bit tag to avoid ABA.
//! - Heap-backed capacity: the manager takes a `capacity` at construction and
//!   stores slots/next pointers in `Vec` so the size is dynamic when `alloc`
//!   is available.
//! - Ergonomic concurrent API: `store_shared`, `read_shared`, etc. take `&self`
//!   and are safe for concurrent use; trait methods required by
//!   `MemoryManager` delegate to these shared methods.
//!
//! Rationale
//! ---------
//! - A Treiber stack with a tagged head (index + u32 tag packed into `u64`)
//!   gives a simple, fast lock-free freelist and mitigates ABA by advancing
//!   the tag on each successful CAS. The tag wraps naturally and safely.
//! - Per-slot `RwLock` ensures that reads are concurrent and writes are
//!   exclusive on a per-slot basis. Combined with the lock-free freelist this
//!   means no global lock is held during steady-state reads and writes.
//! - `available_count` is an `AtomicUsize` for cheap diagnostics without
//!   locking and is kept consistent with successful freelist operations.
//!
//! Safety notes
//! ------------
//! - `free_shared` uses a write lock on the slot, which waits for existing
//!   readers to finish. This ensures correctness: freeing a slot cannot race
//!   with readers that think the message exists because readers hold the read
//!   lock while accessing `message`.
//! - `pop_free` / `push_free` are lock-free loops using atomic CAS on the
//!   tagged head; they may spin under heavy contention. Backoff/yielding is
//!   used to avoid burning a single core indefinitely.
//!
//! Performance notes
//! -----------------
//! - Reads are highly concurrent and cheap (single `RwLock::read` + access).
//! - Writes are exclusive per-slot and only block readers of the same slot.
//! - Allocation/free operations are lock-free and fast; the only blocking
//!   synchronization remaining is slot `RwLock` for the actual store/free.
//!
//! Tests in this module exercise correctness under concurrent usage and
//! allocation reuse.

use core::ops::{Deref, DerefMut};
use std::sync::{
    atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Arc, RwLock, RwLockReadGuard, RwLockWriteGuard,
};

use crate::errors::MemoryError;
use crate::memory::header_store::HeaderStore;
use crate::memory::manager::MemoryManager;
use crate::memory::MemoryClass;
use crate::message::payload::Payload;
use crate::message::{Message, MessageHeader};
use crate::prelude::ScopedManager;
use crate::types::MessageToken;

/// Special value meaning "no index" for the freelist next pointer and head.
const EMPTY_INDEX: u32 = u32::MAX;

/// Pack helper for the free-list head: upper 32 bits contain a tag (u32),
/// lower 32 bits contain the head index (u32). The tagged head prevents ABA
/// by changing the tag on each successful CAS update.
#[inline]
fn pack_head(tag: u32, idx: u32) -> u64 {
    ((tag as u64) << 32) | (idx as u64)
}
/// Unpack helper returning (tag, index).
#[inline]
fn unpack_head(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, (v & 0xffff_ffff) as u32)
}

/// The mutable per-slot payload state protected by an `RwLock`.
///
/// The `message` field is `Option<Message<P>>`. While `Some`, the slot is
/// considered allocated and its content must be accessible to readers and
/// writers only while appropriate locks are held.
struct ConcurrentSlotState<P: Payload> {
    message: Option<Message<P>>,
}

impl<P: Payload> ConcurrentSlotState<P> {
    /// Construct an empty slot state (unallocated).
    fn new() -> Self {
        Self { message: None }
    }
}

/// A slot combines the `RwLock` with its `ConcurrentSlotState`.
///
/// We keep a `ConcurrentSlot` abstraction so the shared vector type is clear
/// and we can initialize slots uniformly.
struct ConcurrentSlot<P: Payload> {
    state: RwLock<ConcurrentSlotState<P>>,
}

impl<P: Payload> ConcurrentSlot<P> {
    /// Construct a new slot with an empty state.
    fn new() -> Self {
        Self {
            state: RwLock::new(ConcurrentSlotState::new()),
        }
    }
}

/// The shared, heap-backed state of the concurrent manager.
///
/// This struct is reference-counted by `Arc` inside `ConcurrentMemoryManager` so
/// multiple handles can be cloned cheaply and share the underlying storage.
struct ConcurrentMemoryManagerShared<P: Payload> {
    /// The per-slot RwLocks and states.
    slots: Vec<ConcurrentSlot<P>>,

    /// Per-slot "next" pointers used by the Treiber free-list. `next_free[i]`
    /// stores the index of the successor of `i` in the freelist, or
    /// `EMPTY_INDEX` for none.
    next_free: Vec<AtomicU32>,

    /// The freelist head is a packed (tag, index) in an AtomicU64. Lower 32
    /// bits: index of head or EMPTY_INDEX; upper 32 bits: tag counter.
    free_head: AtomicU64,

    /// Number of free slots, updated on successful push/pop. Used for quick
    /// diagnostics and admission control. Relaxed loads are acceptable.
    available_count: AtomicUsize,

    /// Memory class metadata for diagnostics / policy.
    mem_class: MemoryClass,
}

impl<P: Payload> ConcurrentMemoryManagerShared<P> {
    /// Create a new shared state object with `capacity` slots and initial
    /// freelist linking 0 -> 1 -> 2 -> ... -> capacity-1 -> EMPTY.
    ///
    /// The `mem_class` parameter describes where the memory logically resides.
    fn new(mem_class: MemoryClass, capacity: usize) -> Self {
        assert!(capacity <= u32::MAX as usize);
        // Initialize slots vector with RwLocks protecting empty states.
        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(ConcurrentSlot::new());
        }

        // Initialize the per-slot next_free pointers to form a singly-linked
        // list of free indices. This is the initial freelist state.
        let mut next_free = Vec::with_capacity(capacity);
        for i in 0..capacity {
            let next = if i + 1 < capacity {
                (i + 1) as u32
            } else {
                EMPTY_INDEX
            };
            next_free.push(AtomicU32::new(next));
        }

        // Head initially points to index 0 with tag 0 (unless capacity==0).
        let head_index = if capacity > 0 { 0 } else { EMPTY_INDEX };
        let free_head = AtomicU64::new(pack_head(0, head_index));
        let available_count = AtomicUsize::new(capacity);

        Self {
            slots,
            next_free,
            free_head,
            available_count,
            mem_class,
        }
    }

    // ---------------------------------------------------------------------
    // Lock-free freelist operations (Treiber stack)
    // ---------------------------------------------------------------------

    /// Pop (allocate) an index from the freelist in a lock-free manner.
    ///
    /// Returns `Some(index)` on success, or `None` if the freelist is empty.
    /// On success the `available_count` is decremented. The caller must then
    /// acquire the slot's write lock before storing into the slot.
    ///
    /// Correctness notes:
    /// - This is a classic Treiber stack pop with a packed (tag,index) head to
    ///   avoid ABA. The tag is incremented on every successful CAS.
    /// - We use `Ordering::Acquire` for loads and `AcqRel` for the successful
    ///   CAS to ensure proper memory ordering with respect to `next_free`.
    fn pop_free(&self) -> Option<usize> {
        let mut spins = 0u32;
        loop {
            // Load the packed head (Acquire)
            let head = self.free_head.load(Ordering::Acquire);
            let (tag, idx) = unpack_head(head);

            // Empty?
            if idx == EMPTY_INDEX {
                return None;
            }

            // Read the successor index of the head. Acquire to synchronize.
            let next = self.next_free[idx as usize].load(Ordering::Acquire);

            // Prepare a new head with incremented tag and successor index.
            let new_tag = tag.wrapping_add(1);
            let new_head = pack_head(new_tag, next);

            // Attempt CAS to move head -> successor.
            if self
                .free_head
                .compare_exchange(head, new_head, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                // Successful pop: update diagnostics and return index.
                self.available_count.fetch_sub(1, Ordering::AcqRel);
                return Some(idx as usize);
            }

            // CAS failed due to concurrent updates. Backoff occasionally.
            spins = spins.wrapping_add(1);
            if spins & 0xFF == 0 {
                std::thread::yield_now();
            }
        }
    }

    /// Push (free) an index back onto the freelist in a lock-free manner.
    ///
    /// On success the `available_count` is incremented. The caller must ensure
    /// the slot is already cleared (message removed) before pushing it back.
    ///
    /// Correctness notes:
    /// - We store the current head into `next_free[idx]` (Release) and then try
    ///   to CAS the head to `(tag+1, idx)`. A successful CAS inserts `idx`
    ///   as the new head. We increment the available counter only on success.
    fn push_free(&self, idx: usize) {
        let mut spins = 0u32;
        loop {
            // Load the packed head (Acquire)
            let head = self.free_head.load(Ordering::Acquire);
            let (tag, head_idx) = unpack_head(head);

            // Publish the current head as the successor of the index being pushed.
            // Release ordering ensures successor becomes visible before CAS.
            self.next_free[idx].store(head_idx, Ordering::Release);

            // New head has incremented tag and index==idx.
            let new_tag = tag.wrapping_add(1);
            let new_head = pack_head(new_tag, idx as u32);

            if self
                .free_head
                .compare_exchange(head, new_head, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                // Successful push: update diagnostics and return.
                self.available_count.fetch_add(1, Ordering::AcqRel);
                return;
            }

            // CAS failed; retry with occasional yield.
            spins = spins.wrapping_add(1);
            if spins & 0xFF == 0 {
                std::thread::yield_now();
            }
        }
    }
}

/// Public concurrent manager handle.
///
/// This is cheap to clone (clones the `Arc`), so multiple threads can share
/// a `ConcurrentMemoryManager` instance and call the `*_shared(&self)`
/// methods concurrently.
pub struct ConcurrentMemoryManager<P: Payload> {
    shared: Arc<ConcurrentMemoryManagerShared<P>>,
}

impl<P: Payload> Clone for ConcurrentMemoryManager<P> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<P: Payload> ConcurrentMemoryManager<P> {
    // ---------------------------------------------------------------------
    // Construction
    // ---------------------------------------------------------------------

    /// Create a new manager with the given `capacity`. Capacity must be finite
    /// and typically small-to-moderate (4..1024) for best performance.
    ///
    /// The manager defaults to `MemoryClass::Host`.
    pub fn new(capacity: usize) -> Self {
        Self::with_memory_class(capacity, MemoryClass::Host)
    }

    /// Create a new manager with explicit `memory_class` metadata.
    ///
    /// `memory_class` is useful for telemetry and for future routing decisions
    /// when different storage classes exist.
    pub fn with_memory_class(capacity: usize, mem_class: MemoryClass) -> Self {
        let shared = ConcurrentMemoryManagerShared::new(mem_class, capacity);
        Self {
            shared: Arc::new(shared),
        }
    }

    // ---------------------------------------------------------------------
    // Ergonomic shared (&self) API for concurrent use
    // ---------------------------------------------------------------------
    //
    // These methods all take `&self` so multiple threads can call them on the
    // same manager handle. The trait `MemoryManager` requires `&mut self`,
    // so we implement the trait by delegating to these methods below.

    /// Store a `Message<P>` and return its `MessageToken`.
    ///
    /// This method:
    /// - Pops a free index from the freelist (lock-free).
    /// - Acquires the slot write lock and sets `message = Some(value)`.
    ///
    /// Returns `MemoryError::NoFreeSlots` if the freelist is empty.
    ///
    /// Concurrency: multiple threads may concurrently allocate and store into
    /// different slots. Only the chosen slot is write-locked during the store.
    pub fn store_shared(&self, value: Message<P>) -> Result<MessageToken, MemoryError> {
        // Obtain a free slot index from the lock-free freelist.
        let idx = match self.shared.pop_free() {
            None => return Err(MemoryError::NoFreeSlots),
            Some(i) => i,
        };

        // Acquire the slot's write lock (this is fine: this lock only affects
        // the single slot) and store the message.
        let slot = &self.shared.slots[idx];
        let mut guard = slot.state.write().map_err(|_| MemoryError::Poisoned)?;
        guard.message = Some(value);
        drop(guard);

        Ok(MessageToken::new(idx as u32))
    }

    /// Borrow a stored message immutably, returning a guard that keeps the
    /// slot read-locked for the lifetime of the guard.
    ///
    /// Returns:
    /// - `BadToken` if the token index is out of range,
    /// - `NotAllocated` if the slot is currently empty,
    /// - `Poisoned` if the slot lock is poisoned.
    ///
    /// Concurrency: read locks allow many concurrent readers for the same slot.
    pub fn read_shared(
        &self,
        token: MessageToken,
    ) -> Result<ConcurrentReadGuard<'_, P>, MemoryError> {
        let idx = token.index();
        if idx >= self.shared.slots.len() {
            return Err(MemoryError::BadToken);
        }
        let slot = &self.shared.slots[idx];

        // Acquire read lock and validate allocation.
        let guard = slot.state.read().map_err(|_| MemoryError::Poisoned)?;
        if guard.message.is_none() {
            return Err(MemoryError::NotAllocated);
        }

        Ok(ConcurrentReadGuard { guard })
    }

    /// Borrow a stored message mutably, returning a guard that keeps the slot
    /// write-locked for the lifetime of the guard.
    ///
    /// This enforces exclusive access to the stored message while the guard is live.
    pub fn read_mut_shared(
        &self,
        token: MessageToken,
    ) -> Result<ConcurrentWriteGuard<'_, P>, MemoryError> {
        let idx = token.index();
        if idx >= self.shared.slots.len() {
            return Err(MemoryError::BadToken);
        }
        let slot = &self.shared.slots[idx];

        // Acquire write lock which excludes readers/writers on the slot.
        let guard = slot.state.write().map_err(|_| MemoryError::Poisoned)?;
        if guard.message.is_none() {
            return Err(MemoryError::NotAllocated);
        }
        Ok(ConcurrentWriteGuard { guard })
    }

    /// Free a previously allocated token/slot.
    ///
    /// Behavior:
    /// - Acquire the slot write lock (this will wait for readers/writers).
    /// - Ensure the slot is allocated; set `message = None`.
    /// - Push the index back onto the lock-free freelist.
    ///
    /// Returns `NotAllocated` if slot already empty or `BadToken` if token invalid.
    pub fn free_shared(&self, token: MessageToken) -> Result<(), MemoryError> {
        let idx = token.index();
        if idx >= self.shared.slots.len() {
            return Err(MemoryError::BadToken);
        }
        let slot = &self.shared.slots[idx];

        // Acquire write lock to ensure exclusivity for clearing the slot.
        let mut guard = slot.state.write().map_err(|_| MemoryError::Poisoned)?;
        if guard.message.is_none() {
            return Err(MemoryError::NotAllocated);
        }
        guard.message = None;
        drop(guard);

        // Return index to freelist lock-free.
        self.shared.push_free(idx);
        Ok(())
    }

    /// Return the approximate number of free slots. This is updated atomically
    /// on each successful freelist push/pop. The value is diagnostic and may be
    /// slightly stale if concurrent operations are in-flight.
    pub fn available(&self) -> usize {
        self.shared.available_count.load(Ordering::Relaxed)
    }

    /// Return the configured capacity (number of slots).
    pub fn capacity(&self) -> usize {
        self.shared.slots.len()
    }

    /// Return the memory class attached to this manager.
    pub fn memory_class(&self) -> MemoryClass {
        self.shared.mem_class
    }
}

// ---------------------------------------------------------------------
// Guard wrappers that hold the slot locks while the guard is live.
// ---------------------------------------------------------------------

/// Header guard — holds a read lock on the slot and derefs to `MessageHeader`.
pub struct ConcurrentHeaderGuard<'a, P: Payload> {
    guard: RwLockReadGuard<'a, ConcurrentSlotState<P>>,
}

impl<'a, P: Payload> Deref for ConcurrentHeaderGuard<'a, P> {
    type Target = MessageHeader;
    fn deref(&self) -> &Self::Target {
        self.guard
            .message
            .as_ref()
            .expect("header guard constructed only when Some")
            .header()
    }
}

/// Read guard — holds a read lock and dereferences to `Message<P>`.
pub struct ConcurrentReadGuard<'a, P: Payload> {
    guard: RwLockReadGuard<'a, ConcurrentSlotState<P>>,
}

impl<'a, P: Payload> Deref for ConcurrentReadGuard<'a, P> {
    type Target = Message<P>;
    fn deref(&self) -> &Self::Target {
        self.guard
            .message
            .as_ref()
            .expect("read guard constructed only when Some")
    }
}

/// Write guard — holds a write lock and dereferences (mutably) to `Message<P>`.
pub struct ConcurrentWriteGuard<'a, P: Payload> {
    guard: RwLockWriteGuard<'a, ConcurrentSlotState<P>>,
}

impl<'a, P: Payload> Deref for ConcurrentWriteGuard<'a, P> {
    type Target = Message<P>;
    fn deref(&self) -> &Self::Target {
        self.guard
            .message
            .as_ref()
            .expect("write guard constructed only when Some")
    }
}

impl<'a, P: Payload> DerefMut for ConcurrentWriteGuard<'a, P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard
            .message
            .as_mut()
            .expect("write guard constructed only when Some")
    }
}

// ---------------------------------------------------------------------
// Trait implementations
// ---------------------------------------------------------------------

impl<P: Payload> HeaderStore for ConcurrentMemoryManager<P> {
    type HeaderGuard<'a>
        = ConcurrentHeaderGuard<'a, P>
    where
        Self: 'a;

    /// Peek the header of the stored message identified by `token`.
    ///
    /// This acquires a slot read lock and returns a guard that keeps the lock
    /// for the lifetime of the returned guard.
    fn peek_header(&self, token: MessageToken) -> Result<Self::HeaderGuard<'_>, MemoryError> {
        let idx = token.index();
        if idx >= self.shared.slots.len() {
            return Err(MemoryError::BadToken);
        }
        let slot = &self.shared.slots[idx];

        // Acquire read lock and validate that slot is allocated.
        let guard = slot.state.read().map_err(|_| MemoryError::Poisoned)?;
        if guard.message.is_none() {
            return Err(MemoryError::NotAllocated);
        }

        Ok(ConcurrentHeaderGuard { guard })
    }
}

impl<P: Payload> MemoryManager<P> for ConcurrentMemoryManager<P> {
    type ReadGuard<'a>
        = ConcurrentReadGuard<'a, P>
    where
        Self: 'a;

    type WriteGuard<'a>
        = ConcurrentWriteGuard<'a, P>
    where
        Self: 'a;

    // The trait requires &mut self for store/read_mut/free. For the concurrent
    // manager we delegate those trait methods to the `&self` shared methods so
    // callers can use either the trait (with a mutable handle) or the shared
    // API for concurrency.
    fn store(&mut self, value: Message<P>) -> Result<MessageToken, MemoryError> {
        // Delegate to the &self method: safe since we use internal Arc-shared state.
        self.store_shared(value)
    }

    fn read(&self, token: MessageToken) -> Result<Self::ReadGuard<'_>, MemoryError> {
        self.read_shared(token)
    }

    fn read_mut(&mut self, token: MessageToken) -> Result<Self::WriteGuard<'_>, MemoryError> {
        self.read_mut_shared(token)
    }

    fn free(&mut self, token: MessageToken) -> Result<(), MemoryError> {
        self.free_shared(token)
    }

    fn available(&self) -> usize {
        self.available()
    }

    fn capacity(&self) -> usize {
        self.capacity()
    }

    fn memory_class(&self) -> MemoryClass {
        self.memory_class()
    }
}

impl<P: Payload + Send + Sync> ScopedManager<P> for ConcurrentMemoryManager<P> {
    type Handle<'a>
        = ConcurrentMemoryManager<P>
    where
        Self: 'a;

    fn scoped_handle<'a>(&'a self) -> Self::Handle<'a>
    where
        Self: 'a,
    {
        self.clone()
    }
}

// ---------------------------------------------------------------------
// Tests (std only)
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageHeader;
    use crate::prelude::{create_test_tensor_filled_with, TestTensor, TEST_TENSOR_BYTE_COUNT};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    // Helper: build a simple Message<TestTensor>.
    fn make_msg(val: u32) -> Message<TestTensor> {
        Message::new(MessageHeader::empty(), create_test_tensor_filled_with(val))
    }

    // --- Concurrency-oriented tests ------------------------------------------------

    #[test]
    fn basic_store_read_free() {
        let mgr: ConcurrentMemoryManager<TestTensor> = ConcurrentMemoryManager::new(4);

        let t = mgr.store_shared(make_msg(10)).unwrap();
        assert_eq!(mgr.available(), 3);

        {
            let g = mgr.read_shared(t).unwrap();
            assert_eq!(*g.payload(), create_test_tensor_filled_with(10));
        }

        mgr.free_shared(t).unwrap();
        assert_eq!(mgr.available(), 4);
    }

    #[test]
    fn concurrent_reads_same_slot() {
        let mgr = Arc::new(ConcurrentMemoryManager::<TestTensor>::new(4));
        let t = mgr.store_shared(make_msg(5)).unwrap();

        let m1 = mgr.clone();
        let th1 = thread::spawn(move || {
            let g = m1.read_shared(t).unwrap();
            assert_eq!(*g.payload(), create_test_tensor_filled_with(5));
        });

        let m2 = mgr.clone();
        let th2 = thread::spawn(move || {
            let g = m2.read_shared(t).unwrap();
            assert_eq!(*g.payload(), create_test_tensor_filled_with(5));
        });

        th1.join().unwrap();
        th2.join().unwrap();

        mgr.free_shared(t).unwrap();
    }

    #[test]
    fn write_excludes_read() {
        use std::sync::Barrier;

        let mgr = Arc::new(ConcurrentMemoryManager::<TestTensor>::new(4));
        let t = mgr.store_shared(make_msg(7)).unwrap();

        // Barrier: writer signals after acquiring the lock, reader waits before reading
        let barrier = Arc::new(Barrier::new(2));

        let mwriter = mgr.clone();
        let bwriter = barrier.clone();
        let writer = thread::spawn(move || {
            let mut w = mwriter.read_mut_shared(t).unwrap();
            *w.payload_mut() = create_test_tensor_filled_with(42);
            // Signal: lock is held and value is written
            bwriter.wait();
            // Hold lock until reader has had a chance to block on it
            std::thread::sleep(Duration::from_millis(50));
        });

        // Wait until writer confirms it holds the lock with value written
        barrier.wait();

        // Now read_shared must block until writer releases; when it returns, value is 42
        let g = mgr.read_shared(t).unwrap();
        assert_eq!(*g.payload(), create_test_tensor_filled_with(42));

        writer.join().unwrap();
    }

    #[test]
    fn allocate_exhaustion_and_reuse() {
        let mgr = ConcurrentMemoryManager::<TestTensor>::new(2);
        let t0 = mgr.store_shared(make_msg(1)).unwrap();
        let t1 = mgr.store_shared(make_msg(2)).unwrap();
        assert_eq!(mgr.available(), 0);
        assert!(matches!(
            mgr.store_shared(make_msg(3)),
            Err(MemoryError::NoFreeSlots)
        ));

        mgr.free_shared(t0).unwrap();
        assert_eq!(mgr.available(), 1);

        // t0 slot reused
        let t2 = mgr.store_shared(make_msg(4)).unwrap();
        assert_eq!(t2.index(), t0.index());
        mgr.free_shared(t1).unwrap();
        mgr.free_shared(t2).unwrap();
    }

    // --- Parity tests mirroring StaticMemoryManager contract -----------------------

    #[test]
    fn store_read_free_cycle() {
        let mgr: ConcurrentMemoryManager<TestTensor> = ConcurrentMemoryManager::new(4);
        assert_eq!(mgr.available(), 4);
        assert_eq!(mgr.capacity(), 4);

        let token = mgr.store_shared(make_msg(42)).unwrap();
        assert_eq!(mgr.available(), 3);

        {
            let msg = mgr.read_shared(token).unwrap();
            assert_eq!(*msg.payload(), create_test_tensor_filled_with(42));
        }

        mgr.free_shared(token).unwrap();
        assert_eq!(mgr.available(), 4);
    }

    #[test]
    fn read_mut_works() {
        let mgr: ConcurrentMemoryManager<TestTensor> = ConcurrentMemoryManager::new(4);
        let token = mgr.store_shared(make_msg(10)).unwrap();

        {
            // mutable borrow, must be declared mut binding
            let mut msg = mgr.read_mut_shared(token).unwrap();
            *msg.payload_mut() = create_test_tensor_filled_with(99);
            // mutable guard dropped here
        }

        {
            // now we can take an immutable borrow safely
            let msg = mgr.read_shared(token).unwrap();
            assert_eq!(*msg.payload(), create_test_tensor_filled_with(99));
        }

        // free now that no borrows exist
        mgr.free_shared(token).unwrap();
    }

    #[test]
    fn peek_header_works() {
        let mgr: ConcurrentMemoryManager<TestTensor> = ConcurrentMemoryManager::new(4);
        let token = mgr.store_shared(make_msg(7)).unwrap();

        {
            let header = mgr.peek_header(token).unwrap();
            assert_eq!(*header.payload_size_bytes(), TEST_TENSOR_BYTE_COUNT);
            // header dropped at end of scope
        }

        mgr.free_shared(token).unwrap();
    }

    #[test]
    fn capacity_exhaustion() {
        let mgr: ConcurrentMemoryManager<TestTensor> = ConcurrentMemoryManager::new(2);
        let _t0 = mgr.store_shared(make_msg(1)).unwrap();
        let _t1 = mgr.store_shared(make_msg(2)).unwrap();
        assert_eq!(mgr.available(), 0);

        let err = mgr.store_shared(make_msg(3));
        assert_eq!(err, Err(MemoryError::NoFreeSlots));
    }

    #[test]
    fn double_free_detected() {
        let mgr: ConcurrentMemoryManager<TestTensor> = ConcurrentMemoryManager::new(4);
        let token = mgr.store_shared(make_msg(1)).unwrap();
        mgr.free_shared(token).unwrap();

        let err = mgr.free_shared(token);
        assert_eq!(err, Err(MemoryError::NotAllocated));
    }

    #[test]
    fn bad_token_detected() {
        let mgr: ConcurrentMemoryManager<TestTensor> = ConcurrentMemoryManager::new(4);
        let bad = MessageToken::new(99);

        assert!(matches!(mgr.read_shared(bad), Err(MemoryError::BadToken)));
        assert!(matches!(mgr.peek_header(bad), Err(MemoryError::BadToken)));
    }

    #[test]
    fn read_freed_slot_is_bad_token() {
        let mgr: ConcurrentMemoryManager<TestTensor> = ConcurrentMemoryManager::new(4);
        let token = mgr.store_shared(make_msg(1)).unwrap();
        mgr.free_shared(token).unwrap();

        assert!(matches!(
            mgr.read_shared(token),
            Err(MemoryError::NotAllocated)
        ));
        assert!(matches!(
            mgr.peek_header(token),
            Err(MemoryError::NotAllocated)
        ));
    }

    #[test]
    fn slot_reuse_after_free() {
        let mgr: ConcurrentMemoryManager<TestTensor> = ConcurrentMemoryManager::new(1);
        let t0 = mgr.store_shared(make_msg(10)).unwrap();
        mgr.free_shared(t0).unwrap();

        // Slot 0 should be reused.
        let t1 = mgr.store_shared(make_msg(20)).unwrap();
        assert_eq!(t1.index(), 0);
        assert_eq!(
            *mgr.read_shared(t1).unwrap().payload(),
            create_test_tensor_filled_with(20)
        );
    }

    #[test]
    fn memory_class_configurable() {
        let mgr: ConcurrentMemoryManager<TestTensor> =
            ConcurrentMemoryManager::with_memory_class(4, MemoryClass::Device(0));
        assert_eq!(mgr.memory_class(), MemoryClass::Device(0));
    }

    #[test]
    fn default_memory_class_is_host() {
        let mgr: ConcurrentMemoryManager<TestTensor> = ConcurrentMemoryManager::new(4);
        assert_eq!(mgr.memory_class(), MemoryClass::Host);
    }
}
