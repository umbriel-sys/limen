#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-light
//!
//! `limen-light` provides **P0** (no_std + no_alloc) and **P1** (no_std + alloc)
//! runtimes and queue implementations on top of [`limen-core`]. It keeps the data
//! path fully generic and monomorphized; there is **no dynamic dispatch** in the
//! hot path.
//!
//! ## What is here
//! - [`spsc`]: single-producer/single-consumer queues for P0 and P1.
//! - [`runtime::p0`]: deterministic cooperative loop for P0.
//! - [`runtime::p1`]: cooperative micro-batching loop for P1.
////! - [`graph`]: traits for integrating your statically-wired graph and helpers
//!   for simple linear pipelines (sources → preprocess → model → postprocess → sink).
//! - [`util`]: small helpers like an empty payload for 0-port nodes.
//!
//! ## What is **not** here
//! Graph construction for arbitrary DAGs is intentionally kept out of `limen-core`
//! and implemented by applications or code generators. This crate offers traits that
//! a typed graph can implement to be runnable by the P0/P1 runtimes. For convenience,
//! [`graph::simple_chain`] demonstrates how to wire a common 5-stage pipeline.
//!
//! ## Features
//! - `p0` (default): enable P0 runtime and static ring queues.
//! - `p1`: enable P1 runtime and heap-backed queues (requires `alloc`).
//! - `std`: enables `std` conveniences; implies `alloc`.
//!
//! ## Safety
//! The provided queues are SPSC and designed for **single-threaded cooperative**
//! execution. They use interior mutability appropriate for this model. Do not use
//! them across threads without additional synchronization.

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod graph;
pub mod runtime;
pub mod spsc;
pub mod util;

pub use limen_core as core;
pub use limen_core::prelude as core_prelude;
