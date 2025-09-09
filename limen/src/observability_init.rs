#![cfg_attr(not(feature = "std"), no_std)]

#[derive(Debug)]
pub enum ObservabilityInitializationError {
    TracingInitializationFailed(&'static str),
}

#[cfg(feature = "std")]
impl std::fmt::Display for ObservabilityInitializationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObservabilityInitializationError::TracingInitializationFailed(msg) => write!(f, "tracing initialization failed: {}", msg),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ObservabilityInitializationError {}

pub fn initialize_observability() -> Result<(), ObservabilityInitializationError> {
    #[cfg(all(feature = "std", feature = "tracing"))]
    {
        use tracing_subscriber::{fmt, prelude::*, EnvFilter};
        let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let subscriber = fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_thread_names(false);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(subscriber)
            .init();
    }
    Ok(())
}
