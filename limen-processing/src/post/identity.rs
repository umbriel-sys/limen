use limen_core::errors::ProcessingError;
use limen_core::traits::configuration::PostprocessorConfiguration;
use limen_core::traits::{Postprocessor, PostprocessorFactory};
use limen_core::types::TensorOutput;

#[cfg(feature = "alloc")]
use alloc::boxed::Box;

#[derive(Clone, Debug, Default)]
pub struct IdentityPostprocessor;

impl Postprocessor for IdentityPostprocessor {
    fn process(&mut self, model_output: TensorOutput) -> Result<TensorOutput, ProcessingError> {
        Ok(model_output)
    }
    fn reset(&mut self) -> Result<(), ProcessingError> {
        Ok(())
    }
}

pub struct IdentityPostprocessorFactory;
impl PostprocessorFactory for IdentityPostprocessorFactory {
    fn postprocessor_name(&self) -> &'static str {
        "identity"
    }
    fn create_postprocessor(
        &self,
        _configuration: &PostprocessorConfiguration,
    ) -> Result<Box<dyn Postprocessor>, ProcessingError> {
        Ok(Box::new(IdentityPostprocessor::default()))
    }
}
