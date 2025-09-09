#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, string::String};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use limen_core::runtime::BackpressurePolicy;
use limen_core::traits::configuration::{
    ComputeBackendConfiguration, OutputSinkConfiguration, PlatformBackendConfiguration,
    PostprocessorConfiguration, PreprocessorConfiguration, SensorStreamConfiguration,
};
use limen_core::types::ModelMetadata;

#[cfg(feature = "alloc")]
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NamedConfiguration<T> {
    pub factory_name: String,
    pub configuration: T,
}

#[cfg(feature = "alloc")]
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ModelRuntimeConfiguration {
    pub compute_backend_factory_name: String,
    pub compute_backend_configuration: ComputeBackendConfiguration,
    #[cfg_attr(feature = "serde", serde(default))]
    pub metadata_hint: Option<ModelMetadata>,
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum BackpressurePolicySetting {
    Block,
    DropNewest,
    DropOldest,
}
impl BackpressurePolicySetting {
    pub fn to_backpressure_policy(self) -> BackpressurePolicy {
        match self {
            BackpressurePolicySetting::Block => BackpressurePolicy::Block,
            BackpressurePolicySetting::DropNewest => BackpressurePolicy::DropNewest,
            BackpressurePolicySetting::DropOldest => BackpressurePolicy::DropOldest,
        }
    }
}

#[cfg(feature = "alloc")]
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RuntimeConfiguration {
    pub sensor_stream: NamedConfiguration<SensorStreamConfiguration>,
    pub preprocessor: NamedConfiguration<PreprocessorConfiguration>,
    pub model: ModelRuntimeConfiguration,
    pub postprocessor: NamedConfiguration<PostprocessorConfiguration>,
    pub output_sink: NamedConfiguration<OutputSinkConfiguration>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub platform_backend: Option<NamedConfiguration<PlatformBackendConfiguration>>,
    #[cfg_attr(
        feature = "serde",
        serde(default = "RuntimeConfiguration::default_preprocessor_input_queue_capacity")
    )]
    pub preprocessor_input_queue_capacity: usize,
    #[cfg_attr(
        feature = "serde",
        serde(default = "RuntimeConfiguration::default_postprocessor_output_queue_capacity")
    )]
    pub postprocessor_output_queue_capacity: usize,
    #[cfg_attr(
        feature = "serde",
        serde(default = "RuntimeConfiguration::default_backpressure_block")
    )]
    pub backpressure_policy_for_preprocessor_input_queue: BackpressurePolicySetting,
    #[cfg_attr(
        feature = "serde",
        serde(default = "RuntimeConfiguration::default_backpressure_drop_oldest")
    )]
    pub backpressure_policy_for_postprocessor_output_queue: BackpressurePolicySetting,
}

#[cfg(feature = "alloc")]
impl RuntimeConfiguration {
    fn default_preprocessor_input_queue_capacity() -> usize {
        64
    }
    fn default_postprocessor_output_queue_capacity() -> usize {
        64
    }
    fn default_backpressure_block() -> BackpressurePolicySetting {
        BackpressurePolicySetting::Block
    }
    fn default_backpressure_drop_oldest() -> BackpressurePolicySetting {
        BackpressurePolicySetting::DropOldest
    }
}

#[cfg(all(feature = "alloc", feature = "serde", feature = "toml"))]
impl RuntimeConfiguration {
    pub fn from_toml_str(input: &str) -> Result<Self, String> {
        toml::from_str::<Self>(input).map_err(|e| e.to_string())
    }
}

#[cfg(all(feature = "alloc", feature = "serde", feature = "json"))]
impl RuntimeConfiguration {
    pub fn from_json_str(input: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(input).map_err(|e| e.to_string())
    }
}
