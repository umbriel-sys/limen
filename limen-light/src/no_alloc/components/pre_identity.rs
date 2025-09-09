#![cfg_attr(not(feature = "std"), no_std)]
use limen_core::errors::ProcessingError;
use limen_core::types::DataType;
use crate::no_alloc::traits::PreprocessorNoAlloc;
use crate::no_alloc::types::FixedShape;

pub struct IdentityPreprocessorNoAlloc {
    configured_data_type: DataType,
    configured_shape_override: Option<&'static [usize]>,
}
impl IdentityPreprocessorNoAlloc {
    pub const fn new(configured_data_type: DataType, configured_shape_override: Option<&'static [usize]>) -> Self {
        Self { configured_data_type, configured_shape_override }
    }
}
impl<const MAX_DIMS: usize> PreprocessorNoAlloc<MAX_DIMS> for IdentityPreprocessorNoAlloc {
    fn process(&mut self, sensor_bytes: &[u8], output_data_type: &mut DataType, output_shape: &mut FixedShape<MAX_DIMS>, output_buffer: &mut [u8]) -> Result<usize, ProcessingError> {
        let elem_size = self.configured_data_type.byte_size();
        if elem_size == 0 { return Err(ProcessingError::InvalidData { message: "elem size zero".to_string() }); }
        let element_count = sensor_bytes.len() / elem_size;
        if element_count.checked_mul(elem_size).unwrap_or(usize::MAX) != sensor_bytes.len() { return Err(ProcessingError::InvalidData { message: "len mismatch".to_string() }); }
        let needed = sensor_bytes.len();
        if output_buffer.len() < needed { return Err(ProcessingError::OperationFailed { message: "insufficient buffer".to_string() }); }
        output_buffer[..needed].copy_from_slice(sensor_bytes);
        *output_data_type = self.configured_data_type;
        output_shape.clear();
        if let Some(shape) = self.configured_shape_override { output_shape.extend_from_slice(shape).map_err(|_| ProcessingError::InvalidData { message: "shape too long".to_string() })?; }
        else { output_shape.push(element_count).map_err(|_| ProcessingError::InvalidData { message: "shape too long".to_string() })?; }
        Ok(needed)
    }
}
