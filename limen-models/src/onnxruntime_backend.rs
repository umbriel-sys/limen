use limen_core::errors::InferenceError;
use limen_core::traits::{ComputeBackend, ComputeBackendFactory, Model};
use limen_core::traits::configuration::ComputeBackendConfiguration;
use limen_core::types::{ModelMetadata, TensorInput, TensorOutput};

#[cfg(feature = "alloc")]
use alloc::boxed::Box;

#[derive(Debug, Default)]
pub struct OnnxRuntimeModel;

impl Model for OnnxRuntimeModel {
    fn model_metadata(&self) -> ModelMetadata { ModelMetadata::empty() }
    fn infer(&mut self, _tensor_input: &TensorInput) -> Result<TensorOutput, InferenceError> {
        Err(InferenceError::BackendUnavailable)
    }
    fn unload(&mut self) -> Result<(), InferenceError> { Ok(()) }
}

#[derive(Debug, Default)]
pub struct OnnxRuntimeComputeBackend;
impl ComputeBackend for OnnxRuntimeComputeBackend {
    fn load_model(&mut self, _configuration: &ComputeBackendConfiguration, _metadata_hint: Option<&ModelMetadata>) -> Result<Box<dyn Model>, InferenceError> {
        Err(InferenceError::BackendUnavailable)
    }
}

pub struct OnnxRuntimeComputeBackendFactory;
impl ComputeBackendFactory for OnnxRuntimeComputeBackendFactory {
    fn backend_name(&self) -> &'static str { "onnxruntime" }
    fn create_backend(&self) -> Result<Box<dyn ComputeBackend>, InferenceError> { Ok(Box::new(OnnxRuntimeComputeBackend::default())) }
}
