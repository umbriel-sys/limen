#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod pipeline;
pub mod builder;

pub use pipeline::LightPipeline;
pub use builder::LightPipelineBuilder;

#[cfg(feature = "no_alloc")]
pub mod no_alloc;
