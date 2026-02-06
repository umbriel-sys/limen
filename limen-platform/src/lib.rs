#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-platform
//!
//! **Limen platform** provides platform adapters for limen. It is `no_std`
//! by default and uses feature gates to enable `alloc` and `std`-specific
//! conveniences.
//!
//! ## Modules Overview
//! - [`linux`]: basic linux platform adapters.
//!
//! ## Feature Flags
//! - `alloc`: enables optional APIs using `alloc` types.
//! - `std`: enables `std`-specific conveniences; implies `alloc`.

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod linux;
