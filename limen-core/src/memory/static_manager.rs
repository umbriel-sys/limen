//! Fixed-capacity, `no_std`, `no_alloc` memory manager.
//!
//! Feature `checked-memory-manager-refs` enables per-slot runtime borrow-state
//! checking and guard types that increment/decrement the borrow counter.
//!
//! Default (no feature): plain references are returned: zero-cost for MCU.

use crate::errors::MemoryError;
use crate::memory::header_store::HeaderStore;
use crate::memory::manager::MemoryManager;
use crate::memory::MemoryClass;
use crate::message::payload::Payload;
use crate::message::{Message, MessageHeader};
use crate::types::MessageToken;

#[cfg(feature = "checked-memory-manager-refs")]
use core::cell::Cell;

#[cfg(feature = "checked-memory-manager-refs")]
use core::ops::{Deref, DerefMut};

/// A fixed-capacity memory manager backed by `[Option<Message<P>>; DEPTH]`.
///
/// Behavior depends on the `checked-memory-manager-refs` feature:
///
/// - **Feature disabled (default)**: guard associated types are plain references:
///   `&MessageHeader`, `&Message<P>`, `&mut Message<P>` — zero-cost, suitable
///   for MCU/no-alloc builds.
/// - **Feature enabled**: per-slot borrow counters are kept (u16). `peek_header`,
///   `read`, `read_mut` return small guard types that update the per-slot
///   borrow counter and restore it on `Drop`. Conflicts return explicit
///   `MemoryError` values.
pub struct StaticMemoryManager<P: Payload, const DEPTH: usize> {
    slots: [Option<Message<P>>; DEPTH],
    #[cfg(feature = "checked-memory-manager-refs")]
    borrow_states: [Cell<u16>; DEPTH], // u16 is small and OK for typical DEPTH <= 65535
    mem_class: MemoryClass,
}

impl<P: Payload, const DEPTH: usize> StaticMemoryManager<P, DEPTH> {
    /// Create a new manager with all slots empty.
    ///
    /// `memory_class` defaults to [`MemoryClass::Host`].
    pub fn new() -> Self {
        Self {
            slots: core::array::from_fn(|_| None),
            #[cfg(feature = "checked-memory-manager-refs")]
            borrow_states: core::array::from_fn(|_| Cell::new(0)),
            mem_class: MemoryClass::Host,
        }
    }

    /// Create a new manager with a specific memory class.
    pub fn with_memory_class(mem_class: MemoryClass) -> Self {
        Self {
            slots: core::array::from_fn(|_| None),
            #[cfg(feature = "checked-memory-manager-refs")]
            borrow_states: core::array::from_fn(|_| Cell::new(0)),
            mem_class,
        }
    }
}

impl<P: Payload, const DEPTH: usize> Default for StaticMemoryManager<P, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

//
// ===== Implementation when checked refs are disabled (zero-cost path) =====
//

#[cfg(not(feature = "checked-memory-manager-refs"))]
mod unchecked {
    use super::*;

    impl<P: Payload, const DEPTH: usize> HeaderStore for StaticMemoryManager<P, DEPTH> {
        type HeaderGuard<'a>
            = &'a MessageHeader
        where
            Self: 'a;

        fn peek_header(&self, token: MessageToken) -> Result<Self::HeaderGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= DEPTH {
                return Err(MemoryError::BadToken);
            }
            // Per tests/contract: treat unallocated slot as BadToken for read/peek.
            self.slots[idx]
                .as_ref()
                .map(|msg| msg.header())
                .ok_or(MemoryError::BadToken)
        }
    }

    impl<P: Payload, const DEPTH: usize> MemoryManager<P> for StaticMemoryManager<P, DEPTH> {
        type ReadGuard<'a>
            = &'a Message<P>
        where
            Self: 'a;

        type WriteGuard<'a>
            = &'a mut Message<P>
        where
            Self: 'a;

        fn store(&mut self, value: Message<P>) -> Result<MessageToken, MemoryError> {
            for (i, slot) in self.slots.iter_mut().enumerate() {
                if slot.is_none() {
                    *slot = Some(value);
                    return Ok(MessageToken::new(i as u32));
                }
            }
            Err(MemoryError::NoFreeSlots)
        }

        fn read(&self, token: MessageToken) -> Result<Self::ReadGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= DEPTH {
                return Err(MemoryError::BadToken);
            }
            // Per tests/contract: unallocated slot -> BadToken
            self.slots[idx].as_ref().ok_or(MemoryError::BadToken)
        }

        fn read_mut(&mut self, token: MessageToken) -> Result<Self::WriteGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= DEPTH {
                return Err(MemoryError::BadToken);
            }
            // unallocated slot -> BadToken (consistent with read)
            self.slots[idx].as_mut().ok_or(MemoryError::BadToken)
        }

        fn free(&mut self, token: MessageToken) -> Result<(), MemoryError> {
            let idx = token.index();
            if idx >= DEPTH {
                return Err(MemoryError::BadToken);
            }
            if self.slots[idx].is_none() {
                // Freeing an already-empty slot is NotAllocated per tests.
                return Err(MemoryError::NotAllocated);
            }
            self.slots[idx] = None;
            Ok(())
        }

        fn available(&self) -> usize {
            self.slots.iter().filter(|s| s.is_none()).count()
        }

        fn capacity(&self) -> usize {
            DEPTH
        }

        fn memory_class(&self) -> MemoryClass {
            self.mem_class
        }
    }
}

//
// ===== Implementation when checked refs are enabled =====
//

#[cfg(feature = "checked-memory-manager-refs")]
mod checked {
    use super::*;

    // Borrow-state representation
    //
    // Convention:
    // - 0 => unborrowed
    // - 1..=u16::MAX - 1 => read borrow count
    // - u16::MAX => write borrowed (exclusive)
    const WRITE_BORROW_MARK: u16 = u16::MAX;

    fn try_increment_read(cell: &Cell<u16>) -> Result<(), MemoryError> {
        let value = cell.get();
        if value == WRITE_BORROW_MARK {
            return Err(MemoryError::AlreadyBorrowed);
        }
        if value == WRITE_BORROW_MARK - 1 {
            return Err(MemoryError::AlreadyBorrowed);
        }
        cell.set(value + 1);
        Ok(())
    }

    fn decrement_read(cell: &Cell<u16>) {
        let v = cell.get();
        if v == 0 || v == WRITE_BORROW_MARK {
            // Recover to zero defensively
            cell.set(0);
        } else {
            cell.set(v - 1);
        }
    }

    fn try_set_write(cell: &Cell<u16>) -> Result<(), MemoryError> {
        if cell.get() != 0 {
            return Err(MemoryError::AlreadyBorrowed);
        }
        cell.set(WRITE_BORROW_MARK);
        Ok(())
    }

    fn clear_write(cell: &Cell<u16>) {
        cell.set(0);
    }

    // Header guard
    pub struct StaticHeaderGuard<'a> {
        header: &'a MessageHeader,
        borrow_state: &'a Cell<u16>,
    }

    impl<'a> Deref for StaticHeaderGuard<'a> {
        type Target = MessageHeader;
        fn deref(&self) -> &Self::Target {
            self.header
        }
    }

    impl<'a> Drop for StaticHeaderGuard<'a> {
        fn drop(&mut self) {
            decrement_read(self.borrow_state);
        }
    }

    // Read guard
    pub struct StaticReadGuard<'a, P: Payload> {
        msg: &'a Message<P>,
        borrow_state: &'a Cell<u16>,
    }

    impl<'a, P: Payload> Deref for StaticReadGuard<'a, P> {
        type Target = Message<P>;
        fn deref(&self) -> &Self::Target {
            self.msg
        }
    }

    impl<'a, P: Payload> Drop for StaticReadGuard<'a, P> {
        fn drop(&mut self) {
            decrement_read(self.borrow_state);
        }
    }

    // Write guard
    pub struct StaticWriteGuard<'a, P: Payload> {
        msg: &'a mut Message<P>,
        borrow_state: &'a Cell<u16>,
    }

    impl<'a, P: Payload> Deref for StaticWriteGuard<'a, P> {
        type Target = Message<P>;
        fn deref(&self) -> &Self::Target {
            self.msg
        }
    }

    impl<'a, P: Payload> DerefMut for StaticWriteGuard<'a, P> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.msg
        }
    }

    impl<'a, P: Payload> Drop for StaticWriteGuard<'a, P> {
        fn drop(&mut self) {
            clear_write(self.borrow_state);
        }
    }

    impl<P: Payload, const DEPTH: usize> HeaderStore for StaticMemoryManager<P, DEPTH> {
        type HeaderGuard<'a>
            = StaticHeaderGuard<'a>
        where
            Self: 'a;

        fn peek_header(&self, token: MessageToken) -> Result<Self::HeaderGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= DEPTH {
                return Err(MemoryError::BadToken);
            }

            match self.slots[idx].as_ref() {
                Some(msg) => {
                    try_increment_read(&self.borrow_states[idx])?;
                    Ok(StaticHeaderGuard {
                        header: msg.header(),
                        borrow_state: &self.borrow_states[idx],
                    })
                }
                None => Err(MemoryError::BadToken),
            }
        }
    }

    impl<P: Payload, const DEPTH: usize> MemoryManager<P> for StaticMemoryManager<P, DEPTH> {
        type ReadGuard<'a>
            = StaticReadGuard<'a, P>
        where
            Self: 'a;

        type WriteGuard<'a>
            = StaticWriteGuard<'a, P>
        where
            Self: 'a;

        fn store(&mut self, value: Message<P>) -> Result<MessageToken, MemoryError> {
            for (i, slot) in self.slots.iter_mut().enumerate() {
                if slot.is_none() {
                    slot.replace(value);
                    // ensure borrow state's cleared
                    self.borrow_states[i].set(0);
                    return Ok(MessageToken::new(i as u32));
                }
            }
            Err(MemoryError::NoFreeSlots)
        }

        fn read(&self, token: MessageToken) -> Result<Self::ReadGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= DEPTH {
                return Err(MemoryError::BadToken);
            }

            match self.slots[idx].as_ref() {
                Some(msg) => {
                    try_increment_read(&self.borrow_states[idx])?;
                    Ok(StaticReadGuard {
                        msg,
                        borrow_state: &self.borrow_states[idx],
                    })
                }
                None => Err(MemoryError::BadToken),
            }
        }

        fn read_mut(&mut self, token: MessageToken) -> Result<Self::WriteGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= DEPTH {
                return Err(MemoryError::BadToken);
            }

            match self.slots[idx].as_mut() {
                Some(msg) => {
                    try_set_write(&self.borrow_states[idx])?;
                    Ok(StaticWriteGuard {
                        msg,
                        borrow_state: &self.borrow_states[idx],
                    })
                }
                None => Err(MemoryError::BadToken),
            }
        }

        fn free(&mut self, token: MessageToken) -> Result<(), MemoryError> {
            let idx = token.index();
            if idx >= DEPTH {
                return Err(MemoryError::BadToken);
            }

            if self.slots[idx].is_none() {
                return Err(MemoryError::NotAllocated);
            }

            let state = self.borrow_states[idx].get();
            if state != 0 {
                return Err(MemoryError::BorrowActive);
            }

            self.slots[idx] = None;
            self.borrow_states[idx].set(0);
            Ok(())
        }

        fn available(&self) -> usize {
            self.slots.iter().filter(|s| s.is_none()).count()
        }

        fn capacity(&self) -> usize {
            DEPTH
        }

        fn memory_class(&self) -> MemoryClass {
            self.mem_class
        }
    }
}

#[cfg(test)]
mod tests {
    // ---------------------------------------------------------------------------
    // Planned memory manager tests — NOT YET IMPLEMENTED
    //
    // C1 / tensor types (planned/C1.md):
    //   - run_store_retrieve_tensor_payload: store a TensorPayload (GPU buffer
    //     descriptor + shape), verify peek_header returns the correct byte count
    //     and memory_class == MemoryClass::Device(0).
    //   - run_free_device_tensor_does_not_double_free: allocate two tensor tokens,
    //     free the first, verify the second is still valid.
    //
    // P1 (PlatformBackend — planned/P1.md):
    //   If the manager gains a device-side allocator backend:
    //   - run_store_on_device_backend_respects_memory_class: store with
    //     MemoryClass::Device(0) and verify the returned token's header reports
    //     the correct class.
    //
    // RS1 (runtime lifecycle — planned/RS1.md):
    //   - run_reset_frees_all_slots: after a runtime reset, all manager slots
    //     must be free and used_slots() must return 0.
    // ---------------------------------------------------------------------------

    use super::*;
    use crate::message::MessageHeader;

    // Helper: build a simple Message<u32>.
    fn make_msg(val: u32) -> Message<u32> {
        Message::new(MessageHeader::empty(), val)
    }

    #[test]
    fn store_read_free_cycle() {
        let mut mgr: StaticMemoryManager<u32, 4> = StaticMemoryManager::new();
        assert_eq!(mgr.available(), 4);
        assert_eq!(mgr.capacity(), 4);

        let token = mgr.store(make_msg(42)).unwrap();
        assert_eq!(mgr.available(), 3);

        {
            let msg = mgr.read(token).unwrap();
            assert_eq!(*msg.payload(), 42);
        }

        mgr.free(token).unwrap();
        assert_eq!(mgr.available(), 4);
    }

    #[test]
    fn read_mut_works() {
        let mut mgr: StaticMemoryManager<u32, 4> = StaticMemoryManager::new();
        let token = mgr.store(make_msg(10)).unwrap();

        {
            let mut write_guard = mgr.read_mut(token).unwrap();
            let msg = core::ops::DerefMut::deref_mut(&mut write_guard);
            *msg.payload_mut() = 99;
        }

        {
            let msg = mgr.read(token).unwrap();
            assert_eq!(*msg.payload(), 99);
        }

        mgr.free(token).unwrap();
    }

    #[test]
    fn peek_header_works() {
        let mut mgr: StaticMemoryManager<u32, 4> = StaticMemoryManager::new();
        let token = mgr.store(make_msg(7)).unwrap();

        {
            let header = mgr.peek_header(token).unwrap();
            assert_eq!(*header.payload_size_bytes(), core::mem::size_of::<u32>());
        }

        mgr.free(token).unwrap();
    }

    #[test]
    fn capacity_exhaustion() {
        let mut mgr: StaticMemoryManager<u32, 2> = StaticMemoryManager::new();
        let _t0 = mgr.store(make_msg(1)).unwrap();
        let _t1 = mgr.store(make_msg(2)).unwrap();
        assert_eq!(mgr.available(), 0);

        let err = mgr.store(make_msg(3));
        assert_eq!(err, Err(MemoryError::NoFreeSlots));
    }

    #[test]
    fn double_free_detected() {
        let mut mgr: StaticMemoryManager<u32, 4> = StaticMemoryManager::new();
        let token = mgr.store(make_msg(1)).unwrap();
        mgr.free(token).unwrap();

        let err = mgr.free(token);
        assert_eq!(err, Err(MemoryError::NotAllocated));
    }

    #[test]
    fn bad_token_detected() {
        let mgr: StaticMemoryManager<u32, 4> = StaticMemoryManager::new();
        let bad = MessageToken::new(99);

        assert!(matches!(mgr.read(bad), Err(MemoryError::BadToken)));
        assert!(matches!(mgr.peek_header(bad), Err(MemoryError::BadToken)));
    }

    #[test]
    fn read_freed_slot_is_bad_token() {
        let mut mgr: StaticMemoryManager<u32, 4> = StaticMemoryManager::new();
        let token = mgr.store(make_msg(1)).unwrap();
        mgr.free(token).unwrap();

        assert!(matches!(mgr.read(token), Err(MemoryError::BadToken)));
        assert!(matches!(mgr.peek_header(token), Err(MemoryError::BadToken)));
    }

    #[test]
    fn slot_reuse_after_free() {
        let mut mgr: StaticMemoryManager<u32, 1> = StaticMemoryManager::new();
        let t0 = mgr.store(make_msg(10)).unwrap();
        mgr.free(t0).unwrap();

        // Slot 0 should be reused.
        let t1 = mgr.store(make_msg(20)).unwrap();
        assert_eq!(t1.index(), 0);
        assert_eq!(*mgr.read(t1).unwrap().payload(), 20);
    }

    #[test]
    fn memory_class_configurable() {
        let mgr: StaticMemoryManager<u32, 4> =
            StaticMemoryManager::with_memory_class(MemoryClass::Device(0));
        assert_eq!(mgr.memory_class(), MemoryClass::Device(0));
    }

    #[test]
    fn default_memory_class_is_host() {
        let mgr: StaticMemoryManager<u32, 4> = StaticMemoryManager::new();
        assert_eq!(mgr.memory_class(), MemoryClass::Host);
    }
}
