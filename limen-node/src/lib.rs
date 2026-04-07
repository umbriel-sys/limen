// Copyright © 2025–present Arlo Louis Byrne (idky137)
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0.
// See the LICENSE-APACHE file in the project root for license terms.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-node
//!
//! **Limen Node** will provide concrete node implementations built on the
//! contracts defined in `limen-core`. It is `no_std` by default and uses
//! feature gates to enable `alloc` and `std`-specific conveniences.
//!
//! > **Status: stub.** The node contract traits live in `limen-core::node`
//! > (source, sink, model, link). This crate is the intended home for
//! > reusable, ready-to-use node implementations once they are developed.
//! > Planned node families include source adapters (sensors, file readers),
//! > sink adapters (GPIO, MQTT, stdout), pre/post-processing operators,
//! > and routing nodes (fan-out, fan-in).
//!
//! ## Feature Flags
//!
//! | Flag | Effect |
//! |------|--------|
//! | *(default)* | `no_std`, no heap |
//! | `alloc` | enables `alloc`-backed node variants |
//! | `std` | implies `alloc`; enables `std`-backed node variants |

#[cfg(feature = "alloc")]
extern crate alloc;
