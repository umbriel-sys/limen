//! Owned, fixed-capacity inline tensor (`no_std`, `no_alloc`).
//!
//! [`Tensor<T, N, R>`] stores up to `N` elements of scalar type `T` with
//! a compile-time rank `R`. Data is held inline (stack / static storage),
//! so no heap allocation is required. A live-element count (`len`) tracks
//! how many of the `N` slots are in use; the shape array `[usize; R]` records
//! the logical dimensions.
//!
//! Use `&Tensor<T, N, R>` for a borrowed, zero-copy view into an existing
//! buffer (e.g. a frame from a DMA ring).

use core::mem;

use crate::memory::BufferDescriptor;
use crate::message::payload::Payload;
use crate::types::{DType, DataType};

// ---------------------------------------------------------------------------
// Owned, fixed-capacity tensor
// ---------------------------------------------------------------------------

/// Owned, fixed-capacity tensor stored inline.
///
/// - `T` — scalar element type (`Copy + Default + DType`).
/// - `N` — maximum element capacity (compile-time).
/// - `R` — tensor rank / number of dimensions (compile-time).
///
/// `len` tracks live elements (≤ `N`). Shape is `[usize; R]` with all
/// dimensions active. Rank-0 scalars use `R = 0`.
///
/// Byte size is computed, not stored — one multiply is cheaper than 8 bytes
/// of struct overhead on a constrained MCU.
///
/// # Examples
/// ```text
/// // TF Lite Micro person detection input.
/// let img = Tensor::<u8, 9216, 4>::nhwc(1, 96, 96, 1, &pixels);
///
/// // Classifier output.
/// let out = Tensor::<i8, 3, 2>::nc(1, 3, &scores);
///
/// // Flat feature vector.
/// let v = Tensor::<f32, 128, 1>::from_slice(&features);
///
/// // Destructure shape for processing.
/// let [batch, height, width, channels] = *img.shape();
/// let pixel = img.at([0, 10, 20, 0]);
/// ```
#[derive(Clone, Copy)]
pub struct Tensor<T: Copy + Default + DType, const N: usize, const R: usize> {
    data: [T; N],
    len: usize,
    shape: [usize; R],
}

// ---------------------------------------------------------------------------
// Core API (all ranks)
// ---------------------------------------------------------------------------

impl<T: Copy + Default + DType, const N: usize, const R: usize> Tensor<T, N, R> {
    /// Create from a shape and data slice.
    ///
    /// # Panics
    /// - If `data.len() > N`.
    /// - In debug: if the product of `shape` != `data.len()`.
    #[inline]
    pub fn from_shape(shape: [usize; R], data: &[T]) -> Self {
        assert!(
            data.len() <= N,
            "Tensor: data length {} exceeds capacity {}",
            data.len(),
            N
        );
        debug_assert_eq!(
            checked_product(&shape),
            Some(data.len()),
            "Tensor: shape product != data length"
        );

        let mut buf = [T::default(); N];
        buf[..data.len()].copy_from_slice(data);

        Self {
            data: buf,
            len: data.len(),
            shape,
        }
    }

    /// Create filled with `value`.
    ///
    /// # Panics
    /// If the shape product exceeds `N`.
    #[inline]
    pub fn filled(shape: [usize; R], value: T) -> Self {
        let count = checked_product(&shape).expect("Tensor: shape overflow");
        assert!(count <= N, "Tensor: count {} exceeds capacity {}", count, N);

        let mut buf = [T::default(); N];
        let mut i = 0;
        while i < count {
            buf[i] = value;
            i += 1;
        }

        Self {
            data: buf,
            len: count,
            shape,
        }
    }

    /// Create zeroed with the given shape.
    #[inline]
    pub fn zeros(shape: [usize; R]) -> Self {
        Self::filled(shape, T::default())
    }

    /// The `DataType` of the scalar elements.
    #[inline]
    pub fn data_type(&self) -> DataType {
        T::DATA_TYPE
    }

    /// Number of live elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the tensor has zero live elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Compile-time element capacity.
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Rank (compile-time constant).
    #[inline]
    pub const fn rank(&self) -> usize {
        R
    }

    /// Active shape dimensions.
    #[inline]
    pub fn shape(&self) -> &[usize; R] {
        &self.shape
    }

    /// Borrow the live data.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.data[..self.len]
    }

    /// Mutably borrow the live data.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data[..self.len]
    }

    /// Total bytes of live data (computed, not stored).
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.len.saturating_mul(mem::size_of::<T>())
    }

    /// Reshape in place (metadata only).
    ///
    /// # Panics
    /// In debug: if the shape product != `self.len`.
    #[inline]
    pub fn reshape(&mut self, new_shape: [usize; R]) {
        debug_assert_eq!(
            checked_product(&new_shape),
            Some(self.len),
            "Tensor::reshape: shape product != len"
        );
        self.shape = new_shape;
    }

    /// Validate that the shape product matches the element count.
    #[inline]
    pub fn is_compatible(&self) -> bool {
        checked_product(&self.shape) == Some(self.len)
    }

    /// Read element by multi-dimensional index.
    ///
    /// # Panics
    /// If the flat index is out of bounds.
    #[inline]
    pub fn at(&self, index: [usize; R]) -> T {
        self.data[self.flat_index(index)]
    }

    /// Write element by multi-dimensional index.
    ///
    /// # Panics
    /// If the flat index is out of bounds.
    #[inline]
    pub fn set(&mut self, index: [usize; R], value: T) {
        let i = self.flat_index(index);
        self.data[i] = value;
    }

    /// Convert multi-dimensional index to flat offset (row-major).
    #[inline]
    fn flat_index(&self, index: [usize; R]) -> usize {
        let mut flat = 0usize;
        let mut stride = 1usize;
        let mut d = R;
        while d > 0 {
            d -= 1;
            flat += index[d] * stride;
            stride *= self.shape[d];
        }
        assert!(flat < self.len, "tensor index out of bounds");
        flat
    }
}

// ---------------------------------------------------------------------------
// Rank-0: scalar
// ---------------------------------------------------------------------------

impl<T: Copy + Default + DType, const N: usize> Tensor<T, N, 0> {
    /// Create a rank-0 scalar.
    #[inline]
    pub fn scalar(value: T) -> Self {
        assert!(N >= 1, "Tensor::scalar: capacity must be >= 1");
        let mut buf = [T::default(); N];
        buf[0] = value;
        Self {
            data: buf,
            len: 1,
            shape: [],
        }
    }

    /// Read the scalar value.
    #[inline]
    pub fn value(&self) -> T {
        self.data[0]
    }
}

// ---------------------------------------------------------------------------
// Rank-1: flat vector
// ---------------------------------------------------------------------------

impl<T: Copy + Default + DType, const N: usize> Tensor<T, N, 1> {
    /// Create from a data slice (shape inferred as `[data.len()]`).
    #[inline]
    pub fn from_slice(data: &[T]) -> Self {
        Self::from_shape([data.len()], data)
    }
}

// ---------------------------------------------------------------------------
// Rank-2: common constructors
// ---------------------------------------------------------------------------

impl<T: Copy + Default + DType, const N: usize> Tensor<T, N, 2> {
    /// `[batch, classes]` — classifier output (TF Lite Micro softmax, etc.).
    #[inline]
    pub fn nc(batch: usize, classes: usize, data: &[T]) -> Self {
        Self::from_shape([batch, classes], data)
    }

    /// `[rows, cols]` — 2-D matrix.
    #[inline]
    pub fn matrix(rows: usize, cols: usize, data: &[T]) -> Self {
        Self::from_shape([rows, cols], data)
    }
}

// ---------------------------------------------------------------------------
// Rank-3: common constructors
// ---------------------------------------------------------------------------

impl<T: Copy + Default + DType, const N: usize> Tensor<T, N, 3> {
    /// `[batch, time_steps, features]` — sequence / spectrogram
    /// (keyword spotting, audio models).
    #[inline]
    pub fn sequence(batch: usize, time_steps: usize, features: usize, data: &[T]) -> Self {
        Self::from_shape([batch, time_steps, features], data)
    }

    /// `[height, width, channels]` — single image, no batch dim
    /// (common in resource-constrained pipelines).
    #[inline]
    pub fn hwc(height: usize, width: usize, channels: usize, data: &[T]) -> Self {
        Self::from_shape([height, width, channels], data)
    }
}

// ---------------------------------------------------------------------------
// Rank-4: common constructors
// ---------------------------------------------------------------------------

impl<T: Copy + Default + DType, const N: usize> Tensor<T, N, 4> {
    /// `[batch, height, width, channels]` — TF Lite / TF Lite Micro standard.
    #[inline]
    pub fn nhwc(batch: usize, height: usize, width: usize, channels: usize, data: &[T]) -> Self {
        Self::from_shape([batch, height, width, channels], data)
    }

    /// `[batch, channels, height, width]` — PyTorch / tract default.
    #[inline]
    pub fn nchw(batch: usize, channels: usize, height: usize, width: usize, data: &[T]) -> Self {
        Self::from_shape([batch, channels, height, width], data)
    }
}

// ---------------------------------------------------------------------------
// Trait impls
// ---------------------------------------------------------------------------

impl<T: Copy + Default + DType, const N: usize, const R: usize> Default for Tensor<T, N, R> {
    #[inline]
    fn default() -> Self {
        Self {
            data: [T::default(); N],
            len: 0,
            shape: [0; R],
        }
    }
}

impl<T: Copy + Default + DType + core::fmt::Debug, const N: usize, const R: usize> core::fmt::Debug
    for Tensor<T, N, R>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // show metadata and the live data slice
        f.debug_struct("Tensor")
            .field("data_type", &self.data_type())
            .field("len", &self.len)
            .field("shape", &&self.shape[..])
            .field("capacity", &N)
            .field("data", &self.as_slice())
            .finish()
    }
}

impl<T: Copy + Default + DType + PartialEq, const N: usize, const R: usize> PartialEq
    for Tensor<T, N, R>
{
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self.shape == other.shape
            && self.data[..self.len] == other.data[..other.len]
    }
}

impl<T: Copy + Default + DType + Eq, const N: usize, const R: usize> Eq for Tensor<T, N, R> {}

impl<T: Copy + Default + DType, const N: usize, const R: usize> Payload for Tensor<T, N, R> {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        BufferDescriptor::new(self.byte_len())
    }
}

/// Checked product of a shape array. Returns `None` on overflow.
#[inline]
fn checked_product(shape: &[usize]) -> Option<usize> {
    let mut acc = 1usize;
    for &d in shape {
        acc = acc.checked_mul(d)?;
    }
    Some(acc)
}

/// Canonical shape used by shared test and benchmark tensor fixtures.
///
/// This shape is fixed at `3 × 3` so all common test payloads use the same
/// rank-2 layout across memory, message, and benchmark code.
#[cfg(any(test, feature = "bench"))]
pub const TEST_TENSOR_SHAPE: [usize; 2] = [3, 3];

/// Total live element count of [`TestTensor`].
///
/// This is the product of [`TEST_TENSOR_SHAPE`] and is fixed at `9`.
#[cfg(any(test, feature = "bench"))]
pub const TEST_TENSOR_ELEMENT_COUNT: usize = 9;

/// Total live payload byte count of [`TestTensor`].
///
/// This reflects the tensor payload size reported through [`Payload`], not the
/// full in-memory size of the `Tensor` struct itself.
#[cfg(any(test, feature = "bench"))]
pub const TEST_TENSOR_BYTE_COUNT: usize = TEST_TENSOR_ELEMENT_COUNT * mem::size_of::<u32>();

/// Shared rank-2 tensor payload used by tests and benchmarks.
///
/// This alias standardizes on a `3 × 3` tensor of `u32` values with exactly
/// `9` live elements.
#[cfg(any(test, feature = "bench"))]
pub type TestTensor = Tensor<u32, TEST_TENSOR_ELEMENT_COUNT, 2>;

/// Create a shared test tensor with every element set to the same value.
///
/// The returned tensor always has shape [`TEST_TENSOR_SHAPE`].
#[cfg(any(test, feature = "bench"))]
#[inline]
pub fn create_test_tensor_filled_with(value: u32) -> TestTensor {
    Tensor::filled(TEST_TENSOR_SHAPE, value)
}

/// Create a shared test tensor from explicit `3 × 3` element values.
///
/// The input array is flattened in row-major order into a tensor with shape
/// [`TEST_TENSOR_SHAPE`], so `values[0][0]` becomes the first element and
/// `values[2][2]` becomes the last.
#[cfg(any(test, feature = "bench"))]
#[inline]
pub fn create_test_tensor_from_array(values: [[u32; 3]; 3]) -> TestTensor {
    Tensor::from_shape(
        TEST_TENSOR_SHAPE,
        &[
            values[0][0],
            values[0][1],
            values[0][2],
            values[1][0],
            values[1][1],
            values[1][2],
            values[2][0],
            values[2][1],
            values[2][2],
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DataType;

    // -----------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------

    #[test]
    fn default_is_empty() {
        let t = Tensor::<f32, 16, 2>::default();
        assert!(t.is_empty());
        assert_eq!(t.len(), 0);
        assert_eq!(t.shape(), &[0, 0]);
        assert_eq!(t.byte_len(), 0);
        assert_eq!(t.rank(), 2);
        assert_eq!(t.capacity(), 16);
    }

    #[test]
    fn from_shape_basic() {
        let data = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let t = Tensor::<f32, 8, 2>::from_shape([2, 3], &data);
        assert_eq!(t.len(), 6);
        assert_eq!(t.shape(), &[2, 3]);
        assert_eq!(t.as_slice(), &data);
        assert_eq!(t.byte_len(), 6 * 4);
        assert!(t.is_compatible());
    }

    #[test]
    fn from_shape_partial_capacity() {
        let data = [1u8, 2, 3];
        let t = Tensor::<u8, 256, 1>::from_shape([3], &data);
        assert_eq!(t.len(), 3);
        assert_eq!(t.capacity(), 256);
        assert_eq!(t.as_slice(), &[1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "exceeds capacity")]
    fn from_shape_panics_on_overflow() {
        let data = [0u8; 32];
        let _ = Tensor::<u8, 16, 1>::from_shape([32], &data);
    }

    #[test]
    fn filled_creates_uniform_tensor() {
        let t = Tensor::<i8, 12, 2>::filled([3, 4], 42i8);
        assert_eq!(t.len(), 12);
        assert_eq!(t.shape(), &[3, 4]);
        assert!(t.as_slice().iter().all(|&v| v == 42));
    }

    #[test]
    #[should_panic(expected = "exceeds capacity")]
    fn filled_panics_on_overflow() {
        let _ = Tensor::<f32, 4, 2>::filled([3, 3], 0.0);
    }

    #[test]
    fn zeros_is_all_default() {
        let t = Tensor::<u32, 64, 3>::zeros([2, 4, 8]);
        assert_eq!(t.len(), 64);
        assert!(t.as_slice().iter().all(|&v| v == 0));
    }

    // -----------------------------------------------------------------
    // Rank-0: scalar
    // -----------------------------------------------------------------

    #[test]
    fn scalar_round_trip() {
        let t = Tensor::<f32, 1, 0>::scalar(3.14);
        assert_eq!(t.value(), 3.14);
        assert_eq!(t.len(), 1);
        assert_eq!(t.rank(), 0);
        assert_eq!(t.shape(), &[]);
        assert_eq!(t.byte_len(), 4);
        assert!(t.is_compatible());
    }

    #[test]
    fn scalar_with_excess_capacity() {
        let t = Tensor::<u8, 4, 0>::scalar(7);
        assert_eq!(t.value(), 7);
        assert_eq!(t.capacity(), 4);
        assert_eq!(t.len(), 1);
    }

    // -----------------------------------------------------------------
    // Rank-1: from_slice
    // -----------------------------------------------------------------

    #[test]
    fn from_slice_rank1() {
        let t = Tensor::<f32, 8, 1>::from_slice(&[1.0, 2.0, 3.0]);
        assert_eq!(t.len(), 3);
        assert_eq!(t.shape(), &[3]);
        assert_eq!(t.as_slice(), &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn from_slice_empty() {
        let t = Tensor::<u8, 8, 1>::from_slice(&[]);
        assert!(t.is_empty());
        assert_eq!(t.shape(), &[0]);
    }

    // -----------------------------------------------------------------
    // Rank-2: named constructors
    // -----------------------------------------------------------------

    #[test]
    fn nc_constructor() {
        let data = [10i8, 20, 30];
        let t = Tensor::<i8, 4, 2>::nc(1, 3, &data);
        assert_eq!(t.shape(), &[1, 3]);
        assert_eq!(t.len(), 3);
        assert_eq!(t.as_slice(), &data);
    }

    #[test]
    fn matrix_constructor() {
        let data: [f32; 6] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let t = Tensor::<f32, 8, 2>::matrix(2, 3, &data);
        assert_eq!(t.shape(), &[2, 3]);
        assert_eq!(t.len(), 6);
    }

    // -----------------------------------------------------------------
    // Rank-3: named constructors
    // -----------------------------------------------------------------

    #[test]
    fn sequence_constructor() {
        let data = [0i8; 1960];
        let t = Tensor::<i8, 1960, 3>::sequence(1, 49, 40, &data);
        assert_eq!(t.shape(), &[1, 49, 40]);
        assert_eq!(t.len(), 1960);
    }

    #[test]
    fn hwc_constructor() {
        let data = [0u8; 48];
        let t = Tensor::<u8, 48, 3>::hwc(4, 4, 3, &data);
        assert_eq!(t.shape(), &[4, 4, 3]);
        assert_eq!(t.len(), 48);
    }

    // -----------------------------------------------------------------
    // Rank-4: named constructors
    // -----------------------------------------------------------------

    #[test]
    fn nhwc_constructor() {
        let data = [0u8; 9216];
        let t = Tensor::<u8, 9216, 4>::nhwc(1, 96, 96, 1, &data);
        assert_eq!(t.shape(), &[1, 96, 96, 1]);
        assert_eq!(t.len(), 9216);
        assert!(t.is_compatible());
    }

    #[test]
    fn nchw_constructor() {
        // 1×3×4×4 = 48 elements — small enough for the test thread stack.
        let data = [0.0f32; 48];
        let t = Tensor::<f32, 48, 4>::nchw(1, 3, 4, 4, &data);
        assert_eq!(t.shape(), &[1, 3, 4, 4]);
    }

    // -----------------------------------------------------------------
    // Indexing: at / set
    // -----------------------------------------------------------------

    #[test]
    fn at_and_set_rank2() {
        let data = [1u32, 2, 3, 4, 5, 6];
        let mut t = Tensor::<u32, 8, 2>::matrix(2, 3, &data);

        // Row-major: [0,0]=1, [0,1]=2, [0,2]=3, [1,0]=4, [1,1]=5, [1,2]=6
        assert_eq!(t.at([0, 0]), 1);
        assert_eq!(t.at([0, 2]), 3);
        assert_eq!(t.at([1, 0]), 4);
        assert_eq!(t.at([1, 2]), 6);

        t.set([1, 1], 99);
        assert_eq!(t.at([1, 1]), 99);
    }

    #[test]
    fn at_rank4_nhwc() {
        // 1×2×2×3 NHWC tensor with sequential values.
        let data: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        let t = Tensor::<u8, 12, 4>::nhwc(1, 2, 2, 3, &data);

        // [0, 0, 0, 0] = 0, [0, 0, 0, 1] = 1, [0, 0, 0, 2] = 2
        // [0, 0, 1, 0] = 3, [0, 0, 1, 1] = 4, [0, 0, 1, 2] = 5
        // [0, 1, 0, 0] = 6, ...
        assert_eq!(t.at([0, 0, 0, 0]), 0);
        assert_eq!(t.at([0, 0, 0, 2]), 2);
        assert_eq!(t.at([0, 0, 1, 0]), 3);
        assert_eq!(t.at([0, 1, 0, 0]), 6);
        assert_eq!(t.at([0, 1, 1, 2]), 11);
    }

    #[test]
    fn at_rank3_sequence() {
        // 1×3×2 sequence tensor.
        let data = [10i8, 20, 30, 40, 50, 60];
        let t = Tensor::<i8, 8, 3>::sequence(1, 3, 2, &data);

        assert_eq!(t.at([0, 0, 0]), 10);
        assert_eq!(t.at([0, 0, 1]), 20);
        assert_eq!(t.at([0, 1, 0]), 30);
        assert_eq!(t.at([0, 2, 1]), 60);
    }

    #[test]
    fn at_rank1() {
        let t = Tensor::<f32, 4, 1>::from_slice(&[10.0, 20.0, 30.0]);
        assert_eq!(t.at([0]), 10.0);
        assert_eq!(t.at([2]), 30.0);
    }

    #[test]
    #[should_panic]
    fn at_out_of_bounds_panics() {
        let t = Tensor::<u8, 4, 1>::from_slice(&[1, 2, 3]);
        let _ = t.at([3]);
    }

    // -----------------------------------------------------------------
    // Reshape
    // -----------------------------------------------------------------

    #[test]
    fn reshape_preserves_data() {
        let data = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mut t = Tensor::<f32, 8, 2>::matrix(2, 3, &data);

        t.reshape([3, 2]);
        assert_eq!(t.shape(), &[3, 2]);
        assert_eq!(t.len(), 6);
        assert_eq!(t.as_slice(), &data);
        assert!(t.is_compatible());

        // Indexing uses new shape.
        assert_eq!(t.at([0, 0]), 1.0);
        assert_eq!(t.at([0, 1]), 2.0);
        assert_eq!(t.at([1, 0]), 3.0);
        assert_eq!(t.at([2, 1]), 6.0);
    }

    // -----------------------------------------------------------------
    // Mutation via as_mut_slice
    // -----------------------------------------------------------------

    #[test]
    fn as_mut_slice_modification() {
        let mut t = Tensor::<u8, 8, 1>::from_slice(&[0, 0, 0]);
        t.as_mut_slice().copy_from_slice(&[10, 20, 30]);
        assert_eq!(t.as_slice(), &[10, 20, 30]);
    }

    // -----------------------------------------------------------------
    // DataType
    // -----------------------------------------------------------------

    #[test]
    fn data_type_reflects_element() {
        assert_eq!(
            Tensor::<f32, 4, 1>::default().data_type(),
            DataType::Float32
        );
        assert_eq!(
            Tensor::<f64, 4, 1>::default().data_type(),
            DataType::Float64
        );
        assert_eq!(
            Tensor::<u8, 4, 1>::default().data_type(),
            DataType::Unsigned8
        );
        assert_eq!(Tensor::<i8, 4, 1>::default().data_type(), DataType::Signed8);
        assert_eq!(
            Tensor::<u16, 4, 1>::default().data_type(),
            DataType::Unsigned16
        );
        assert_eq!(
            Tensor::<i16, 4, 1>::default().data_type(),
            DataType::Signed16
        );
        assert_eq!(
            Tensor::<u32, 4, 1>::default().data_type(),
            DataType::Unsigned32
        );
        assert_eq!(
            Tensor::<i32, 4, 1>::default().data_type(),
            DataType::Signed32
        );
        assert_eq!(
            Tensor::<u64, 4, 1>::default().data_type(),
            DataType::Unsigned64
        );
        assert_eq!(
            Tensor::<i64, 4, 1>::default().data_type(),
            DataType::Signed64
        );
        assert_eq!(
            Tensor::<bool, 4, 1>::default().data_type(),
            DataType::Boolean
        );
    }

    // -----------------------------------------------------------------
    // Byte length
    // -----------------------------------------------------------------

    #[test]
    fn byte_len_correct_for_types() {
        let t_f32 = Tensor::<f32, 8, 1>::from_slice(&[0.0; 6]);
        assert_eq!(t_f32.byte_len(), 24); // 6 × 4

        let t_u8 = Tensor::<u8, 8, 1>::from_slice(&[0u8; 5]);
        assert_eq!(t_u8.byte_len(), 5); // 5 × 1

        let t_f64 = Tensor::<f64, 4, 1>::from_slice(&[0.0f64; 3]);
        assert_eq!(t_f64.byte_len(), 24); // 3 × 8

        let t_empty = Tensor::<f32, 4, 1>::default();
        assert_eq!(t_empty.byte_len(), 0);
    }

    // -----------------------------------------------------------------
    // Payload trait
    // -----------------------------------------------------------------

    #[test]
    fn payload_buffer_descriptor() {
        let t = Tensor::<f32, 8, 2>::matrix(2, 3, &[0.0; 6]);
        let bd = t.buffer_descriptor();
        assert_eq!(*bd.bytes(), 24);
    }

    // -----------------------------------------------------------------
    // Equality
    // -----------------------------------------------------------------

    #[test]
    fn eq_same_data_same_shape() {
        let a = Tensor::<u8, 8, 2>::matrix(2, 3, &[1, 2, 3, 4, 5, 6]);
        let b = Tensor::<u8, 8, 2>::matrix(2, 3, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(a, b);
    }

    #[test]
    fn ne_different_data() {
        let a = Tensor::<u8, 4, 1>::from_slice(&[1, 2, 3]);
        let b = Tensor::<u8, 4, 1>::from_slice(&[1, 2, 4]);
        assert_ne!(a, b);
    }

    #[test]
    fn ne_different_shape_same_data() {
        let data = [1u8, 2, 3, 4, 5, 6];
        let a = Tensor::<u8, 8, 2>::matrix(2, 3, &data);
        let b = Tensor::<u8, 8, 2>::matrix(3, 2, &data);
        assert_ne!(a, b);
    }

    #[test]
    fn ne_different_len() {
        let a = Tensor::<u8, 8, 1>::from_slice(&[1, 2, 3]);
        let b = Tensor::<u8, 8, 1>::from_slice(&[1, 2]);
        assert_ne!(a, b);
    }

    #[test]
    fn eq_ignores_padding_beyond_len() {
        // Two tensors with same live data but different capacity padding.
        let a = Tensor::<u8, 8, 1>::from_slice(&[1, 2, 3]);
        let _b = Tensor::<u8, 16, 1>::from_slice(&[1, 2, 3]);
        // Different N means different types — can't compare directly.
        // But same-N tensors with same live data are equal regardless of padding.
        let mut c = Tensor::<u8, 8, 1>::default();
        c.data[0] = 1;
        c.data[1] = 2;
        c.data[2] = 3;
        c.data[7] = 99; // padding differs
        c.len = 3;
        c.shape = [3];
        assert_eq!(a, c);
    }

    // -----------------------------------------------------------------
    // Copy semantics
    // -----------------------------------------------------------------

    #[test]
    fn tensor_is_copy() {
        let a = Tensor::<f32, 4, 1>::from_slice(&[1.0, 2.0]);
        let b = a; // copy
        let c = a; // still valid — Copy
        assert_eq!(b, c);
    }

    // -----------------------------------------------------------------
    // Clone
    // -----------------------------------------------------------------

    #[test]
    fn tensor_clone_equals_original() {
        let a = Tensor::<i32, 16, 3>::from_shape([2, 2, 2], &[1, 2, 3, 4, 5, 6, 7, 8]);
        let b = a.clone();
        assert_eq!(a, b);
    }

    // -----------------------------------------------------------------
    // Debug
    // -----------------------------------------------------------------

    #[test]
    fn debug_format_does_not_panic() {
        use core::fmt::Write;
        struct StackBuf([u8; 256], usize);
        impl Write for StackBuf {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                for &b in s.as_bytes() {
                    if self.1 >= self.0.len() {
                        return Ok(()); // silently truncate
                    }
                    self.0[self.1] = b;
                    self.1 += 1;
                }
                Ok(())
            }
        }
        let t = Tensor::<f32, 4, 2>::matrix(2, 2, &[1.0, 2.0, 3.0, 4.0]);
        let mut buf = StackBuf([0u8; 256], 0);
        write!(buf, "{:?}", t).unwrap();
        let s = core::str::from_utf8(&buf.0[..buf.1]).unwrap();
        assert!(s.contains("Tensor"));
        assert!(s.contains("Float32"));
        assert!(s.contains("len: 4"));
    }

    // -----------------------------------------------------------------
    // is_compatible
    // -----------------------------------------------------------------

    #[test]
    fn is_compatible_valid() {
        let t = Tensor::<u8, 8, 2>::matrix(2, 3, &[0; 6]);
        assert!(t.is_compatible());
    }

    #[test]
    fn is_compatible_default_zero_shape() {
        // Default has len=0, shape=[0, 0]. Product of [0, 0] = 0 = len. Compatible.
        let t = Tensor::<u8, 8, 2>::default();
        assert!(t.is_compatible());
    }

    // -----------------------------------------------------------------
    // flat_index consistency
    // -----------------------------------------------------------------

    #[test]
    fn flat_index_row_major_rank3() {
        // 2×3×4 tensor, sequential data.
        let data: [u32; 24] = [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
        ];
        let t = Tensor::<u32, 24, 3>::from_shape([2, 3, 4], &data);

        // Row-major: flat = i*12 + j*4 + k
        for i in 0..2 {
            for j in 0..3 {
                for k in 0..4 {
                    let expected = (i * 12 + j * 4 + k) as u32;
                    assert_eq!(t.at([i, j, k]), expected);
                }
            }
        }
    }

    #[test]
    fn flat_index_rank4_exhaustive_small() {
        // 1×2×2×2 = 8 elements.
        let data: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
        let t = Tensor::<u8, 8, 4>::nhwc(1, 2, 2, 2, &data);

        let mut idx = 0u8;
        for b in 0..1 {
            for h in 0..2 {
                for w in 0..2 {
                    for c in 0..2 {
                        assert_eq!(t.at([b, h, w, c]), idx);
                        idx += 1;
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Real-world shapes smoke tests
    // -----------------------------------------------------------------

    #[test]
    fn tflm_person_detection_shapes() {
        // Input: [1, 96, 96, 1] u8
        let input = Tensor::<u8, 9216, 4>::nhwc(1, 96, 96, 1, &[128u8; 9216]);
        assert_eq!(input.len(), 9216);
        assert_eq!(input.byte_len(), 9216);
        let [b, h, w, c] = *input.shape();
        assert_eq!((b, h, w, c), (1, 96, 96, 1));

        // Output: [1, 3] i8
        let output = Tensor::<i8, 3, 2>::nc(1, 3, &[10, -5, 30]);
        assert_eq!(output.len(), 3);
        assert_eq!(output.at([0, 2]), 30);
    }

    #[test]
    fn tflm_keyword_spotting_shapes() {
        // Input: [1, 49, 40] i8
        let input = Tensor::<i8, 1960, 3>::sequence(1, 49, 40, &[0i8; 1960]);
        assert_eq!(input.len(), 1960);
        let [b, t, f] = *input.shape();
        assert_eq!((b, t, f), (1, 49, 40));

        // Output: [1, 12] i8
        let output = Tensor::<i8, 12, 2>::nc(1, 12, &[0i8; 12]);
        assert_eq!(output.len(), 12);
    }

    #[test]
    fn tract_mobilenet_shapes() {
        // tract typically uses NCHW.
        // MobileNet v2: [1, 3, 224, 224] f32
        let input = Tensor::<f32, 150528, 4>::nchw(1, 3, 224, 224, &[0.0f32; 150528]);
        assert_eq!(input.len(), 150528);
        assert_eq!(input.byte_len(), 150528 * 4);
        let [b, c, h, w] = *input.shape();
        assert_eq!((b, c, h, w), (1, 3, 224, 224));
    }
}
