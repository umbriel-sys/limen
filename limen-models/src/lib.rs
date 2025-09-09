#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(all(feature = "alloc", feature = "native-identity"))]
pub mod native_identity;

#[cfg(all(feature = "alloc", feature = "backend-tract"))]
pub mod tract_backend;

#[cfg(all(feature = "alloc", feature = "backend-onnxruntime"))]
pub mod onnxruntime_backend;

#[cfg(all(feature = "alloc", feature = "register"))]
pub fn register_all(
    registries: &mut limen::Registries,
) -> Result<(), limen::registry::RegistryError> {
    use alloc::boxed::Box;

    #[cfg(feature = "native-identity")]
    {
        use native_identity::NativeIdentityComputeBackendFactory;
        registries.register_compute_backend_factory_with_name(
            "native-identity".to_string(),
            Box::new(NativeIdentityComputeBackendFactory),
        )?;
    }
    #[cfg(feature = "backend-tract")]
    {
        use tract_backend::TractComputeBackendFactory;
        registries.register_compute_backend_factory_with_name(
            "tract".to_string(),
            Box::new(TractComputeBackendFactory),
        )?;
    }
    #[cfg(feature = "backend-onnxruntime")]
    {
        use onnxruntime_backend::OnnxRuntimeComputeBackendFactory;
        registries.register_compute_backend_factory_with_name(
            "onnxruntime".to_string(),
            Box::new(OnnxRuntimeComputeBackendFactory),
        )?;
    }
    Ok(())
}
