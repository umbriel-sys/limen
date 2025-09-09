#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
use alloc::boxed::Box;

use core::fmt;

use crate::boxed::{
    BoxedModel, BoxedOutputSink, BoxedPostprocessor, BoxedPreprocessor, BoxedSensorStream,
};
use crate::config::RuntimeConfiguration;
use crate::registry::Registries;

use limen_core::errors::{InferenceError, OutputError, ProcessingError, RuntimeError, SensorError};
use limen_core::runtime::Runtime;
use limen_core::traits::configuration::*;
use limen_core::traits::{
    ComputeBackend, Model, OutputSink, PlatformBackend, Postprocessor, Preprocessor, SensorStream,
};

#[derive(Debug)]
pub enum BuildError {
    FactoryNotFound {
        component: &'static str,
        factory_name: alloc::string::String,
    },
    ComputeBackendCreationFailed(InferenceError),
    ModelLoadFailed(InferenceError),
    SensorStreamCreationFailed(SensorError),
    PreprocessorCreationFailed(ProcessingError),
    PostprocessorCreationFailed(ProcessingError),
    OutputSinkCreationFailed(OutputError),
    PlatformBackendCreationFailed(RuntimeError),
    PlatformBackendInitializationFailed(RuntimeError),
    InvalidConfiguration(alloc::string::String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use BuildError::*;
        match self {
            FactoryNotFound {
                component,
                factory_name,
            } => write!(f, "factory not found for {}: '{}'", component, factory_name),
            ComputeBackendCreationFailed(e) => {
                write!(f, "compute backend creation failed: {:?}", e)
            }
            ModelLoadFailed(e) => write!(f, "model load failed: {:?}", e),
            SensorStreamCreationFailed(e) => write!(f, "sensor creation failed: {:?}", e),
            PreprocessorCreationFailed(e) => write!(f, "preprocessor creation failed: {:?}", e),
            PostprocessorCreationFailed(e) => write!(f, "postprocessor creation failed: {:?}", e),
            OutputSinkCreationFailed(e) => write!(f, "output sink creation failed: {:?}", e),
            PlatformBackendCreationFailed(e) => {
                write!(f, "platform backend creation failed: {:?}", e)
            }
            PlatformBackendInitializationFailed(e) => {
                write!(f, "platform backend initialization failed: {:?}", e)
            }
            InvalidConfiguration(s) => write!(f, "invalid configuration: {}", s),
        }
    }
}
#[cfg(feature = "std")]
impl std::error::Error for BuildError {}

#[cfg(feature = "alloc")]
pub struct BuiltRuntime {
    pub runtime: Runtime<
        BoxedSensorStream,
        BoxedPreprocessor,
        BoxedModel,
        BoxedPostprocessor,
        BoxedOutputSink,
    >,
    pub platform_backend: Option<Box<dyn PlatformBackend>>,
}

#[cfg(feature = "alloc")]
impl BuiltRuntime {
    pub fn runtime(
        &self,
    ) -> &Runtime<
        BoxedSensorStream,
        BoxedPreprocessor,
        BoxedModel,
        BoxedPostprocessor,
        BoxedOutputSink,
    > {
        &self.runtime
    }
    pub fn runtime_mut(
        &mut self,
    ) -> &mut Runtime<
        BoxedSensorStream,
        BoxedPreprocessor,
        BoxedModel,
        BoxedPostprocessor,
        BoxedOutputSink,
    > {
        &mut self.runtime
    }
    pub fn platform_backend(&self) -> Option<&Box<dyn PlatformBackend>> {
        self.platform_backend.as_ref()
    }
}

#[cfg(feature = "alloc")]
pub fn build_from_configuration(
    configuration: &RuntimeConfiguration,
    registries: &Registries,
) -> Result<BuiltRuntime, BuildError> {
    let sensor_factory = registries
        .get_sensor_stream_factory(&configuration.sensor_stream.factory_name)
        .ok_or_else(|| BuildError::FactoryNotFound {
            component: "sensor_stream",
            factory_name: configuration.sensor_stream.factory_name.clone(),
        })?;

    let sensor_instance = sensor_factory
        .create_sensor_stream(&configuration.sensor_stream.configuration)
        .map_err(BuildError::SensorStreamCreationFailed)?;

    let preprocessor_factory = registries
        .get_preprocessor_factory(&configuration.preprocessor.factory_name)
        .ok_or_else(|| BuildError::FactoryNotFound {
            component: "preprocessor",
            factory_name: configuration.preprocessor.factory_name.clone(),
        })?;

    let preprocessor_instance = preprocessor_factory
        .create_preprocessor(&configuration.preprocessor.configuration)
        .map_err(BuildError::PreprocessorCreationFailed)?;

    let backend_factory = registries
        .get_compute_backend_factory(&configuration.model.compute_backend_factory_name)
        .ok_or_else(|| BuildError::FactoryNotFound {
            component: "compute_backend",
            factory_name: configuration.model.compute_backend_factory_name.clone(),
        })?;

    let mut compute_backend_instance = backend_factory
        .create_backend()
        .map_err(BuildError::ComputeBackendCreationFailed)?;

    let model_instance = compute_backend_instance
        .load_model(
            &configuration.model.compute_backend_configuration,
            configuration.model.metadata_hint.as_ref(),
        )
        .map_err(BuildError::ModelLoadFailed)?;

    let postprocessor_factory = registries
        .get_postprocessor_factory(&configuration.postprocessor.factory_name)
        .ok_or_else(|| BuildError::FactoryNotFound {
            component: "postprocessor",
            factory_name: configuration.postprocessor.factory_name.clone(),
        })?;

    let postprocessor_instance = postprocessor_factory
        .create_postprocessor(&configuration.postprocessor.configuration)
        .map_err(BuildError::PostprocessorCreationFailed)?;

    let output_sink_factory = registries
        .get_output_sink_factory(&configuration.output_sink.factory_name)
        .ok_or_else(|| BuildError::FactoryNotFound {
            component: "output_sink",
            factory_name: configuration.output_sink.factory_name.clone(),
        })?;

    let output_sink_instance = output_sink_factory
        .create_output_sink(&configuration.output_sink.configuration)
        .map_err(BuildError::OutputSinkCreationFailed)?;

    let mut platform_backend: Option<Box<dyn PlatformBackend>> = None;
    if let Some(platform_cfg) = &configuration.platform_backend {
        let platform_factory = registries
            .get_platform_backend_factory(&platform_cfg.factory_name)
            .ok_or_else(|| BuildError::FactoryNotFound {
                component: "platform_backend",
                factory_name: platform_cfg.factory_name.clone(),
            })?;

        let mut platform_instance = platform_factory
            .create_platform_backend(&platform_cfg.configuration)
            .map_err(BuildError::PlatformBackendCreationFailed)?;

        platform_instance
            .initialize()
            .map_err(BuildError::PlatformBackendInitializationFailed)?;

        platform_backend = Some(platform_instance);
    }

    let pre_q_cap = configuration.preprocessor_input_queue_capacity;
    let post_q_cap = configuration.postprocessor_output_queue_capacity;
    if pre_q_cap == 0 || post_q_cap == 0 {
        return Err(BuildError::InvalidConfiguration(
            alloc::string::String::from("queue capacities must be greater than zero"),
        ));
    }

    let pre_policy = configuration
        .backpressure_policy_for_preprocessor_input_queue
        .to_backpressure_policy();
    let post_policy = configuration
        .backpressure_policy_for_postprocessor_output_queue
        .to_backpressure_policy();

    let sensor_wrapped = BoxedSensorStream(sensor_instance);
    let preprocessor_wrapped = BoxedPreprocessor(preprocessor_instance);
    let model_wrapped = BoxedModel(model_instance);
    let postprocessor_wrapped = BoxedPostprocessor(postprocessor_instance);
    let output_sink_wrapped = BoxedOutputSink(output_sink_instance);

    let runtime = Runtime::new(
        sensor_wrapped,
        preprocessor_wrapped,
        model_wrapped,
        postprocessor_wrapped,
        output_sink_wrapped,
        pre_q_cap,
        post_q_cap,
        pre_policy,
        post_policy,
    );

    Ok(BuiltRuntime {
        runtime,
        platform_backend,
    })
}
