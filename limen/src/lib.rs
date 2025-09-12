#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen (P2 runtime)
//!
//! This crate implements the **P2** runtime for Limen on top of
//! [`limen-core`](https://crates.io/crates/limen-core), providing:
//!
//! - **Schedulers**: latency-first (**EDF**) and throughput-oriented policies.
//! - **Queues**: lock-free single-producer/single-consumer rings (SPSC) and
//!   optional **priority lanes** (feature: `priority_lanes`).
//! - **Platform adapters**: std-based clock and stubs for timers/affinity.
//! - **Runtime**: a single-thread event loop with the same *graph-facing
//!   interface* shape as P0/P1, plus an **optional concurrent runtime** for
//!   graphs that provide interior-mutability based stepping.
//!
//! ## Design Notes
//! - The hot path remains **free of dynamic dispatch**. All queues and the
//!   scheduler policy are **generic** and **monomorphized**.
//! - Worker pools require interior mutability inside the graph; we provide an
//!   optional concurrent runtime that operates via a `GraphP2Concurrent` trait.
//!   This keeps the data plane generic while enabling concurrency where safe.
//!
//! ## Modules
//! - [`spsc`]: lock-free SPSC ring and optional priority wrapper.
//! - [`scheduler`]: EDF and throughput dequeue policies implementing the core trait.
//! - [`runtime`]: single-thread event loop (`RuntimeP2`) and concurrent variant
//!   (`RuntimeP2Concurrent`) along with the `GraphP2` / `GraphP2Concurrent` traits.
//! - [`platform`]: std clock (`StdClock`) and affinity/timer stubs.
//! - [`graph`]: optional helpers, including a typed **SimpleChain5** for parity
//!   with `limen-light` examples (purely optional).
//!
//! ## Switching Between Runtimes
//! The graph-facing traits mirror the P0/P1 shapes so the same **typed graph** can
//! implement `GraphP0` (limen-light), `GraphP1` (limen-light), and `GraphP2` (this
//! crate). This allows selecting the runtime per target without changing node code.

pub mod graph;
pub mod platform;
pub mod runtime;
pub mod scheduler;
pub mod spsc;

pub use limen_core as core;
pub use limen_core::prelude as core_prelude;
