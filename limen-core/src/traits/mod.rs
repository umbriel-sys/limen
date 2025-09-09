#![cfg_attr(not(feature = "std"), no_std)]

pub mod configuration;
pub mod platform;

use crate::errors::{InferenceError, OutputError, ProcessingError, RuntimeError, SensorError};
use crate::types::{ModelMetadata, SensorData, TensorInput, TensorOutput};

pub trait SensorStream {
    fn open(&mut self) -> Result<(), SensorError>;
    fn read_next<'a>(&'a mut self) -> Result<Option<SensorData<'a>>, SensorError>;
    fn reset(&mut self) -> Result<(), SensorError>;
    fn close(&mut self) -> Result<(), SensorError>;
    fn describe(&self) -> Option<crate::types::SensorSampleMetadata>;
}

pub trait Preprocessor {
    fn process(
        &mut self,
        sensor_data: crate::types::SensorData<'_>,
    ) -> Result<TensorInput, ProcessingError>;
    fn reset(&mut self) -> Result<(), ProcessingError>;
}

pub trait Model {
    fn model_metadata(&self) -> ModelMetadata;
    fn infer(&mut self, tensor_input: &TensorInput) -> Result<TensorOutput, InferenceError>;
    fn unload(&mut self) -> Result<(), InferenceError>;
}

pub trait Postprocessor {
    fn process(&mut self, model_output: TensorOutput) -> Result<TensorOutput, ProcessingError>;
    fn reset(&mut self) -> Result<(), ProcessingError>;
}

pub trait OutputSink {
    fn write(&mut self, output: &TensorOutput) -> Result<(), OutputError>;
    fn flush(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), OutputError>;
}

pub trait SensorStreamFactory {
    fn sensor_name(&self) -> &'static str;
    fn create_sensor_stream(
        &self,
        configuration: &configuration::SensorStreamConfiguration,
    ) -> Result<alloc::boxed::Box<dyn SensorStream>, SensorError>;
}

pub trait PreprocessorFactory {
    fn preprocessor_name(&self) -> &'static str;
    fn create_preprocessor(
        &self,
        configuration: &configuration::PreprocessorConfiguration,
    ) -> Result<alloc::boxed::Box<dyn Preprocessor>, ProcessingError>;
}

pub trait PostprocessorFactory {
    fn postprocessor_name(&self) -> &'static str;
    fn create_postprocessor(
        &self,
        configuration: &configuration::PostprocessorConfiguration,
    ) -> Result<alloc::boxed::Box<dyn Postprocessor>, ProcessingError>;
}

pub trait OutputSinkFactory {
    fn sink_name(&self) -> &'static str;
    fn create_output_sink(
        &self,
        configuration: &configuration::OutputSinkConfiguration,
    ) -> Result<alloc::boxed::Box<dyn OutputSink>, OutputError>;
}

pub trait ComputeBackend {
    fn load_model(
        &mut self,
        configuration: &configuration::ComputeBackendConfiguration,
        metadata_hint: Option<&ModelMetadata>,
    ) -> Result<alloc::boxed::Box<dyn Model>, InferenceError>;
}

pub trait ComputeBackendFactory {
    fn backend_name(&self) -> &'static str;
    fn create_backend(&self) -> Result<alloc::boxed::Box<dyn ComputeBackend>, InferenceError>;
}

pub trait PlatformBackend {
    fn constraints(&self) -> platform::PlatformConstraints;
    fn initialize(&mut self) -> Result<(), RuntimeError>;
    fn shutdown(&mut self) -> Result<(), RuntimeError>;
}

pub trait PlatformBackendFactory {
    fn platform_name(&self) -> &'static str;
    fn create_platform_backend(
        &self,
        configuration: &configuration::PlatformBackendConfiguration,
    ) -> Result<alloc::boxed::Box<dyn PlatformBackend>, RuntimeError>;
}
