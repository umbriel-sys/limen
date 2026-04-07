// Copyright © 2025–present Arlo Louis Byrne (idky137)
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0.
// See the LICENSE-APACHE file in the project root for license terms.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-platform
//!
//! **Limen Platform** provides concrete implementations of the platform
//! abstractions defined in `limen-core::platform` ([`PlatformClock`],
//! [`Timers`], [`Affinity`]).
//!
//! > **Status: stub.** The platform contract traits are defined and stable in
//! > `limen-core`. This crate is the intended home for target-specific
//! > adapters. The `linux` module skeleton exists but is not yet implemented.
//! > The `P1` planned item tracks full `PlatformBackend` finalisation.
//!
//! ## Modules
//!
//! - [`linux`] — Linux / desktop platform adapters (stub; see module docs).
//!
//! ## Feature Flags
//!
//! | Flag | Effect |
//! |------|--------|
//! | *(default)* | `no_std`, no heap |
//! | `alloc` | enables `alloc`-backed adapters |
//! | `std` | implies `alloc`; enables `std::time`-backed clock and OS primitives |
//!
//! [`PlatformClock`]: limen_core::platform::PlatformClock
//! [`Timers`]: limen_core::platform::Timers
//! [`Affinity`]: limen_core::platform::Affinity

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod linux;
