#![cfg_attr(not(feature = "std"), no_std)]
use limen_core::errors::{SensorError, ProcessingError, InferenceError, OutputError};
use limen_core::types::DataType;
use crate::no_alloc::types::{BorrowedTensorView, BorrowedTensorViewMut, FixedShape, SensorFrameMeta};

pub trait SensorSourceBorrowed {
    fn open(&mut self) -> Result<(), SensorError> { Ok(()) }
    fn read_next_into<'a>(&'a mut self, destination_buffer: &'a mut [u8]) -> Result<Option<(usize, SensorFrameMeta)>, SensorError>;
    fn reset(&mut self) -> Result<(), SensorError> { Ok(()) }
    fn close(&mut self) -> Result<(), SensorError> { Ok(()) }
}
pub trait PreprocessorNoAlloc<const MAX_DIMS: usize> {
    fn process(
        &mut self,
        sensor_bytes: &[u8],
        output_data_type: &mut DataType,
        output_shape: &mut FixedShape<MAX_DIMS>,
        output_buffer: &mut [u8],
    ) -> Result<usize, ProcessingError>;
    fn reset(&mut self) -> Result<(), ProcessingError> { Ok(()) }
}
pub trait ModelNoAlloc<const MAX_DIMS: usize> {
    fn infer(
        &mut self,
        input: BorrowedTensorView<'_>,
        output_data_type: &mut DataType,
        output_shape: &mut FixedShape<MAX_DIMS>,
        output_buffer: &mut [u8],
    ) -> Result<usize, InferenceError>;
    fn unload(&mut self) -> Result<(), InferenceError> { Ok(()) }
}
pub trait PostprocessorNoAlloc<const MAX_DIMS: usize> {
    fn process(
        &mut self,
        model_output: BorrowedTensorView<'_>,
        output_data_type: &mut DataType,
        output_shape: &mut FixedShape<MAX_DIMS>,
        output_buffer: &mut [u8],
    ) -> Result<usize, ProcessingError>;
    fn reset(&mut self) -> Result<(), ProcessingError> { Ok(()) }
}
pub trait OutputSinkNoAlloc {
    fn write(&mut self, output: BorrowedTensorView<'_>) -> Result<(), OutputError>;
    fn flush(&mut self) -> Result<(), OutputError> { Ok(()) }
    fn close(&mut self) -> Result<(), OutputError> { Ok(()) }
}
