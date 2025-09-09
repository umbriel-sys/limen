#![cfg_attr(not(feature = "std"), no_std)]
use limen_core::errors::InferenceError;
use limen_core::types::DataType;
use crate::no_alloc::traits::ModelNoAlloc;
use crate::no_alloc::types::{BorrowedTensorView, FixedShape};

#[derive(Default)]
pub struct IdentityModelNoAlloc;
impl<const MAX_DIMS: usize> ModelNoAlloc<MAX_DIMS> for IdentityModelNoAlloc {
    fn infer(&mut self, input: BorrowedTensorView<'_>, output_data_type: &mut DataType, output_shape: &mut FixedShape<MAX_DIMS>, output_buffer: &mut [u8]) -> Result<usize, InferenceError> {
        if output_buffer.len() < input.buffer.len() { return Err(InferenceError::Other { message: "insufficient output buffer".to_string() }); }
        output_buffer[..input.buffer.len()].copy_from_slice(input.buffer);
        *output_data_type = input.data_type;
        output_shape.clear();
        output_shape.extend_from_slice(input.shape).map_err(|_| InferenceError::Other { message: "shape too large".to_string() })?;
        Ok(input.buffer.len())
    }
    fn unload(&mut self) -> Result<(), InferenceError> { Ok(()) }
}
