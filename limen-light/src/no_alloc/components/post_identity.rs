#![cfg_attr(not(feature = "std"), no_std)]
use limen_core::errors::ProcessingError;
use limen_core::types::DataType;
use crate::no_alloc::traits::PostprocessorNoAlloc;
use crate::no_alloc::types::{BorrowedTensorView, FixedShape};

#[derive(Default)]
pub struct IdentityPostprocessorNoAlloc;
impl<const MAX_DIMS: usize> PostprocessorNoAlloc<MAX_DIMS> for IdentityPostprocessorNoAlloc {
    fn process(&mut self, model_output: BorrowedTensorView<'_>, output_data_type: &mut DataType, output_shape: &mut FixedShape<MAX_DIMS>, output_buffer: &mut [u8]) -> Result<usize, ProcessingError> {
        if output_buffer.len() < model_output.buffer.len() { return Err(ProcessingError::OperationFailed { message: "insufficient buffer".to_string() }); }
        output_buffer[..model_output.buffer.len()].copy_from_slice(model_output.buffer);
        *output_data_type = model_output.data_type;
        output_shape.clear();
        output_shape.extend_from_slice(model_output.shape).map_err(|_| ProcessingError::InvalidData { message: "shape too large".to_string() })?;
        Ok(model_output.buffer.len())
    }
    fn reset(&mut self) -> Result<(), ProcessingError> { Ok(()) }
}
