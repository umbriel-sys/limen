//! Heap-backed fixed-capacity memory manager.
//!
//! Provides a fixed-capacity heap-backed manager that stores `Message<P>`
//! values in a `Vec<Option<Message<P>>>`. The manager supports the same
//! `checked-memory-manager-refs` feature as `StaticMemoryManager`:
//!
//! - Feature **disabled**: returns plain references (zero-cost).
//! - Feature **enabled**: returns guard types and keeps per-slot borrow counters.

extern crate alloc;

use alloc::vec::Vec;

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

/// A heap-backed fixed-capacity memory manager.
pub struct HeapMemoryManager<P: Payload> {
    slots: Vec<Option<Message<P>>>,
    #[cfg(feature = "checked-memory-manager-refs")]
    borrow_states: Vec<Cell<u16>>,
    mem_class: MemoryClass,
}

impl<P: Payload> HeapMemoryManager<P> {
    /// Create a new manager with given fixed `capacity`.
    ///
    /// The manager pre-allocates `capacity` slots, all initially `None`.
    /// `memory_class` defaults to `MemoryClass::Host`.
    pub fn new(capacity: usize) -> Self {
        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(None);
        }

        #[cfg(feature = "checked-memory-manager-refs")]
        let borrow_states = {
            let mut v = Vec::with_capacity(capacity);
            for _ in 0..capacity {
                v.push(Cell::new(0));
            }
            v
        };

        Self {
            slots,
            #[cfg(feature = "checked-memory-manager-refs")]
            borrow_states,
            mem_class: MemoryClass::Host,
        }
    }

    /// Create a new manager with given `capacity` and explicit `memory_class`.
    pub fn with_memory_class(capacity: usize, mem_class: MemoryClass) -> Self {
        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(None);
        }

        #[cfg(feature = "checked-memory-manager-refs")]
        let borrow_states = {
            let mut v = Vec::with_capacity(capacity);
            for _ in 0..capacity {
                v.push(Cell::new(0));
            }
            v
        };

        Self {
            slots,
            #[cfg(feature = "checked-memory-manager-refs")]
            borrow_states,
            mem_class,
        }
    }

    /// Return the configured capacity.
    pub fn configured_capacity(&self) -> usize {
        self.slots.len()
    }
}

//
// ===== Implementation when checked refs are disabled (zero-cost path) =====
//

#[cfg(not(feature = "checked-memory-manager-refs"))]
mod unchecked {
    use super::*;

    impl<P: Payload> HeaderStore for HeapMemoryManager<P> {
        type HeaderGuard<'a>
            = &'a MessageHeader
        where
            Self: 'a;

        fn peek_header(&self, token: MessageToken) -> Result<Self::HeaderGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= self.slots.len() {
                return Err(MemoryError::BadToken);
            }
            self.slots[idx]
                .as_ref()
                .map(|m| m.header())
                .ok_or(MemoryError::BadToken)
        }
    }

    impl<P: Payload> MemoryManager<P> for HeapMemoryManager<P> {
        type ReadGuard<'a>
            = &'a Message<P>
        where
            Self: 'a;

        type WriteGuard<'a>
            = &'a mut Message<P>
        where
            Self: 'a;

        fn store(&mut self, value: Message<P>) -> Result<MessageToken, MemoryError> {
            // Find first free slot
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
            if idx >= self.slots.len() {
                return Err(MemoryError::BadToken);
            }
            self.slots[idx].as_ref().ok_or(MemoryError::BadToken)
        }

        fn read_mut(&mut self, token: MessageToken) -> Result<Self::WriteGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= self.slots.len() {
                return Err(MemoryError::BadToken);
            }
            self.slots[idx].as_mut().ok_or(MemoryError::BadToken)
        }

        fn free(&mut self, token: MessageToken) -> Result<(), MemoryError> {
            let idx = token.index();
            if idx >= self.slots.len() {
                return Err(MemoryError::BadToken);
            }
            if self.slots[idx].is_none() {
                return Err(MemoryError::NotAllocated);
            }
            self.slots[idx] = None;
            Ok(())
        }

        fn available(&self) -> usize {
            self.slots.iter().filter(|s| s.is_none()).count()
        }

        fn capacity(&self) -> usize {
            self.slots.len()
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
    use core::marker::PhantomData;

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
    pub struct HeapHeaderGuard<'a> {
        header: &'a MessageHeader,
        borrow_state: &'a Cell<u16>,
    }

    impl<'a> Deref for HeapHeaderGuard<'a> {
        type Target = MessageHeader;
        fn deref(&self) -> &Self::Target {
            self.header
        }
    }

    impl<'a> Drop for HeapHeaderGuard<'a> {
        fn drop(&mut self) {
            decrement_read(self.borrow_state);
        }
    }

    // Read guard
    pub struct HeapReadGuard<'a, P: Payload> {
        msg: &'a Message<P>,
        borrow_state: &'a Cell<u16>,
    }

    impl<'a, P: Payload> Deref for HeapReadGuard<'a, P> {
        type Target = Message<P>;
        fn deref(&self) -> &Self::Target {
            self.msg
        }
    }

    impl<'a, P: Payload> Drop for HeapReadGuard<'a, P> {
        fn drop(&mut self) {
            decrement_read(self.borrow_state);
        }
    }

    // Write guard
    pub struct HeapWriteGuard<'a, P: Payload> {
        msg: &'a mut Message<P>,
        borrow_state: &'a Cell<u16>,
        _phantom: PhantomData<&'a mut Message<P>>,
    }

    impl<'a, P: Payload> Deref for HeapWriteGuard<'a, P> {
        type Target = Message<P>;
        fn deref(&self) -> &Self::Target {
            self.msg
        }
    }

    impl<'a, P: Payload> DerefMut for HeapWriteGuard<'a, P> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.msg
        }
    }

    impl<'a, P: Payload> Drop for HeapWriteGuard<'a, P> {
        fn drop(&mut self) {
            clear_write(self.borrow_state);
        }
    }

    impl<P: Payload> HeaderStore for HeapMemoryManager<P> {
        type HeaderGuard<'a>
            = HeapHeaderGuard<'a>
        where
            Self: 'a;

        fn peek_header(&self, token: MessageToken) -> Result<Self::HeaderGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= self.slots.len() {
                return Err(MemoryError::BadToken);
            }

            match self.slots[idx].as_ref() {
                Some(msg) => {
                    try_increment_read(&self.borrow_states[idx])?;
                    Ok(HeapHeaderGuard {
                        header: msg.header(),
                        borrow_state: &self.borrow_states[idx],
                    })
                }
                None => Err(MemoryError::BadToken),
            }
        }
    }

    impl<P: Payload> MemoryManager<P> for HeapMemoryManager<P> {
        type ReadGuard<'a>
            = HeapReadGuard<'a, P>
        where
            Self: 'a;

        type WriteGuard<'a>
            = HeapWriteGuard<'a, P>
        where
            Self: 'a;

        fn store(&mut self, value: Message<P>) -> Result<MessageToken, MemoryError> {
            for (i, slot) in self.slots.iter_mut().enumerate() {
                if slot.is_none() {
                    slot.replace(value);
                    // ensure borrow state cleared
                    self.borrow_states[i].set(0);
                    return Ok(MessageToken::new(i as u32));
                }
            }
            Err(MemoryError::NoFreeSlots)
        }

        fn read(&self, token: MessageToken) -> Result<Self::ReadGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= self.slots.len() {
                return Err(MemoryError::BadToken);
            }

            match self.slots[idx].as_ref() {
                Some(msg) => {
                    try_increment_read(&self.borrow_states[idx])?;
                    Ok(HeapReadGuard {
                        msg,
                        borrow_state: &self.borrow_states[idx],
                    })
                }
                None => Err(MemoryError::BadToken),
            }
        }

        fn read_mut(&mut self, token: MessageToken) -> Result<Self::WriteGuard<'_>, MemoryError> {
            let idx = token.index();
            if idx >= self.slots.len() {
                return Err(MemoryError::BadToken);
            }

            match self.slots[idx].as_mut() {
                Some(msg) => {
                    try_set_write(&self.borrow_states[idx])?;
                    Ok(HeapWriteGuard {
                        msg,
                        borrow_state: &self.borrow_states[idx],
                        _phantom: PhantomData,
                    })
                }
                None => Err(MemoryError::BadToken),
            }
        }

        fn free(&mut self, token: MessageToken) -> Result<(), MemoryError> {
            let idx = token.index();
            if idx >= self.slots.len() {
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
            self.slots.len()
        }

        fn memory_class(&self) -> MemoryClass {
            self.mem_class
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        message::MessageHeader,
        prelude::{create_test_tensor_filled_with, TestTensor, TEST_TENSOR_BYTE_COUNT},
    };

    // Helper: build a simple Message<TestTensor>.
    fn make_msg(val: u32) -> Message<TestTensor> {
        Message::new(MessageHeader::empty(), create_test_tensor_filled_with(val))
    }

    #[test]
    fn store_read_free_cycle() {
        let mut mgr: HeapMemoryManager<TestTensor> = HeapMemoryManager::new(4);
        assert_eq!(mgr.available(), 4);
        assert_eq!(mgr.capacity(), 4);

        let token = mgr.store(make_msg(42)).unwrap();
        assert_eq!(mgr.available(), 3);

        {
            let msg = mgr.read(token).unwrap();
            assert_eq!(*msg.payload(), create_test_tensor_filled_with(42));
        }

        mgr.free(token).unwrap();
        assert_eq!(mgr.available(), 4);
    }

    #[test]
    fn read_mut_works() {
        let mut mgr: HeapMemoryManager<TestTensor> = HeapMemoryManager::new(4);
        let token = mgr.store(make_msg(10)).unwrap();

        {
            let mut write_guard = mgr.read_mut(token).unwrap();
            let msg = core::ops::DerefMut::deref_mut(&mut write_guard);
            *msg.payload_mut() = create_test_tensor_filled_with(99);
        }

        {
            let msg = mgr.read(token).unwrap();
            assert_eq!(*msg.payload(), create_test_tensor_filled_with(99));
        }

        mgr.free(token).unwrap();
    }

    #[test]
    fn peek_header_works() {
        let mut mgr: HeapMemoryManager<TestTensor> = HeapMemoryManager::new(4);
        let token = mgr.store(make_msg(7)).unwrap();

        {
            let header = mgr.peek_header(token).unwrap();
            assert_eq!(*header.payload_size_bytes(), TEST_TENSOR_BYTE_COUNT);
        }

        mgr.free(token).unwrap();
    }

    #[test]
    fn capacity_exhaustion() {
        let mut mgr: HeapMemoryManager<TestTensor> = HeapMemoryManager::new(2);
        let _t0 = mgr.store(make_msg(1)).unwrap();
        let _t1 = mgr.store(make_msg(2)).unwrap();
        assert_eq!(mgr.available(), 0);

        let err = mgr.store(make_msg(3));
        assert_eq!(err, Err(MemoryError::NoFreeSlots));
    }

    #[test]
    fn double_free_detected() {
        let mut mgr: HeapMemoryManager<TestTensor> = HeapMemoryManager::new(4);
        let token = mgr.store(make_msg(1)).unwrap();
        mgr.free(token).unwrap();

        let err = mgr.free(token);
        assert_eq!(err, Err(MemoryError::NotAllocated));
    }

    #[test]
    fn bad_token_detected() {
        let mgr: HeapMemoryManager<TestTensor> = HeapMemoryManager::new(4);
        let bad = MessageToken::new(99);

        assert!(matches!(mgr.read(bad), Err(MemoryError::BadToken)));
        assert!(matches!(mgr.peek_header(bad), Err(MemoryError::BadToken)));
    }

    #[test]
    fn read_freed_slot_is_bad_token() {
        let mut mgr: HeapMemoryManager<TestTensor> = HeapMemoryManager::new(4);
        let token = mgr.store(make_msg(1)).unwrap();
        mgr.free(token).unwrap();

        assert!(matches!(mgr.read(token), Err(MemoryError::BadToken)));
        assert!(matches!(mgr.peek_header(token), Err(MemoryError::BadToken)));
    }

    #[test]
    fn slot_reuse_after_free() {
        let mut mgr: HeapMemoryManager<TestTensor> = HeapMemoryManager::new(4);
        let t0 = mgr.store(make_msg(10)).unwrap();
        mgr.free(t0).unwrap();

        // Slot 0 should be reused.
        let t1 = mgr.store(make_msg(20)).unwrap();
        assert_eq!(t1.index(), 0);
        assert_eq!(
            *mgr.read(t1).unwrap().payload(),
            create_test_tensor_filled_with(20)
        );
    }

    #[test]
    fn memory_class_configurable() {
        let mgr: HeapMemoryManager<TestTensor> =
            HeapMemoryManager::with_memory_class(4, MemoryClass::Device(0));
        assert_eq!(mgr.memory_class(), MemoryClass::Device(0));
    }

    #[test]
    fn default_memory_class_is_host() {
        let mgr: HeapMemoryManager<TestTensor> = HeapMemoryManager::new(4);
        assert_eq!(mgr.memory_class(), MemoryClass::Host);
    }
}
