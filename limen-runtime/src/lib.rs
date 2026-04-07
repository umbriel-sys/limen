// Copyright © 2025–present Arlo Louis Byrne (idky137)
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0.
// See the LICENSE-APACHE file in the project root for license terms.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-runtime
//!
//! **Limen Runtime** provides concrete runtime and scheduler implementations
//! for executing Limen graphs. Runtimes implement the [`LimenRuntime`] trait
//! defined in `limen-core`; schedulers implement [`DequeuePolicy`] (sequential)
//! or [`WorkerScheduler`] (concurrent).
//!
//! > **Status: skeleton.** Module structure and scheduling algorithms are
//! > designed and partially implemented (see commented code). Full activation
//! > is tracked by the `RS1` runtime lifecycle and `Q1` test overhaul planned
//! > items. The `limen-examples` integration tests drive the currently active
//! > test runtimes (`TestNoStdRuntime`, `TestScopedRuntime`) defined in
//! > `limen-core::runtime::bench`.
//!
//! ## Modules
//!
//! - [`runtime`] — P2 single-thread and concurrent runtime implementations.
//! - [`scheduler`] — EDF and throughput `DequeuePolicy` implementations.
//!
//! ## Feature Flags
//!
//! | Flag | Effect |
//! |------|--------|
//! | *(default)* | `no_std`, no heap; single-thread runtime |
//! | `alloc` | enables `alloc`-backed runtime variants |
//! | `std` | implies `alloc`; enables `ScopedGraphApi`-based concurrent runtime |
//!
//! [`LimenRuntime`]: limen_core::runtime::LimenRuntime
//! [`DequeuePolicy`]: limen_core::scheduling::DequeuePolicy
//! [`WorkerScheduler`]: limen_core::scheduling::WorkerScheduler

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod runtime;
pub mod scheduler;
