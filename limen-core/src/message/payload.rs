//! Minimal, zero-cost payload descriptors for generic Rust data.

use crate::memory::BufferDescriptor;
use core::mem;

/// Trait for payload types that can provide byte length.
///
/// Memory class (placement) is a property of the [`MemoryManager`], not the
/// payload. The payload only knows its byte size.
pub trait Payload {
    /// Return the buffer descriptor (byte size).
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
