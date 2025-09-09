use limen_core::errors::ProcessingError;
use limen_core::traits::{Preprocessor, PreprocessorFactory};
use limen_core::traits::configuration::PreprocessorConfiguration;
use limen_core::types::{DataType, TensorInput};

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, collections::VecDeque, vec::Vec};

use crate::util::parse_shape_csv;

#[derive(Clone, Debug)]
pub struct WindowPreprocessor {
    window_length_elements: usize,
    hop_length_elements: usize,
    output_shape: alloc::boxed::Box<[usize]>,
    buffer: VecDeque<u8>,
}

impl WindowPreprocessor {
    fn validate_shape(window_length_elements: usize, output_shape: &[usize]) -> Result<(), ProcessingError> {
        let product = output_shape.iter().copied().product::<usize>();
        if product != window_length_elements {
            return Err(ProcessingError::InvalidData { message: "output_shape does not multiply to window_length_elements".to_string() });
        }
        Ok(())
    }
}

impl Preprocessor for WindowPreprocessor {
    fn process(&mut self, sensor_data: limen_core::types::SensorData<'_>) -> Result<TensorInput, ProcessingError> {
        for &b in sensor_data.payload.iter() { self.buffer.push_back(b); }
        if self.buffer.len() < self.window_length_elements {
            return Err(ProcessingError::OperationFailed { message: "insufficient data to produce a window".to_string() });
        }
        let mut window_bytes: Vec<u8> = Vec::with_capacity(self.window_length_elements);
        for _ in 0..self.window_length_elements { window_bytes.push(self.buffer.pop_front().unwrap()); }

        if self.hop_length_elements < self.window_length_elements {
            let keep = self.window_length_elements - self.hop_length_elements;
            let start = self.window_length_elements - keep;
            for &b in window_bytes[start..].iter().rev() { self.buffer.push_front(b); }
        }
        if self.hop_length_elements > self.window_length_elements {
            let extra_drop = self.hop_length_elements - self.window_length_elements;
            for _ in 0..extra_drop { if self.buffer.pop_front().is_none() { break; } }
        }

        TensorInput::new(DataType::Unsigned8, self.output_shape.clone(), None, window_bytes)
            .map_err(|e| ProcessingError::InvalidData { message: e.to_string() })
    }

    fn reset(&mut self) -> Result<(), ProcessingError> { self.buffer.clear(); Ok(()) }
}

pub struct WindowPreprocessorFactory;

impl PreprocessorFactory for WindowPreprocessorFactory {
    fn preprocessor_name(&self) -> &'static str { "window" }

    fn create_preprocessor(&self, configuration: &PreprocessorConfiguration) -> Result<Box<dyn Preprocessor>, ProcessingError> {
        let mut window_length_elements: Option<usize> = None;
        let mut hop_length_elements: Option<usize> = None;
        let mut output_shape: Option<alloc::boxed::Box<[usize]>> = None;

        #[cfg(feature = "alloc")]
        {
            if let Some(s) = configuration.parameters.get("window_length_elements") {
                window_length_elements = Some(s.parse::<usize>().map_err(|_| ProcessingError::InvalidData { message: "invalid 'window_length_elements' parameter".to_string() })?);
            }
            if let Some(s) = configuration.parameters.get("hop_length_elements") {
                hop_length_elements = Some(s.parse::<usize>().map_err(|_| ProcessingError::InvalidData { message: "invalid 'hop_length_elements' parameter".to_string() })?);
            }
            if let Some(s) = configuration.parameters.get("output_shape") {
                let parsed = parse_shape_csv(s).map_err(|m| ProcessingError::InvalidData { message: m })?;
                output_shape = Some(alloc::boxed::Box::from(parsed));
            }
        }

        let window_length_elements = window_length_elements.ok_or_else(|| ProcessingError::InvalidData { message: "window preprocessor requires 'window_length_elements' parameter".to_string() })?;
        let hop_length_elements = hop_length_elements.unwrap_or(window_length_elements);

        let output_shape = if let Some(s) = output_shape {
            WindowPreprocessor::validate_shape(window_length_elements, &s)?;
            s
        } else {
            alloc::boxed::Box::from(vec![window_length_elements])
        };

        Ok(Box::new(WindowPreprocessor {
            window_length_elements,
            hop_length_elements,
            output_shape,
            buffer: VecDeque::with_capacity(window_length_elements.saturating_mul(2)),
        }))
    }
}
