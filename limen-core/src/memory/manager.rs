//! Typed token-backed message storage.
//!
//! This module defines [`MemoryManager`], the core storage interface for
//! managers that own complete [`Message<P>`] values and expose them through
//! lightweight [`MessageToken`] handles.
//!
//! A memory manager is responsible for:
//!
//! - allocating storage for new messages,
//! - resolving tokens back to stored messages,
//! - providing shared and exclusive borrows of stored messages,
//! - releasing storage when a token is no longer needed,
//! - reporting capacity and memory-class information.
//!
//! The trait is intentionally typed over a single payload type `P`. A concrete
//! manager instance stores only `Message<P>` values.
//!
//! The API remains small and direct:
//!
//! - [`MemoryManager::store`]
//! - [`MemoryManager::read`]
//! - [`MemoryManager::read_mut`]
//! - [`MemoryManager::free`]
//! - [`MemoryManager::available`]
//! - [`MemoryManager::capacity`]
//! - [`MemoryManager::memory_class`]
//!
//! Shared header access is provided separately through the
//! [`HeaderStore`](crate::memory::header_store::HeaderStore) supertrait.
//!
//! # Guard-based borrows
//!
//! The borrow-returning methods use associated guard types instead of plain
//! references so implementations can support both:
//!
//! - zero-overhead plain references in single-threaded managers, and
//! - slot-level synchronization in concurrent managers.
//!
//! For example:
//!
//! - a static or heap-backed manager may use `&Message<P>` and
//!   `&mut Message<P>` directly;
//! - a concurrent manager may use wrapper types around slot-level read/write
//!   lock guards that dereference to `Message<P>`.
//!
//! This keeps the public API stable while allowing implementations to choose
//! the correct synchronization strategy internally.

use core::ops::{Deref, DerefMut};

use crate::errors::MemoryError;
use crate::memory::header_store::HeaderStore;
use crate::memory::MemoryClass;
use crate::message::payload::Payload;
use crate::message::Message;
use crate::types::MessageToken;

/// Typed storage interface for token-addressed messages.
///
/// A `MemoryManager<P>` owns stored [`Message<P>`] values and provides access
/// to them through stable [`MessageToken`] handles.
///
/// The manager's responsibilities are:
///
/// - storing new messages and returning tokens,
/// - resolving tokens to stored messages,
/// - providing immutable and mutable borrows of stored messages,
/// - freeing storage when a token is no longer live,
/// - reporting capacity and memory-class information.
///
/// The trait is parameterized by `P`, and a single manager instance stores only
/// one concrete payload type.
///
/// # Why `P` is a generic parameter
///
/// `P` is a generic parameter rather than an associated type so that a future
/// implementation can provide `MemoryManager<P>` for multiple payload types via
/// monomorphization without changing the external interface.
///
/// # Borrow model
///
/// `read` and `read_mut` return guards rather than naked references.
///
/// This is necessary because:
///
/// - single-threaded managers should be able to return plain references with
///   no additional runtime overhead;
/// - concurrent managers need the returned borrow to remain tied to a
///   slot-level synchronization guard.
///
/// The associated guard types make both implementation strategies possible with
/// one API.
///
/// # Implementation notes
///
/// Typical implementations:
///
/// - static manager:
///   - `ReadGuard<'a> = &'a Message<P>`
///   - `WriteGuard<'a> = &'a mut Message<P>`
/// - heap manager:
///   - `ReadGuard<'a> = &'a Message<P>`
///   - `WriteGuard<'a> = &'a mut Message<P>`
/// - concurrent manager:
///   - small wrapper types around slot-level read/write lock guards
///
/// # Errors
///
/// Implementations should use [`MemoryError`] variants to report invalid
/// tokens, unallocated slots, active borrows, poisoned synchronization
/// primitives, or exhausted capacity.
///
/// # Safety
///
/// The trait itself is fully safe. Any unsafe code needed by a specific
/// implementation must remain internal to that implementation.
pub trait MemoryManager<P: Payload>: HeaderStore {
    /// Shared read guard returned by [`MemoryManager::read`].
    ///
    /// This guard dereferences to the stored [`Message<P>`].
    ///
    /// Typical implementations:
    ///
    /// - `&'a Message<P>` for single-threaded managers;
    /// - a wrapper around a slot-level read lock guard for concurrent
    ///   managers.
    type ReadGuard<'a>: Deref<Target = Message<P>>
    where
        Self: 'a;

    /// Exclusive mutable guard returned by [`MemoryManager::read_mut`].
    ///
    /// This guard dereferences mutably to the stored [`Message<P>`].
    ///
    /// Typical implementations:
    ///
    /// - `&'a mut Message<P>` for single-threaded managers;
    /// - a wrapper around a slot-level write lock guard for concurrent
    ///   managers.
    type WriteGuard<'a>: DerefMut<Target = Message<P>>
    where
        Self: 'a;

    /// Allocate storage for `value` and return its token.
    ///
    /// The returned [`MessageToken`] becomes the stable handle used to refer to
    /// the stored message.
    ///
    /// # Errors
    ///
    /// Returns:
    ///
    /// - [`MemoryError::NoFreeSlots`] if the manager has no remaining
    ///   capacity;
    /// - [`MemoryError::Poisoned`] if a concurrent implementation cannot
    ///   recover from a poisoned synchronization primitive during allocation.
    fn store(&mut self, value: Message<P>) -> Result<MessageToken, MemoryError>;

    /// Borrow a stored message immutably.
    ///
    /// This is the primary read path for manager-owned messages. The returned
    /// guard provides access to the stored message without copying it.
    ///
    /// # Errors
    ///
    /// Returns:
    ///
    /// - [`MemoryError::BadToken`] if `token` is invalid;
    /// - [`MemoryError::NotAllocated`] if the slot is currently empty;
    /// - [`MemoryError::AlreadyBorrowed`] if the implementation chooses to
    ///   report an incompatible borrow state explicitly;
    /// - [`MemoryError::Poisoned`] if a concurrent implementation encounters a
    ///   poisoned synchronization primitive.
    fn read(&self, token: MessageToken) -> Result<Self::ReadGuard<'_>, MemoryError>;

    /// Borrow a stored message mutably.
    ///
    /// This method provides exclusive mutable access to the stored message
    /// identified by `token`.
    ///
    /// It is intended for in-place mutation of manager-owned messages.
    ///
    /// # Errors
    ///
    /// Returns:
    ///
    /// - [`MemoryError::BadToken`] if `token` is invalid;
    /// - [`MemoryError::NotAllocated`] if the slot is currently empty;
    /// - [`MemoryError::AlreadyBorrowed`] if the slot cannot be mutably
    ///   borrowed because it is already borrowed incompatibly;
    /// - [`MemoryError::Poisoned`] if a concurrent implementation encounters a
    ///   poisoned synchronization primitive.
    fn read_mut(&mut self, token: MessageToken) -> Result<Self::WriteGuard<'_>, MemoryError>;

    /// Free the slot identified by `token`.
    ///
    /// After a successful call, the token must no longer be used to access the
    /// manager.
    ///
    /// # Errors
    ///
    /// Returns:
    ///
    /// - [`MemoryError::BadToken`] if `token` is invalid;
    /// - [`MemoryError::NotAllocated`] if the slot is already empty;
    /// - [`MemoryError::BorrowActive`] if the slot still has active borrows;
    /// - [`MemoryError::QueueOwned`] if the implementation tracks queue
    ///   ownership internally and the token is still considered live in one or
    ///   more queues;
    /// - [`MemoryError::Poisoned`] if a concurrent implementation encounters a
    ///   poisoned synchronization primitive while releasing the slot.
    fn free(&mut self, token: MessageToken) -> Result<(), MemoryError>;

    /// Return the number of currently free slots.
    ///
    /// This is intended for diagnostics, telemetry, and admission decisions.
    fn available(&self) -> usize;

    /// Return the total slot capacity of the manager.
    fn capacity(&self) -> usize;

    /// Return the memory class represented by this manager.
    ///
    /// This value describes the storage domain used by the manager, such as
    /// host memory or another backing class.
    fn memory_class(&self) -> MemoryClass;
}

/// Scoped handle factory for memory managers used in concurrent execution.
///
/// Analogous to [`ScopedEdge`](crate::edge::ScopedEdge) for memory managers.
/// The GAT `Handle<'a>` allows implementations to return either an owned
/// clone (Arc-based) or a borrowed view (future lock-free managers).
#[cfg(feature = "std")]
pub trait ScopedManager<P: crate::message::payload::Payload>:
    crate::memory::manager::MemoryManager<P>
{
    /// Per-worker handle type.
    type Handle<'a>: crate::memory::manager::MemoryManager<P> + Send + 'a
    where
        Self: 'a;

    /// Create a scoped handle for a worker thread.
    fn scoped_handle<'a>(&'a self) -> Self::Handle<'a>
    where
        Self: 'a;
}
