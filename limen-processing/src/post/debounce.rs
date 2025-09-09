use limen_core::errors::ProcessingError;
use limen_core::traits::configuration::PostprocessorConfiguration;
use limen_core::traits::{Postprocessor, PostprocessorFactory};
use limen_core::types::{DataType, TensorOutput};

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, vec::Vec};

#[derive(Clone, Debug)]
pub struct DebouncePostprocessor {
    activation_count: u32,
    deactivation_count: u32,
    debounced_active: bool,
    consecutive_active: u32,
    consecutive_inactive: u32,
}

impl DebouncePostprocessor {
    fn push_value(&mut self, active_now: bool) {
        if active_now {
            self.consecutive_active = self.consecutive_active.saturating_add(1);
            self.consecutive_inactive = 0;
            if !self.debounced_active && self.consecutive_active >= self.activation_count {
                self.debounced_active = true;
            }
        } else {
            self.consecutive_inactive = self.consecutive_inactive.saturating_add(1);
            self.consecutive_active = 0;
            if self.debounced_active && self.consecutive_inactive >= self.deactivation_count {
                self.debounced_active = false;
            }
        }
    }
}

impl Postprocessor for DebouncePostprocessor {
    fn process(&mut self, model_output: TensorOutput) -> Result<TensorOutput, ProcessingError> {
        if model_output.data_type != DataType::Unsigned8 {
            return Err(ProcessingError::InvalidData {
                message: "debounce postprocessor expects Unsigned8 input".to_string(),
            });
        }
        if model_output.buffer.is_empty() {
            return Err(ProcessingError::InvalidData {
                message: "model output buffer is empty".to_string(),
            });
        }
        let active_now = model_output.buffer[0] != 0;
        self.push_value(active_now);
        let out_value: u8 = if self.debounced_active { 1 } else { 0 };
        let output = TensorOutput::new(
            DataType::Unsigned8,
            alloc::boxed::Box::from(vec![1usize]),
            None,
            vec![out_value],
        )
        .map_err(|e| ProcessingError::InvalidData {
            message: e.to_string(),
        })?;
        Ok(output)
    }
    fn reset(&mut self) -> Result<(), ProcessingError> {
        self.debounced_active = false;
        self.consecutive_active = 0;
        self.consecutive_inactive = 0;
        Ok(())
    }
}

pub struct DebouncePostprocessorFactory;

impl PostprocessorFactory for DebouncePostprocessorFactory {
    fn postprocessor_name(&self) -> &'static str {
        "debounce"
    }
    fn create_postprocessor(
        &self,
        configuration: &PostprocessorConfiguration,
    ) -> Result<Box<dyn Postprocessor>, ProcessingError> {
        let mut activation_count: u32 = 1;
        let mut deactivation_count: u32 = 1;
        #[cfg(feature = "alloc")]
        {
            if let Some(s) = configuration.parameters.get("activation_count") {
                activation_count = s.parse::<u32>().map_err(|_| ProcessingError::InvalidData {
                    message: "invalid 'activation_count' parameter".to_string(),
                })?;
                if activation_count == 0 {
                    return Err(ProcessingError::InvalidData {
                        message: "'activation_count' must be greater than zero".to_string(),
                    });
                }
            }
            if let Some(s) = configuration.parameters.get("deactivation_count") {
                deactivation_count =
                    s.parse::<u32>().map_err(|_| ProcessingError::InvalidData {
                        message: "invalid 'deactivation_count' parameter".to_string(),
                    })?;
                if deactivation_count == 0 {
                    return Err(ProcessingError::InvalidData {
                        message: "'deactivation_count' must be greater than zero".to_string(),
                    });
                }
            }
        }
        Ok(Box::new(DebouncePostprocessor {
            activation_count,
            deactivation_count,
            debounced_active: false,
            consecutive_active: 0,
            consecutive_inactive: 0,
        }))
    }
}
