#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
use alloc::boxed::Box;

use limen_core::errors::{InferenceError, OutputError, ProcessingError, SensorError};
use limen_core::traits::{Model, OutputSink, Postprocessor, Preprocessor, SensorStream};
use limen_core::types::{ModelMetadata, SensorData, SensorSampleMetadata, TensorInput, TensorOutput};

#[cfg(feature = "alloc")]
pub struct BoxedSensorStream(pub Box<dyn SensorStream>);
#[cfg(feature = "alloc")]
impl SensorStream for BoxedSensorStream {
    fn open(&mut self) -> Result<(), SensorError> { self.0.open() }
    fn read_next<'a>(&'a mut self) -> Result<Option<SensorData<'a>>, SensorError> { self.0.read_next() }
    fn reset(&mut self) -> Result<(), SensorError> { self.0.reset() }
    fn close(&mut self) -> Result<(), SensorError> { self.0.close() }
    fn describe(&self) -> Option<SensorSampleMetadata> { self.0.describe() }
}

#[cfg(feature = "alloc")]
pub struct BoxedPreprocessor(pub Box<dyn Preprocessor>);
#[cfg(feature = "alloc")]
impl Preprocessor for BoxedPreprocessor {
    fn process(&mut self, sensor_data: SensorData<'_>) -> Result<TensorInput, ProcessingError> { self.0.process(sensor_data) }
    fn reset(&mut self) -> Result<(), ProcessingError> { self.0.reset() }
}

#[cfg(feature = "alloc")]
pub struct BoxedModel(pub Box<dyn Model>);
#[cfg(feature = "alloc")]
impl Model for BoxedModel {
    fn model_metadata(&self) -> ModelMetadata { self.0.model_metadata() }
    fn infer(&mut self, tensor_input: &TensorInput) -> Result<TensorOutput, InferenceError> { self.0.infer(tensor_input) }
    fn unload(&mut self) -> Result<(), InferenceError> { self.0.unload() }
}

#[cfg(feature = "alloc")]
pub struct BoxedPostprocessor(pub Box<dyn Postprocessor>);
#[cfg(feature = "alloc")]
impl Postprocessor for BoxedPostprocessor {
    fn process(&mut self, model_output: TensorOutput) -> Result<TensorOutput, ProcessingError> { self.0.process(model_output) }
    fn reset(&mut self) -> Result<(), ProcessingError> { self.0.reset() }
}

#[cfg(feature = "alloc")]
pub struct BoxedOutputSink(pub Box<dyn OutputSink>);
#[cfg(feature = "alloc")]
impl OutputSink for BoxedOutputSink {
    fn write(&mut self, output: &TensorOutput) -> Result<(), OutputError> { self.0.write(output) }
    fn flush(&mut self) -> Result<(), OutputError> { self.0.flush() }
    fn close(&mut self) -> Result<(), OutputError> { self.0.close() }
}
