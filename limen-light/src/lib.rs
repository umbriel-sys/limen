#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod builder;
pub mod pipeline;

#[cfg(feature = "alloc")]
pub use builder::LightPipelineBuilder;
#[cfg(feature = "alloc")]
pub use pipeline::LightPipeline;

#[cfg(feature = "no_alloc")]
pub mod no_alloc;
