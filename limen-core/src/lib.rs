#![cfg_attr(not(feature = "std"), no_std)]
//! limen-core: foundational traits, types, errors, and the cooperative runtime.

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod errors;
pub mod types;
pub mod traits;
pub mod runtime;
