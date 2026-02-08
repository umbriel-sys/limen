#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-core
//!
//! **Limen Core** defines the *stable contracts and primitives* for the Limen
//! graph-driven, edge inference runtime. It is `no_std` by default and uses
//! feature gates to enable `alloc` and `std`-specific conveniences. The data
//! plane is designed for *monomorphization* via generics and const generics,
//! avoiding dynamic dispatch in the hot path.
//!
//! This crate intentionally **does not** provide graph construction, concrete
//! schedulers, or queue implementations. Those are provided by `limen-light`
//! (P0/P1) and `limen` (P2).
//!
//! ## Modules Overview
//! - [`types`]: small newtypes and shared enums (QoS, identifiers).
//! - [`memory`]: memory classes and placement descriptors for zero-copy paths.
//! - [`message`]: message header, payload contract, and message types.
//! - [`policy`]: batching, budgets, deadlines, admission and edge policies.
//! - [`edge`]: single-producer single-consumer queue trait and results.
//! - [`node`]: uniform node contract and step lifecycle.
//! - [`routing`]: split (fan-out) and join (fan-in) operator traits.
//! - [`telemetry`]: counters, histograms, and tracing interfaces.
//! - [`platform`]: platform abstractions (clock, timers, affinities).
//! - [`scheduling`]: readiness and dequeue policy traits (EDF hooks).
//! - [`graph`]: port indices, edge descriptors, invariant validation traits.
//! - [`errors`]: error families for nodes, queues, and runtime surfaces.
//! - [`prelude`]: convenient re-exports for implementers.
//!
//! ## Feature Flags
//! - `alloc`: enables optional APIs using `alloc` types.
//! - `std`: enables `std`-specific conveniences; implies `alloc`.
//!
//! ## Versioning and Stability
//! The contracts defined here are intended to be *stable* so higher-level
//! runtimes can evolve independently. Avoid adding trait objects or dynamic
//! allocation requirements to keep the core maximally portable.

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
