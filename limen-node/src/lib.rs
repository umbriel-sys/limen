#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-node
//!
//! **Limen Node** provides node implementations based on the contracts
//! defined in `limen-core`. It is `no_std` by default and uses feature gates
//! to enable `alloc` and `std`-specific conveniences.
//!
//! ## Modules Overview
//! - [`source`]: source nodes.
//! - [`sink`]: sink nodes.
//! - [`routing`]: data routing nodes.
//! - [`processing`]: internal processing nodes.
//! - [`model`]: model nodes.
//!
//! ## Feature Flags
//! - `alloc`: enables optional APIs using `alloc` types.
//! - `std`: enables `std`-specific conveniences; implies `alloc`.

#[cfg(feature = "alloc")]
extern crate alloc;
