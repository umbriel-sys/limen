#![cfg_attr(not(feature = "std"), no_std)]
use heapless::{Vec as FixedVec, Vec};
use limen_core::types::{DataType, SequenceNumber, TimestampNanosecondsSinceUnixEpoch};

pub struct BorrowedTensorView<'a> {
    pub data_type: DataType,
    pub shape: &'a [usize],
    pub buffer: &'a [u8],
}
pub struct BorrowedTensorViewMut<'a> {
    pub data_type: DataType,
    pub shape: &'a [usize],
    pub buffer: &'a mut [u8],
}
pub struct FixedShape<const MAX_DIMS: usize> {
    inner: FixedVec<usize, MAX_DIMS>,
}
impl<const MAX_DIMS: usize> FixedShape<MAX_DIMS> {
    pub const fn new() -> Self { Self { inner: FixedVec::new() } }
    pub fn as_slice(&self) -> &[usize] { &self.inner }
    pub fn clear(&mut self) { self.inner.clear() }
    pub fn push(&mut self, dim: usize) -> Result<(), ()> { self.inner.push(dim).map_err(|_| ()) }
    pub fn extend_from_slice(&mut self, dims: &[usize]) -> Result<(), ()> { for &d in dims { self.inner.push(d).map_err(|_| ())?; } Ok(()) }
    pub fn product(&self) -> usize { self.inner.iter().copied().product::<usize>() }
}
#[derive(Clone, Copy)]
pub struct SensorFrameMeta {
    pub timestamp: TimestampNanosecondsSinceUnixEpoch,
    pub sequence_number: SequenceNumber,
}
pub struct TensorFrame<const MAX_BYTES: usize, const MAX_DIMS: usize> {
    pub data_type: DataType,
    pub shape: FixedShape<MAX_DIMS>,
    pub bytes: Vec<u8, MAX_BYTES>,
}
impl<const MAX_BYTES: usize, const MAX_DIMS: usize> TensorFrame<MAX_BYTES, MAX_DIMS> {
    pub fn try_from_parts(data_type: DataType, shape_slice: &[usize], data: &[u8]) -> Result<Self, ()> {
        let mut bytes: Vec<u8, MAX_BYTES> = Vec::new();
        if bytes.extend_from_slice(data).is_err() { return Err(()); }
        let mut shape: FixedShape<MAX_DIMS> = FixedShape::new();
        shape.extend_from_slice(shape_slice)?;
        Ok(Self { data_type, shape, bytes })
    }
}
