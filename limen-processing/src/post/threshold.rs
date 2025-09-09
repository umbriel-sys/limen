use limen_core::errors::ProcessingError;
use limen_core::traits::{Postprocessor, PostprocessorFactory};
use limen_core::traits::configuration::PostprocessorConfiguration;
use limen_core::types::{DataType, TensorOutput};

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, vec::Vec};

#[derive(Clone, Debug)]
pub struct ThresholdPostprocessor { threshold: f32 }

impl Postprocessor for ThresholdPostprocessor {
    fn process(&mut self, model_output: TensorOutput) -> Result<TensorOutput, ProcessingError> {
        if model_output.data_type != DataType::Float32 {
            return Err(ProcessingError::InvalidData { message: "threshold postprocessor expects Float32 input".to_string() });
        }
        if model_output.buffer.len() < core::mem::size_of::<f32>() {
            return Err(ProcessingError::InvalidData { message: "model output buffer too small for Float32 element".to_string() });
        }
        let bytes = &model_output.buffer[..4];
        let first = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let active: u8 = if first >= self.threshold { 1 } else { 0 };
        let output = TensorOutput::new(DataType::Unsigned8, alloc::boxed::Box::from(vec![1usize]), None, vec![active])
            .map_err(|e| ProcessingError::InvalidData { message: e.to_string() })?;
        Ok(output)
    }
    fn reset(&mut self) -> Result<(), ProcessingError> { Ok(()) }
}

pub struct ThresholdPostprocessorFactory;

impl PostprocessorFactory for ThresholdPostprocessorFactory {
    fn postprocessor_name(&self) -> &'static str { "threshold" }
    fn create_postprocessor(&self, configuration: &PostprocessorConfiguration) -> Result<Box<dyn Postprocessor>, ProcessingError> {
        let mut threshold_value: Option<f32> = None;
        #[cfg(feature = "alloc")]
        {
            if let Some(s) = configuration.parameters.get("threshold") {
                threshold_value = Some(s.parse::<f32>().map_err(|_| ProcessingError::InvalidData { message: "invalid 'threshold' parameter".to_string() })?);
            }
        }
        let threshold = threshold_value.ok_or_else(|| ProcessingError::InvalidData { message: "threshold postprocessor requires 'threshold' parameter".to_string() })?;
        Ok(Box::new(ThresholdPostprocessor { threshold }))
    }
}
