use limen_core::errors::ProcessingError;
use limen_core::traits::{Preprocessor, PreprocessorFactory};
use limen_core::traits::configuration::PreprocessorConfiguration;
use limen_core::types::{DataType, TensorInput};

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, vec::Vec};

use crate::util::{parse_data_type, parse_shape_csv};

#[derive(Clone, Debug)]
pub struct IdentityPreprocessor {
    configured_data_type: DataType,
    configured_shape_override: Option<alloc::boxed::Box<[usize]>>,
}

impl IdentityPreprocessor {
    fn compute_shape_for_payload_len(&self, payload_len_bytes: usize) -> Result<alloc::boxed::Box<[usize]>, ProcessingError> {
        let element_size = self.configured_data_type.byte_size();
        if element_size == 0 {
            return Err(ProcessingError::InvalidData { message: "data type has zero byte size".to_string() });
        }
        if payload_len_bytes % element_size != 0 {
            return Err(ProcessingError::InvalidData { message: "payload length is not a multiple of the selected data type size".to_string() });
        }
        let element_count = payload_len_bytes / element_size;
        Ok(alloc::boxed::Box::from(vec![element_count]))
    }
}

impl Preprocessor for IdentityPreprocessor {
    fn process(&mut self, sensor_data: limen_core::types::SensorData<'_>) -> Result<TensorInput, ProcessingError> {
        let shape = if let Some(s) = &self.configured_shape_override { s.clone() } else { self.compute_shape_for_payload_len(sensor_data.payload.len())? };
        let expected_len = shape.iter().copied().product::<usize>().checked_mul(self.configured_data_type.byte_size())
            .ok_or(ProcessingError::InvalidData { message: "byte length overflow".to_string() })?;
        if expected_len != sensor_data.payload.len() {
            return Err(ProcessingError::InvalidData { message: "payload length does not match the configured shape and data type".to_string() });
        }
        let buffer: Vec<u8> = sensor_data.payload.to_vec();
        TensorInput::new(self.configured_data_type, shape, None, buffer)
            .map_err(|e| ProcessingError::InvalidData { message: e.to_string() })
    }
    fn reset(&mut self) -> Result<(), ProcessingError> { Ok(()) }
}

pub struct IdentityPreprocessorFactory;

impl PreprocessorFactory for IdentityPreprocessorFactory {
    fn preprocessor_name(&self) -> &'static str { "identity" }
    fn create_preprocessor(&self, configuration: &PreprocessorConfiguration) -> Result<Box<dyn Preprocessor>, ProcessingError> {
        let mut data_type: Option<DataType> = None;
        let mut shape_override: Option<alloc::boxed::Box<[usize]>> = None;

        #[cfg(feature = "alloc")]
        {
            if let Some(t) = configuration.parameters.get("data_type") {
                data_type = Some(parse_data_type(t).map_err(|m| ProcessingError::InvalidData { message: m })?);
            }
            if let Some(s) = configuration.parameters.get("shape") {
                let parsed = parse_shape_csv(s).map_err(|m| ProcessingError::InvalidData { message: m })?;
                shape_override = Some(alloc::boxed::Box::from(parsed));
            }
        }

        let data_type = data_type.ok_or_else(|| ProcessingError::InvalidData { message: "identity preprocessor requires 'data_type' parameter".to_string() })?;

        Ok(Box::new(IdentityPreprocessor { configured_data_type: data_type, configured_shape_override: shape_override }))
    }
}
