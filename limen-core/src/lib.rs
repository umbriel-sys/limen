// Copyright © 2025–present Arlo Louis Byrne (idky137)
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0.
// See the LICENSE-APACHE file in the project root for license terms.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-core
//!
//! **Limen Core** defines the *stable contracts and primitives* for the Limen
//! graph-driven, edge inference runtime targeting embedded and resource-constrained
//! systems.
//!
//! ## Design principles
//!
//! - **`no_std` by default.** All code compiles without `std`. Heap use is gated
//!   behind `#[cfg(feature = "alloc")]`; concurrent primitives behind `std`.
//! - **No dynamic dispatch in hot paths.** Node, edge, memory manager, and
//!   scheduler types are monomorphized via generics and const generics.
//! - **Token-based message passing.** Edges carry [`types::MessageToken`] handles
//!   rather than full messages. Message data (header + payload) lives in a
//!   [`memory::manager::MemoryManager`], addressed by token. This enables
//!   zero-copy routing and heterogeneous memory classes (host, pinned, device).
//! - **Stable contracts.** The traits defined here are the versioned boundary
//!   between application code and the runtime. Higher-level crates
//!   (`limen-runtime`, `limen-codegen`, `limen-node`) depend on this crate and
//!   can evolve independently without breaking the contract surface.
//!
//! ## Module overview
//!
//! | Module | What it provides |
//! |--------|-----------------|
//! | [`types`] | Newtypes for IDs, timing, QoS, `MessageToken`, `DataType`/`DType`, `F16`/`BF16` |
//! | [`errors`] | Error families for queues, nodes, inference, graph, runtime |
//! | [`memory`] | `MemoryClass`, `PlacementAcceptance`, `BufferDescriptor`; memory manager traits and impls |
//! | [`message`] | `MessageHeader`, `MessageFlags`, `Message<P>`; `Payload` trait; `Tensor`; `Batch` |
//! | [`policy`] | Batching, budget, deadline, admission, and per-edge/node policy types |
//! | [`compute`] | `ComputeBackend` / `ComputeModel` traits for dyn-free inference backends |
//! | [`edge`] | `Edge` SPSC trait; `EnqueueResult`, `EdgeOccupancy`; queue implementations |
//! | [`node`] | `Node` trait; `StepContext`, `StepResult`, `ProcessResult`; source/sink/model sub-traits |
//! | [`graph`] | `GraphApi`, `ScopedGraphApi`; compile-time node/edge access traits; descriptor validation |
//! | [`scheduling`] | `Readiness`, `NodeSummary`, `DequeuePolicy`; `WorkerScheduler` for concurrent execution |
//! | [`telemetry`] | `Telemetry` trait; `TelemetryEvent`; `NodeMetrics`, `EdgeMetrics`, `GraphMetrics` |
//! | [`platform`] | `PlatformClock`, `Span`, `Timers`, `Affinity`; `NoopClock` |
//! | [`runtime`] | `LimenRuntime` trait; `RuntimeStopHandle` |
//! | [`prelude`] | Convenience re-exports for all of the above, feature-gated |
//!
//! ## Feature flags
//!
//! | Flag | Effect |
//! |------|--------|
//! | *(default)* | `no_std`, no heap; fixed-size SPSC queues (`SpscArrayQueue`) |
//! | `alloc` | `HeapMemoryManager`, `SpscVecDeque`, owned `BatchView` |
//! | `std` | implies `alloc`; `ConcurrentMemoryManager`, `ConcurrentEdge`, `ScopedEdge`, `ScopedGraphApi`, concurrent telemetry |
//! | `spsc_raw` | unsafe lock-free ring buffer (`SpscRawQueue`); requires `std` |
//! | `bench` | exposes test nodes, edges, graphs, and runtimes for integration tests |
//! | `checked-memory-manager-refs` | adds per-slot borrow-state tracking to `StaticMemoryManager` and `HeapMemoryManager` |

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod errors;
pub mod memory;
pub mod types;

pub mod message;
pub mod platform;
pub mod policy;

pub mod compute;
pub mod scheduling;
pub mod telemetry;

pub mod edge;
pub mod graph;
pub mod node;
pub mod runtime;

pub mod prelude;
