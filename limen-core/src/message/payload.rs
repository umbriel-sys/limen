//! The [`Payload`] trait and blanket implementations for common Rust types.
//!
//! A payload type must only report its byte size via [`Payload::buffer_descriptor`].
//! Memory class (placement) is a property of the [`MemoryManager`](crate::memory::manager::MemoryManager),
//! not the payload.
//!
//! Blanket impls are provided for:
//! - slices and slice references (`[T]`, `&[T]`),
//! - fixed-size arrays and their references (`[T; N]`, `&[T; N]`),
//! - unit `()` (zero bytes), and
//! - `u32` (convenience for simple test/source nodes).

use crate::memory::BufferDescriptor;
use core::mem;

/// Trait for payload types that can report their byte size.
///
/// Implementing this trait is the only requirement for a type to be used as
/// a message payload in the graph. Memory class (placement) is tracked by the
/// [`MemoryManager`](crate::memory::manager::MemoryManager), not by the payload itself.
pub trait Payload {
    /// Return a descriptor containing the byte size of this payload.
    fn buffer_descriptor(&self) -> BufferDescriptor;
}

/* ---------- Generic slices & arrays (cover all element types T) ---------- */

#[allow(clippy::manual_slice_size_calculation)]
impl<T> Payload for [T] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor::new(self.len() * mem::size_of::<T>())
    }
}

#[allow(clippy::needless_lifetimes, clippy::manual_slice_size_calculation)]
impl<'a, T> Payload for &'a [T] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor::new(self.len() * mem::size_of::<T>())
    }
}

impl<T, const N: usize> Payload for [T; N] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor::new(N * mem::size_of::<T>())
    }
}

#[allow(clippy::needless_lifetimes)]
impl<'a, T, const N: usize> Payload for &'a [T; N] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor::new(N * mem::size_of::<T>())
    }
}

/* ------------------------- Common scalar payloads ------------------------ */

impl Payload for () {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor::new(0)
    }
}

impl Payload for u32 {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor::new(mem::size_of::<u32>())
    }
}
