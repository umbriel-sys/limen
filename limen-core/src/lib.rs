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

// **PR NOTES**:
// - step context must have scratch buffer = nodes n_max - All nodes should expose an n_max (or should be configurable), this should probably be separate to node policy fixed_n, fixed_n should be the desired batch size and n_max the capacity, these should bpth be configurabe. (only required for stride enabled batches or concurrent graphs.)
// - fix feature boundary (edges are confused)(default, alloc (untilises alloc), std (enables multithreading))
// - can we remove concurrentedgelink and use a concurrent friendly memory manager instead? (maybe not..)
// - single interface for nodes to use.

// Phase 3d: Eviction design note
//
//  The current Evict(n) path can evict multiple tokens but EnqueueResult::Evicted only returns one. For v0.1.0, this is acceptable because:
//  - Evict(n) with n>1 is rare (only EvictUntilBelowHard evicts multiple)
//  - StepContext needs to free ALL evicted tokens, not just the last one
//
//  Better approach: StepContext should handle eviction pre-push (call get_admission_decision, pop evicted tokens itself, free them, then push). The edge's try_push would only handle Admit/DropNewest/Reject/Block. Eviction
//   logic moves to StepContext.
//
//  For now, the implementation above returns the last evicted token. If multiple evictions are needed, StepContext can pre-check admission and handle eviction externally before calling try_push. This is a minor TODO for
//  Phase 5 (StepContext).
//
// REMOVE ADMISSION INFO!!!
