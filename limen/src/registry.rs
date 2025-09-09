#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, collections::BTreeMap, string::String};

use core::fmt;

use limen_core::traits::{
    ComputeBackendFactory, OutputSinkFactory, PlatformBackendFactory, PostprocessorFactory,
    PreprocessorFactory, SensorStreamFactory,
};

#[derive(Debug)]
pub enum RegistryError {
    DuplicateFactoryRegistration { kind: &'static str, name: String },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::DuplicateFactoryRegistration { kind, name } => {
                write!(f, "duplicate factory registration for {} named '{}'", kind, name)
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RegistryError {}

#[cfg(feature = "alloc")]
#[derive(Default)]
pub struct Registries {
    compute_backend_factories: BTreeMap<String, Box<dyn ComputeBackendFactory>>,
    sensor_stream_factories: BTreeMap<String, Box<dyn SensorStreamFactory>>,
    preprocessor_factories: BTreeMap<String, Box<dyn PreprocessorFactory>>,
    postprocessor_factories: BTreeMap<String, Box<dyn PostprocessorFactory>>,
    output_sink_factories: BTreeMap<String, Box<dyn OutputSinkFactory>>,
    platform_backend_factories: BTreeMap<String, Box<dyn PlatformBackendFactory>>,
}

#[cfg(feature = "alloc")]
impl Registries {
    pub fn new() -> Self { Self::default() }

    fn insert_unique<T, F>(
        &mut self,
        kind: &'static str,
        name: String,
        factory: T,
        map: F,
    ) -> Result<(), RegistryError>
    where
        F: FnOnce(&mut Self) -> &mut BTreeMap<String, T>,
    {
        let factories = map(self);
        if factories.contains_key(&name) {
            return Err(RegistryError::DuplicateFactoryRegistration { kind, name });
        }
        factories.insert(name, factory);
        Ok(())
    }

    pub fn register_compute_backend_factory_with_name(
        &mut self,
        name: String,
        factory: Box<dyn ComputeBackendFactory>,
    ) -> Result<(), RegistryError> {
        self.insert_unique("compute_backend", name, factory, |s| &mut s.compute_backend_factories)
    }
    pub fn register_compute_backend_factory(&mut self, factory: Box<dyn ComputeBackendFactory>) -> Result<(), RegistryError> {
        let name = factory.backend_name().to_string();
        self.register_compute_backend_factory_with_name(name, factory)
    }
    pub fn get_compute_backend_factory(&self, name: &str) -> Option<&Box<dyn ComputeBackendFactory>> {
        self.compute_backend_factories.get(name)
    }

    pub fn register_sensor_stream_factory_with_name(
        &mut self,
        name: String,
        factory: Box<dyn SensorStreamFactory>,
    ) -> Result<(), RegistryError> {
        self.insert_unique("sensor_stream", name, factory, |s| &mut s.sensor_stream_factories)
    }
    pub fn register_sensor_stream_factory(&mut self, factory: Box<dyn SensorStreamFactory>) -> Result<(), RegistryError> {
        let name = factory.sensor_name().to_string();
        self.register_sensor_stream_factory_with_name(name, factory)
    }
    pub fn get_sensor_stream_factory(&self, name: &str) -> Option<&Box<dyn SensorStreamFactory>> {
        self.sensor_stream_factories.get(name)
    }

    pub fn register_preprocessor_factory_with_name(
        &mut self,
        name: String,
        factory: Box<dyn PreprocessorFactory>,
    ) -> Result<(), RegistryError> {
        self.insert_unique("preprocessor", name, factory, |s| &mut s.preprocessor_factories)
    }
    pub fn register_preprocessor_factory(&mut self, factory: Box<dyn PreprocessorFactory>) -> Result<(), RegistryError> {
        let name = factory.preprocessor_name().to_string();
        self.register_preprocessor_factory_with_name(name, factory)
    }
    pub fn get_preprocessor_factory(&self, name: &str) -> Option<&Box<dyn PreprocessorFactory>> {
        self.preprocessor_factories.get(name)
    }

    pub fn register_postprocessor_factory_with_name(
        &mut self,
        name: String,
        factory: Box<dyn PostprocessorFactory>,
    ) -> Result<(), RegistryError> {
        self.insert_unique("postprocessor", name, factory, |s| &mut s.postprocessor_factories)
    }
    pub fn register_postprocessor_factory(&mut self, factory: Box<dyn PostprocessorFactory>) -> Result<(), RegistryError> {
        let name = factory.postprocessor_name().to_string();
        self.register_postprocessor_factory_with_name(name, factory)
    }
    pub fn get_postprocessor_factory(&self, name: &str) -> Option<&Box<dyn PostprocessorFactory>> {
        self.postprocessor_factories.get(name)
    }

    pub fn register_output_sink_factory_with_name(
        &mut self,
        name: String,
        factory: Box<dyn OutputSinkFactory>,
    ) -> Result<(), RegistryError> {
        self.insert_unique("output_sink", name, factory, |s| &mut s.output_sink_factories)
    }
    pub fn register_output_sink_factory(&mut self, factory: Box<dyn OutputSinkFactory>) -> Result<(), RegistryError> {
        let name = factory.sink_name().to_string();
        self.register_output_sink_factory_with_name(name, factory)
    }
    pub fn get_output_sink_factory(&self, name: &str) -> Option<&Box<dyn OutputSinkFactory>> {
        self.output_sink_factories.get(name)
    }

    pub fn register_platform_backend_factory_with_name(
        &mut self,
        name: String,
        factory: Box<dyn PlatformBackendFactory>,
    ) -> Result<(), RegistryError> {
        self.insert_unique("platform_backend", name, factory, |s| &mut s.platform_backend_factories)
    }
    pub fn register_platform_backend_factory(&mut self, factory: Box<dyn PlatformBackendFactory>) -> Result<(), RegistryError> {
        let name = factory.platform_name().to_string();
        self.register_platform_backend_factory_with_name(name, factory)
    }
    pub fn get_platform_backend_factory(&self, name: &str) -> Option<&Box<dyn PlatformBackendFactory>> {
        self.platform_backend_factories.get(name)
    }
}
