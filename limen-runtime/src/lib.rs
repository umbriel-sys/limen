#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-runtime
//!
//! **Limen Runtime** provides runtime implementations used to run limen graphs.
//! It is `no_std` by default and uses feature gates to enable `alloc` and
//! `std`-specific conveniences.
//!
//! ## Modules Overview
//! - [`todo`]: still todo.
//! - [`todo`]: still todo.
//! - [`todo`]: still todo.
//!
//! ## Feature Flags
//! - `alloc`: enables optional APIs using `alloc` types.
//! - `std`: enables `std`-specific conveniences; implies `alloc`.

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod runtime;
pub mod scheduler;
