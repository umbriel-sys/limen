//! Borrowed tensor wrappers with explicit `DataType`, `shape`, and `MemoryClass`.
//!
//! Design goals:
//! - **No unsafe**, **no allocation** (borrows typed slices).
//! - Typed accessors per scalar kind (e.g., `as_u8`, `as_f32`, `as_f16`, …).
//! - Simple, unified interface for nodes that need tensors and queues that only
//!   care about payload bytes and placement via the `Payload` trait.
//!
//! Representation notes:
//! - We store elements as a typed slice enum rather than `&[u8]` to avoid
//!   any pointer casting.
//! - `byte_len()` is computed from the typed slice length and the element size.
//! - `is_compatible()` checks that `shape`’s product matches the element count.

use core::mem;

use crate::memory::{BufferDescriptor, MemoryClass};
use crate::message::payload::Payload;
use crate::types::{DataType, BF16, F16};

/// Borrowed, typed element storage for a tensor.
///
/// Each variant holds a borrowed, contiguous slice of the corresponding scalar type.
/// No allocation or copying is performed.
#[derive(Debug, Clone, Copy)]
pub enum TensorElements<'a> {
    /// Slice of boolean elements.
    Boolean(&'a [bool]),
    /// Slice of unsigned 8-bit integers.
    Unsigned8(&'a [u8]),
    /// Slice of unsigned 16-bit integers.
    Unsigned16(&'a [u16]),
    /// Slice of unsigned 32-bit integers.
    Unsigned32(&'a [u32]),
    /// Slice of unsigned 64-bit integers.
    Unsigned64(&'a [u64]),
    /// Slice of signed 8-bit integers.
    Signed8(&'a [i8]),
    /// Slice of signed 16-bit integers.
    Signed16(&'a [i16]),
    /// Slice of signed 32-bit integers.
    Signed32(&'a [i32]),
    /// Slice of signed 64-bit integers.
    Signed64(&'a [i64]),
    /// Slice of IEEE-754 binary16 values.
    Float16(&'a [F16]),
    /// Slice of bfloat16 values.
    BFloat16(&'a [BF16]),
    /// Slice of 32-bit IEEE-754 floats.
    Float32(&'a [f32]),
    /// Slice of 64-bit IEEE-754 floats.
    Float64(&'a [f64]),
}

impl<'a> TensorElements<'a> {
    /// Return the logical element `DataType`.
    #[inline]
    pub fn data_type(&self) -> DataType {
        match self {
            Self::Boolean(_) => DataType::Boolean,
            Self::Unsigned8(_) => DataType::Unsigned8,
            Self::Unsigned16(_) => DataType::Unsigned16,
            Self::Unsigned32(_) => DataType::Unsigned32,
            Self::Unsigned64(_) => DataType::Unsigned64,
            Self::Signed8(_) => DataType::Signed8,
            Self::Signed16(_) => DataType::Signed16,
            Self::Signed32(_) => DataType::Signed32,
            Self::Signed64(_) => DataType::Signed64,
            Self::Float16(_) => DataType::Float16,
            Self::BFloat16(_) => DataType::BFloat16,
            Self::Float32(_) => DataType::Float32,
            Self::Float64(_) => DataType::Float64,
        }
    }

    /// Number of elements stored.
    #[inline]
    pub fn len_elems(&self) -> usize {
        match self {
            Self::Boolean(s) => s.len(),
            Self::Unsigned8(s) => s.len(),
            Self::Unsigned16(s) => s.len(),
            Self::Unsigned32(s) => s.len(),
            Self::Unsigned64(s) => s.len(),
            Self::Signed8(s) => s.len(),
            Self::Signed16(s) => s.len(),
            Self::Signed32(s) => s.len(),
            Self::Signed64(s) => s.len(),
            Self::Float16(s) => s.len(),
            Self::BFloat16(s) => s.len(),
            Self::Float32(s) => s.len(),
            Self::Float64(s) => s.len(),
        }
    }

    /// Size in bytes of a single element.
    #[inline]
    pub fn elem_size_bytes(&self) -> usize {
        match self {
            Self::Boolean(_) => mem::size_of::<bool>(),
            Self::Unsigned8(_) => mem::size_of::<u8>(),
            Self::Unsigned16(_) => mem::size_of::<u16>(),
            Self::Unsigned32(_) => mem::size_of::<u32>(),
            Self::Unsigned64(_) => mem::size_of::<u64>(),
            Self::Signed8(_) => mem::size_of::<i8>(),
            Self::Signed16(_) => mem::size_of::<i16>(),
            Self::Signed32(_) => mem::size_of::<i32>(),
            Self::Signed64(_) => mem::size_of::<i64>(),
            Self::Float16(_) => mem::size_of::<F16>(),
            Self::BFloat16(_) => mem::size_of::<BF16>(),
            Self::Float32(_) => mem::size_of::<f32>(),
            Self::Float64(_) => mem::size_of::<f64>(),
        }
    }

    /// Total byte length of the storage.
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.len_elems().saturating_mul(self.elem_size_bytes())
    }
}

/// Borrowed, **immutable** tensor view (no allocation, no casting).
#[derive(Debug, Clone, Copy)]
pub struct TensorRef<'a> {
    elems: TensorElements<'a>,
    shape: &'a [usize],
    memory_class: MemoryClass,
}

impl<'a> TensorRef<'a> {
    /// Construct from typed elements and a borrowed shape.
    ///
    /// This is the primary, safe constructor. It does not allocate.
    #[inline]
    pub fn new(elems: TensorElements<'a>, shape: &'a [usize], memory_class: MemoryClass) -> Self {
        // Debug-only: catch overflow and mismatched shapes early.
        debug_assert!(
            match Self::checked_element_count(shape) {
                Some(expected) => expected == elems.len_elems(),
                None => false, // overflow
            },
            "TensorRef::new: shape elements ({}) != data elements ({})",
            // These expressions are only evaluated in debug builds:
            Self::checked_element_count(shape).unwrap_or(usize::MAX),
            elems.len_elems()
        );

        Self {
            elems,
            shape,
            memory_class,
        }
    }

    /// Convenience: build from a typed slice (no allocation).
    ///
    /// Example:
    /// ```text
    /// # let shape = &[2, 3];
    /// # let data: &[f32] = &[0.0; 6];
    /// # let class = MemoryClass::Host;
    /// let t = TensorRef::from_slice_f32(data, shape, class);
    /// ```
    #[inline]
    /// Construct a `TensorRef` from `&[bool]`.
    pub fn from_slice_bool(data: &'a [bool], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Boolean(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[u8]`.
    pub fn from_slice_u8(data: &'a [u8], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Unsigned8(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[u16]`.
    pub fn from_slice_u16(data: &'a [u16], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Unsigned16(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[u32]`.
    pub fn from_slice_u32(data: &'a [u32], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Unsigned32(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[u64]`.
    pub fn from_slice_u64(data: &'a [u64], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Unsigned64(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[i8]`.
    pub fn from_slice_i8(data: &'a [i8], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Signed8(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[i16]`.
    pub fn from_slice_i16(data: &'a [i16], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Signed16(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[i32]`.
    pub fn from_slice_i32(data: &'a [i32], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Signed32(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[i64]`.
    pub fn from_slice_i64(data: &'a [i64], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Signed64(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[F16]`.
    pub fn from_slice_f16(data: &'a [F16], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Float16(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[BF16]`.
    pub fn from_slice_bf16(data: &'a [BF16], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::BFloat16(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[f32]`.
    pub fn from_slice_f32(data: &'a [f32], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Float32(data), shape, class)
    }
    #[inline]
    /// Construct a `TensorRef` from `&[f64]`.
    pub fn from_slice_f64(data: &'a [f64], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElements::Float64(data), shape, class)
    }

    /// Get the element `DataType`.
    #[inline]
    pub fn data_type(&self) -> DataType {
        self.elems.data_type()
    }

    /// Borrow the shape.
    #[inline]
    pub fn shape(&self) -> &'a [usize] {
        self.shape
    }

    /// Rank (number of dimensions).
    #[inline]
    pub fn rank(&self) -> usize {
        self.shape.len()
    }

    /// Number of elements.
    #[inline]
    pub fn len_elems(&self) -> usize {
        self.elems.len_elems()
    }

    /// Total byte length of the underlying storage.
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.elems.byte_len()
    }

    /// Memory placement of the backing storage.
    #[inline]
    pub fn memory_class(&self) -> MemoryClass {
        self.memory_class
    }

    /// Validate that `shape` matches the element count.
    #[inline]
    pub fn is_compatible(&self) -> bool {
        match Self::checked_element_count(self.shape) {
            Some(n) => n == self.len_elems(),
            None => false,
        }
    }

    /// Change the logical shape **without** reallocating or moving data.
    ///
    /// - Updates only the metadata; the underlying buffer and `memory_class` stay the same.
    /// - Consumes and returns `self` (builder-style).
    /// - **Safety**: relies on a `debug_assert!` that the element count implied by
    ///   `new_shape` matches the current data length. In release builds this is not
    ///   checked; use only when you know the shape is correct.
    #[inline]
    pub fn reshape_unchecked(mut self, new_shape: &'a [usize]) -> Self {
        debug_assert!(
            Self::checked_element_count(new_shape) == Some(self.len_elems()),
            "TensorRef::reshape_unchecked: shape elements ({}) != data elements ({})",
            Self::checked_element_count(new_shape).unwrap_or(usize::MAX),
            self.len_elems()
        );
        self.shape = new_shape;
        self
    }

    // ---- Typed accessors (no casts, fully safe) ----------------------------

    #[inline]
    /// Borrow as `&[bool]` if the underlying type is `Boolean`.
    pub fn as_bool(&self) -> Option<&'a [bool]> {
        if let TensorElements::Boolean(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[u8]` if the underlying type is `Unsigned8`.
    pub fn as_u8(&self) -> Option<&'a [u8]> {
        if let TensorElements::Unsigned8(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[u16]` if the underlying type is `Unsigned16`.
    pub fn as_u16(&self) -> Option<&'a [u16]> {
        if let TensorElements::Unsigned16(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[u32]` if the underlying type is `Unsigned32`.
    pub fn as_u32(&self) -> Option<&'a [u32]> {
        if let TensorElements::Unsigned32(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[u64]` if the underlying type is `Unsigned64`.
    pub fn as_u64(&self) -> Option<&'a [u64]> {
        if let TensorElements::Unsigned64(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[i8]` if the underlying type is `Signed8`.
    pub fn as_i8(&self) -> Option<&'a [i8]> {
        if let TensorElements::Signed8(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[i16]` if the underlying type is `Signed16`.
    pub fn as_i16(&self) -> Option<&'a [i16]> {
        if let TensorElements::Signed16(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[i32]` if the underlying type is `Signed32`.
    pub fn as_i32(&self) -> Option<&'a [i32]> {
        if let TensorElements::Signed32(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[i64]` if the underlying type is `Signed64`.
    pub fn as_i64(&self) -> Option<&'a [i64]> {
        if let TensorElements::Signed64(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[F16]` if the underlying type is `Float16`.
    pub fn as_f16(&self) -> Option<&'a [F16]> {
        if let TensorElements::Float16(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[BF16]` if the underlying type is `BFloat16`.
    pub fn as_bf16(&self) -> Option<&'a [BF16]> {
        if let TensorElements::BFloat16(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[f32]` if the underlying type is `Float32`.
    pub fn as_f32(&self) -> Option<&'a [f32]> {
        if let TensorElements::Float32(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&[f64]` if the underlying type is `Float64`.
    pub fn as_f64(&self) -> Option<&'a [f64]> {
        if let TensorElements::Float64(s) = self.elems {
            Some(s)
        } else {
            None
        }
    }

    #[inline]
    fn checked_element_count(shape: &[usize]) -> Option<usize> {
        let mut acc = 1usize;
        for &d in shape {
            acc = acc.checked_mul(d)?;
        }
        Some(acc)
    }
}

impl<'a> Payload for TensorRef<'a> {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        debug_assert!(self.is_compatible(), "TensorRef: incompatible shape/data");
        BufferDescriptor::new(self.byte_len(), self.memory_class)
    }
}

/// Borrowed, **mutable** tensor view element storage.
///
/// Each variant holds a mutable borrowed, contiguous slice of the corresponding
/// scalar type. No allocation or copying is performed.
#[derive(Debug)]
pub enum TensorElementsMut<'a> {
    /// Mutable slice of boolean elements.
    Boolean(&'a mut [bool]),
    /// Mutable slice of unsigned 8-bit integers.
    Unsigned8(&'a mut [u8]),
    /// Mutable slice of unsigned 16-bit integers.
    Unsigned16(&'a mut [u16]),
    /// Mutable slice of unsigned 32-bit integers.
    Unsigned32(&'a mut [u32]),
    /// Mutable slice of unsigned 64-bit integers.
    Unsigned64(&'a mut [u64]),
    /// Mutable slice of signed 8-bit integers.
    Signed8(&'a mut [i8]),
    /// Mutable slice of signed 16-bit integers.
    Signed16(&'a mut [i16]),
    /// Mutable slice of signed 32-bit integers.
    Signed32(&'a mut [i32]),
    /// Mutable slice of signed 64-bit integers.
    Signed64(&'a mut [i64]),
    /// Mutable slice of IEEE-754 binary16 values.
    Float16(&'a mut [F16]),
    /// Mutable slice of bfloat16 values.
    BFloat16(&'a mut [BF16]),
    /// Mutable slice of 32-bit IEEE-754 floats.
    Float32(&'a mut [f32]),
    /// Mutable slice of 64-bit IEEE-754 floats.
    Float64(&'a mut [f64]),
}

impl<'a> TensorElementsMut<'a> {
    /// Return the logical `DataType` of the underlying mutable slice.
    #[inline]
    pub fn data_type(&self) -> DataType {
        match self {
            Self::Boolean(_) => DataType::Boolean,
            Self::Unsigned8(_) => DataType::Unsigned8,
            Self::Unsigned16(_) => DataType::Unsigned16,
            Self::Unsigned32(_) => DataType::Unsigned32,
            Self::Unsigned64(_) => DataType::Unsigned64,
            Self::Signed8(_) => DataType::Signed8,
            Self::Signed16(_) => DataType::Signed16,
            Self::Signed32(_) => DataType::Signed32,
            Self::Signed64(_) => DataType::Signed64,
            Self::Float16(_) => DataType::Float16,
            Self::BFloat16(_) => DataType::BFloat16,
            Self::Float32(_) => DataType::Float32,
            Self::Float64(_) => DataType::Float64,
        }
    }

    /// Number of elements in the underlying mutable slice.
    #[inline]
    pub fn len_elems(&self) -> usize {
        match self {
            Self::Boolean(s) => s.len(),
            Self::Unsigned8(s) => s.len(),
            Self::Unsigned16(s) => s.len(),
            Self::Unsigned32(s) => s.len(),
            Self::Unsigned64(s) => s.len(),
            Self::Signed8(s) => s.len(),
            Self::Signed16(s) => s.len(),
            Self::Signed32(s) => s.len(),
            Self::Signed64(s) => s.len(),
            Self::Float16(s) => s.len(),
            Self::BFloat16(s) => s.len(),
            Self::Float32(s) => s.len(),
            Self::Float64(s) => s.len(),
        }
    }

    /// Size in bytes of a single element of the current variant.
    #[inline]
    pub fn elem_size_bytes(&self) -> usize {
        match self {
            Self::Boolean(_) => mem::size_of::<bool>(),
            Self::Unsigned8(_) => mem::size_of::<u8>(),
            Self::Unsigned16(_) => mem::size_of::<u16>(),
            Self::Unsigned32(_) => mem::size_of::<u32>(),
            Self::Unsigned64(_) => mem::size_of::<u64>(),
            Self::Signed8(_) => mem::size_of::<i8>(),
            Self::Signed16(_) => mem::size_of::<i16>(),
            Self::Signed32(_) => mem::size_of::<i32>(),
            Self::Signed64(_) => mem::size_of::<i64>(),
            Self::Float16(_) => mem::size_of::<F16>(),
            Self::BFloat16(_) => mem::size_of::<BF16>(),
            Self::Float32(_) => mem::size_of::<f32>(),
            Self::Float64(_) => mem::size_of::<f64>(),
        }
    }

    /// Total byte length of the underlying mutable slice.
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.len_elems().saturating_mul(self.elem_size_bytes())
    }
}

/// Mutable tensor view.
#[derive(Debug)]
pub struct TensorRefMut<'a> {
    elems: TensorElementsMut<'a>,
    shape: &'a [usize],
    memory_class: MemoryClass,
}

impl<'a> TensorRefMut<'a> {
    /// Construct from typed elements and a borrowed shape.
    #[inline]
    pub fn new(
        elems: TensorElementsMut<'a>,
        shape: &'a [usize],
        memory_class: MemoryClass,
    ) -> Self {
        // Debug-only: catch overflow and mismatched shapes early.
        debug_assert!(
            match Self::checked_element_count(shape) {
                Some(expected) => expected == elems.len_elems(),
                None => false, // overflow
            },
            "TensorRefMut::new: shape elements ({}) != data elements ({})",
            // Only evaluated in debug:
            Self::checked_element_count(shape).unwrap_or(usize::MAX),
            elems.len_elems()
        );
        Self {
            elems,
            shape,
            memory_class,
        }
    }

    /// Construct from `&mut [bool]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_bool(data: &'a mut [bool], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Boolean(data), shape, class)
    }

    /// Construct from `&mut [u8]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_u8(data: &'a mut [u8], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Unsigned8(data), shape, class)
    }

    /// Construct from `&mut [u16]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_u16(data: &'a mut [u16], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Unsigned16(data), shape, class)
    }

    /// Construct from `&mut [u32]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_u32(data: &'a mut [u32], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Unsigned32(data), shape, class)
    }

    /// Construct from `&mut [u64]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_u64(data: &'a mut [u64], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Unsigned64(data), shape, class)
    }

    /// Construct from `&mut [i8]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_i8(data: &'a mut [i8], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Signed8(data), shape, class)
    }

    /// Construct from `&mut [i16]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_i16(data: &'a mut [i16], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Signed16(data), shape, class)
    }

    /// Construct from `&mut [i32]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_i32(data: &'a mut [i32], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Signed32(data), shape, class)
    }

    /// Construct from `&mut [i64]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_i64(data: &'a mut [i64], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Signed64(data), shape, class)
    }

    /// Construct from `&mut [F16]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_f16(data: &'a mut [F16], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Float16(data), shape, class)
    }

    /// Construct from `&mut [BF16]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_bf16(data: &'a mut [BF16], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::BFloat16(data), shape, class)
    }

    /// Construct from `&mut [f32]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_f32(data: &'a mut [f32], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Float32(data), shape, class)
    }

    /// Construct from `&mut [f64]` with borrowed shape and placement.
    #[inline]
    pub fn from_slice_f64(data: &'a mut [f64], shape: &'a [usize], class: MemoryClass) -> Self {
        Self::new(TensorElementsMut::Float64(data), shape, class)
    }

    /// Get the element `DataType`.
    #[inline]
    pub fn data_type(&self) -> DataType {
        self.elems.data_type()
    }

    /// Borrow the shape.
    #[inline]
    pub fn shape(&self) -> &'a [usize] {
        self.shape
    }

    /// Rank (number of dimensions).
    #[inline]
    pub fn rank(&self) -> usize {
        self.shape.len()
    }

    /// Number of elements.
    #[inline]
    pub fn len_elems(&self) -> usize {
        self.elems.len_elems()
    }

    /// Total byte length of the underlying storage.
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.elems.byte_len()
    }

    /// Memory placement of the backing storage.
    #[inline]
    pub fn memory_class(&self) -> MemoryClass {
        self.memory_class
    }

    /// Validate that `shape` matches the element count.
    #[inline]
    pub fn is_compatible(&self) -> bool {
        match Self::checked_element_count(self.shape) {
            Some(n) => n == self.len_elems(),
            None => false,
        }
    }

    /// Change the logical shape **in place** for a mutable tensor view.
    ///
    /// - Updates only the metadata; the backing storage and `memory_class` are unchanged.
    /// - Consumes and returns `self` (builder-style).
    /// - **Safety**: guarded by a `debug_assert!` that `new_shape` has the same total
    ///   element count as the current data. No check runs in release builds.
    #[inline]
    pub fn reshape_unchecked(mut self, new_shape: &'a [usize]) -> Self {
        debug_assert!(
            Self::checked_element_count(new_shape) == Some(self.len_elems()),
            "TensorRefMut::reshape_unchecked: shape elements ({}) != data elements ({})",
            Self::checked_element_count(new_shape).unwrap_or(usize::MAX),
            self.len_elems()
        );
        self.shape = new_shape;
        self
    }

    // ---- Typed mutable accessors (no casts, fully safe) --------------------

    #[inline]
    /// Borrow as `&mut [bool]` if the underlying type is `Boolean`.
    pub fn as_bool_mut(&mut self) -> Option<&'_ mut [bool]> {
        if let TensorElementsMut::Boolean(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [u8]` if the underlying type is `Unsigned8`.
    pub fn as_u8_mut(&mut self) -> Option<&'_ mut [u8]> {
        if let TensorElementsMut::Unsigned8(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [u16]` if the underlying type is `Unsigned16`.
    pub fn as_u16_mut(&mut self) -> Option<&'_ mut [u16]> {
        if let TensorElementsMut::Unsigned16(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [u32]` if the underlying type is `Unsigned32`.
    pub fn as_u32_mut(&mut self) -> Option<&'_ mut [u32]> {
        if let TensorElementsMut::Unsigned32(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [u64]` if the underlying type is `Unsigned64`.
    pub fn as_u64_mut(&mut self) -> Option<&'_ mut [u64]> {
        if let TensorElementsMut::Unsigned64(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [i8]` if the underlying type is `Signed8`.
    pub fn as_i8_mut(&mut self) -> Option<&'_ mut [i8]> {
        if let TensorElementsMut::Signed8(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [i16]` if the underlying type is `Signed16`.
    pub fn as_i16_mut(&mut self) -> Option<&'_ mut [i16]> {
        if let TensorElementsMut::Signed16(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [i32]` if the underlying type is `Signed32`.
    pub fn as_i32_mut(&mut self) -> Option<&'_ mut [i32]> {
        if let TensorElementsMut::Signed32(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [i64]` if the underlying type is `Signed64`.
    pub fn as_i64_mut(&mut self) -> Option<&'_ mut [i64]> {
        if let TensorElementsMut::Signed64(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [F16]` if the underlying type is `Float16`.
    pub fn as_f16_mut(&mut self) -> Option<&'_ mut [F16]> {
        if let TensorElementsMut::Float16(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [BF16]` if the underlying type is `BFloat16`.
    pub fn as_bf16_mut(&mut self) -> Option<&'_ mut [BF16]> {
        if let TensorElementsMut::BFloat16(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [f32]` if the underlying type is `Float32`.
    pub fn as_f32_mut(&mut self) -> Option<&'_ mut [f32]> {
        if let TensorElementsMut::Float32(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }
    #[inline]
    /// Borrow as `&mut [f64]` if the underlying type is `Float64`.
    pub fn as_f64_mut(&mut self) -> Option<&'_ mut [f64]> {
        if let TensorElementsMut::Float64(s) = &mut self.elems {
            Some(s)
        } else {
            None
        }
    }

    #[inline]
    fn checked_element_count(shape: &[usize]) -> Option<usize> {
        let mut acc = 1usize;
        for &d in shape {
            acc = acc.checked_mul(d)?;
        }
        Some(acc)
    }
}

impl<'a> TensorRefMut<'a> {
    /// Convert an exclusive mutable view into an immutable view (no copy).
    #[inline]
    pub fn into_immutable(self) -> TensorRef<'a> {
        use crate::message::tensor::{TensorElements, TensorElementsMut};

        let elems = match self.elems {
            TensorElementsMut::Boolean(s) => TensorElements::Boolean(s),
            TensorElementsMut::Unsigned8(s) => TensorElements::Unsigned8(s),
            TensorElementsMut::Unsigned16(s) => TensorElements::Unsigned16(s),
            TensorElementsMut::Unsigned32(s) => TensorElements::Unsigned32(s),
            TensorElementsMut::Unsigned64(s) => TensorElements::Unsigned64(s),
            TensorElementsMut::Signed8(s) => TensorElements::Signed8(s),
            TensorElementsMut::Signed16(s) => TensorElements::Signed16(s),
            TensorElementsMut::Signed32(s) => TensorElements::Signed32(s),
            TensorElementsMut::Signed64(s) => TensorElements::Signed64(s),
            TensorElementsMut::Float16(s) => TensorElements::Float16(s),
            TensorElementsMut::BFloat16(s) => TensorElements::BFloat16(s),
            TensorElementsMut::Float32(s) => TensorElements::Float32(s),
            TensorElementsMut::Float64(s) => TensorElements::Float64(s),
        };

        TensorRef::new(elems, self.shape, self.memory_class)
    }
}

impl<'a> Payload for TensorRefMut<'a> {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        debug_assert!(
            self.is_compatible(),
            "TensorRefMut: incompatible shape/data"
        );
        BufferDescriptor::new(self.byte_len(), self.memory_class)
    }
}
