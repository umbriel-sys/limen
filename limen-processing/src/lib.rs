#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod util;

#[cfg(feature = "alloc")]
pub mod pre;

#[cfg(feature = "alloc")]
pub mod post;

#[cfg(all(feature = "alloc", feature = "register"))]
pub fn register_all(
    registries: &mut limen::Registries,
) -> Result<(), limen::registry::RegistryError> {
    use post::{
        debounce::DebouncePostprocessorFactory, identity::IdentityPostprocessorFactory,
        threshold::ThresholdPostprocessorFactory,
    };
    use pre::{
        identity::IdentityPreprocessorFactory, normalize::NormalizePreprocessorFactory,
        window::WindowPreprocessorFactory,
    };

    registries.register_preprocessor_factory_with_name(
        "identity".to_string(),
        alloc::boxed::Box::new(IdentityPreprocessorFactory),
    )?;
    registries.register_preprocessor_factory_with_name(
        "normalize".to_string(),
        alloc::boxed::Box::new(NormalizePreprocessorFactory),
    )?;
    registries.register_preprocessor_factory_with_name(
        "window".to_string(),
        alloc::boxed::Box::new(WindowPreprocessorFactory),
    )?;

    registries.register_postprocessor_factory_with_name(
        "identity".to_string(),
        alloc::boxed::Box::new(IdentityPostprocessorFactory),
    )?;
    registries.register_postprocessor_factory_with_name(
        "threshold".to_string(),
        alloc::boxed::Box::new(ThresholdPostprocessorFactory),
    )?;
    registries.register_postprocessor_factory_with_name(
        "debounce".to_string(),
        alloc::boxed::Box::new(DebouncePostprocessorFactory),
    )?;

    Ok(())
}
