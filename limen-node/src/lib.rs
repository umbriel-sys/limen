#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! Compute backends and model nodes for Limen.

#[cfg(feature = "alloc")]
extern crate alloc;
