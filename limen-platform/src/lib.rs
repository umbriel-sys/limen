#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod desktop;

#[cfg(all(feature = "alloc", feature = "register"))]
pub fn register_all(
    registries: &mut limen::Registries,
) -> Result<(), limen::registry::RegistryError> {
    use alloc::boxed::Box;
    registries.register_platform_backend_factory_with_name(
        "desktop".to_string(),
        Box::new(desktop::DesktopPlatformBackendFactory),
    )?;
    Ok(())
}
