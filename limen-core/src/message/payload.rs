//! TODO:

use crate::memory::{BufferDescriptor, MemoryClass};

/// Trait for payload types that can provide byte length and memory class.
pub trait Payload {
    /// Return the buffer descriptor (byte size & memory class).
    fn buffer_descriptor(&self) -> BufferDescriptor;
}

impl Payload for [u8] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: self.len(),
            class: MemoryClass::Host,
        }
    }
}

impl<'a> Payload for &'a [u8] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: self.len(),
            class: MemoryClass::Host,
        }
    }
}

impl<const N: usize> Payload for [u8; N] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: N,
            class: MemoryClass::Host,
        }
    }
}

impl<'a, const N: usize> Payload for &'a [u8; N] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: N,
            class: MemoryClass::Host,
        }
    }
}

impl Payload for u32 {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: core::mem::size_of::<u32>(),
            class: MemoryClass::Host,
        }
    }
}

impl<'a> Payload for &'a u32 {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: core::mem::size_of::<u32>(),
            class: MemoryClass::Host,
        }
    }
}

impl Payload for [u32] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: self.len() * core::mem::size_of::<u32>(),
            class: MemoryClass::Host,
        }
    }
}

impl<'a> Payload for &'a [u32] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: self.len() * core::mem::size_of::<u32>(),
            class: MemoryClass::Host,
        }
    }
}

impl<const N: usize> Payload for [u32; N] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: N * core::mem::size_of::<u32>(),
            class: MemoryClass::Host,
        }
    }
}

impl<'a, const N: usize> Payload for &'a [u32; N] {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: N * core::mem::size_of::<u32>(),
            class: MemoryClass::Host,
        }
    }
}

impl Payload for () {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor {
            bytes: 0,
            class: MemoryClass::Host,
        }
    }
}
