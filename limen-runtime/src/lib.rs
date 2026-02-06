#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen (P2 runtime)
//!
//! This crate implements the **P2** runtime for Limen on top of
//! [`limen-core`](https://crates.io/crates/limen-core), providing:
//!
//! - **Schedulers**: latency-first (**EDF**) and throughput-oriented policies.
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
//! - [`scheduler`]: EDF and throughput dequeue policies implementing the core trait.
//! - [`runtime`]: single-thread event loop (`RuntimeP2`) and concurrent variant

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod runtime;
pub mod scheduler;
