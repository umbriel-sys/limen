use limen_core::errors::ProcessingError;
use limen_core::traits::configuration::PreprocessorConfiguration;
use limen_core::traits::{Preprocessor, PreprocessorFactory};
use limen_core::types::{DataType, TensorInput};

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, vec::Vec};

use crate::util::parse_shape_csv;

#[derive(Clone, Debug)]
pub struct NormalizePreprocessor {
    scale: f32,
    offset: f32,
    output_shape_override: Option<alloc::boxed::Box<[usize]>>,
}

impl Preprocessor for NormalizePreprocessor {
    fn process(
        &mut self,
        sensor_data: limen_core::types::SensorData<'_>,
    ) -> Result<TensorInput, ProcessingError> {
        let shape = if let Some(s) = &self.output_shape_override {
            s.clone()
        } else {
            alloc::boxed::Box::from(vec![sensor_data.payload.len()])
        };
        let element_count = shape.iter().copied().product::<usize>();
        if element_count != sensor_data.payload.len() {
            return Err(ProcessingError::InvalidData {
                message: "output shape does not match the number of input bytes".to_string(),
            });
        }

        let mut buffer = Vec::with_capacity(element_count * core::mem::size_of::<f32>());
        for &b in sensor_data.payload.iter() {
            let value = self.scale * (b as f32) + self.offset;
            buffer.extend_from_slice(&value.to_le_bytes());
        }

        TensorInput::new(DataType::Float32, shape, None, buffer).map_err(|e| {
            ProcessingError::InvalidData {
                message: e.to_string(),
            }
        })
    }

    fn reset(&mut self) -> Result<(), ProcessingError> {
        Ok(())
    }
}

pub struct NormalizePreprocessorFactory;

impl PreprocessorFactory for NormalizePreprocessorFactory {
    fn preprocessor_name(&self) -> &'static str {
        "normalize"
    }

    fn create_preprocessor(
        &self,
        configuration: &PreprocessorConfiguration,
    ) -> Result<Box<dyn Preprocessor>, ProcessingError> {
        let mut scale: f32 = 1.0 / 255.0;
        let mut offset: f32 = 0.0;
        let mut output_shape_override: Option<alloc::boxed::Box<[usize]>> = None;

        #[cfg(feature = "alloc")]
        {
            if let Some(s) = configuration.parameters.get("scale") {
                scale = s.parse::<f32>().map_err(|_| ProcessingError::InvalidData {
                    message: "invalid 'scale' parameter".to_string(),
                })?;
            }
            if let Some(s) = configuration.parameters.get("offset") {
                offset = s.parse::<f32>().map_err(|_| ProcessingError::InvalidData {
                    message: "invalid 'offset' parameter".to_string(),
                })?;
            }
            if let Some(s) = configuration.parameters.get("output_shape") {
                let parsed =
                    parse_shape_csv(s).map_err(|m| ProcessingError::InvalidData { message: m })?;
                output_shape_override = Some(alloc::boxed::Box::from(parsed));
            }
        }

        Ok(Box::new(NormalizePreprocessor {
            scale,
            offset,
            output_shape_override,
        }))
    }
}
