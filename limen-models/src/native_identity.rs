use limen_core::errors::InferenceError;
use limen_core::traits::{ComputeBackend, ComputeBackendFactory, Model};
use limen_core::traits::configuration::ComputeBackendConfiguration;
use limen_core::types::{ModelMetadata, TensorInput, TensorOutput};

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, vec::Vec};

#[derive(Debug, Default)]
pub struct NativeIdentityModel { declared_metadata: ModelMetadata }

impl Model for NativeIdentityModel {
    fn model_metadata(&self) -> ModelMetadata { self.declared_metadata.clone() }

    fn infer(&mut self, tensor_input: &TensorInput) -> Result<TensorOutput, InferenceError> {
        let buffer: Vec<u8> = tensor_input.buffer.clone();
        TensorOutput::new(tensor_input.data_type, tensor_input.shape.clone(), tensor_input.strides.clone(), buffer)
            .map_err(|e| InferenceError::Other { message: e.to_string() })
    }

    fn unload(&mut self) -> Result<(), InferenceError> { Ok(()) }
}

#[derive(Debug, Default)]
pub struct NativeIdentityComputeBackend;

impl ComputeBackend for NativeIdentityComputeBackend {
    fn load_model(&mut self, _configuration: &ComputeBackendConfiguration, metadata_hint: Option<&ModelMetadata>) -> Result<Box<dyn Model>, InferenceError> {
        let declared_metadata = metadata_hint.cloned().unwrap_or_else(ModelMetadata::empty);
        Ok(Box::new(NativeIdentityModel { declared_metadata }))
    }
}

pub struct NativeIdentityComputeBackendFactory;
impl ComputeBackendFactory for NativeIdentityComputeBackendFactory {
    fn backend_name(&self) -> &'static str { "native-identity" }
    fn create_backend(&self) -> Result<Box<dyn ComputeBackend>, InferenceError> { Ok(Box::new(NativeIdentityComputeBackend::default())) }
}
