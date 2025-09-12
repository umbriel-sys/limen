//! Payload helpers for processing nodes.
use core::mem::size_of;
use limen_core::message::Payload;
use limen_core::memory::{BufferDescriptor, MemoryClass};

/// A fixed-size 1D tensor payload backed by an array.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Tensor1D<T, const N: usize> {
    /// Elements of the tensor.
    pub data: [T; N],
}

impl<T: Copy, const N: usize> Tensor1D<T, N> {
    /// Construct from an array.
    pub const fn from_array(data: [T; N]) -> Self { Self { data } }
}

impl<T, const N: usize> Payload for Tensor1D<T, N> {
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: size_of::<T>() * N,
            class: MemoryClass::Host,
        }
    }
}

/// A single-byte class label.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Label(pub u8);

impl Payload for Label {
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor { bytes: 1, class: MemoryClass::Host }
    }
}
