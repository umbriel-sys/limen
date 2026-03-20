//! Read-only header access for token-backed message storage.
//!
//! This module defines [`HeaderStore`], a small non-generic abstraction for
//! retrieving the [`MessageHeader`] associated with a stored [`MessageToken`].
//!
//! The trait is intended for components that need to inspect message metadata
//! without knowing the payload type of the stored message. Typical consumers
//! include queueing, scheduling, admission-control, and byte-accounting code.
//!
//! Implementations may be fully single-threaded or concurrent:
//!
//! - single-threaded managers can usually return plain shared references;
//! - concurrent managers can return lightweight guard wrappers that hold the
//!   slot-level synchronization primitive for the duration of the borrow.
//!
//! The API is intentionally read-only. Mutation of stored messages belongs to
//! the typed [`MemoryManager`](crate::memory::manager::MemoryManager) interface.

use core::ops::Deref;

use crate::errors::MemoryError;
use crate::message::MessageHeader;
use crate::types::MessageToken;

/// Read-only header access for token-addressed messages.
///
/// `HeaderStore` allows callers to inspect a stored message's header without
/// knowing the message's payload type.
///
/// This is primarily useful for subsystems that make decisions based only on
/// metadata, such as:
///
/// - queue admission and eviction policies,
/// - scheduling,
/// - byte accounting,
/// - QoS- or deadline-aware routing.
///
/// The trait resolves a [`MessageToken`] to its corresponding header and
/// returns a guard that dereferences to [`MessageHeader`].
///
/// # Guard semantics
///
/// The returned guard must keep the referenced header valid for the full
/// lifetime of the guard.
///
/// Typical implementations:
///
/// - a single-threaded manager may use `&MessageHeader`;
/// - a concurrent manager may use a wrapper around a slot-level read lock
///   guard that dereferences to `&MessageHeader`.
///
/// # Errors
///
/// Implementations should return:
///
/// - [`MemoryError::BadToken`] if the token does not identify a valid slot;
/// - [`MemoryError::NotAllocated`] if the slot exists but is currently empty;
/// - [`MemoryError::Poisoned`] if a concurrent implementation cannot recover
///   from a poisoned synchronization primitive.
///
/// # Concurrency
///
/// Concurrent implementations should protect only the slot identified by
/// `token`. Looking up one header should not require locking the entire
/// manager.
pub trait HeaderStore {
    /// Guard type returned by [`HeaderStore::peek_header`].
    ///
    /// This guard dereferences to the [`MessageHeader`] associated with the
    /// stored message identified by a token.
    type HeaderGuard<'a>: Deref<Target = MessageHeader>
    where
        Self: 'a;

    /// Borrow the header of the message identified by `token`.
    ///
    /// This method provides read-only access to message metadata without
    /// requiring knowledge of the payload type stored in the manager.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// - `token` is invalid,
    /// - the target slot is unallocated,
    /// - or a concurrent implementation encounters a poisoned lock.
    fn peek_header(&self, token: MessageToken) -> Result<Self::HeaderGuard<'_>, MemoryError>;
}
