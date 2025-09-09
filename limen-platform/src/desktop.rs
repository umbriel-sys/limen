use limen_core::errors::RuntimeError;
use limen_core::traits::configuration::PlatformBackendConfiguration;
use limen_core::traits::platform::PlatformConstraints;
use limen_core::traits::{PlatformBackend, PlatformBackendFactory};

pub struct DesktopPlatformBackend {
    constraints: PlatformConstraints,
}

impl DesktopPlatformBackend {
    fn detect_recommended_worker_thread_count() -> u16 {
        #[cfg(feature = "std")]
        {
            #[cfg(feature = "num_cpus")]
            {
                let count = num_cpus::get().max(1);
                return u16::try_from(count).unwrap_or(u16::MAX);
            }
        }
        4
    }
    fn default_constraints() -> PlatformConstraints {
        PlatformConstraints {
            maximum_concurrent_model_tasks: 1,
            has_general_purpose_input_output_pins: false,
            has_graphics_processing_unit: false,
            recommended_worker_thread_count: Self::detect_recommended_worker_thread_count(),
        }
    }
}

impl PlatformBackend for DesktopPlatformBackend {
    fn constraints(&self) -> PlatformConstraints {
        self.constraints
    }
    fn initialize(&mut self) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn shutdown(&mut self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

pub struct DesktopPlatformBackendFactory;
impl PlatformBackendFactory for DesktopPlatformBackendFactory {
    fn platform_name(&self) -> &'static str {
        "desktop"
    }
    fn create_platform_backend(
        &self,
        _configuration: &PlatformBackendConfiguration,
    ) -> Result<alloc::boxed::Box<dyn PlatformBackend>, RuntimeError> {
        Ok(alloc::boxed::Box::new(DesktopPlatformBackend {
            constraints: DesktopPlatformBackend::default_constraints(),
        }))
    }
}
