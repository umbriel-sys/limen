//! P2 runtime implementations.
//!
//! - [`p2_single`] — single-thread runtime; drives a graph with a pluggable
//!   [`DequeuePolicy`](limen_core::scheduling::DequeuePolicy) (EDF or throughput).
//! - [`p2_concurrent`] — concurrent runtime; uses `ScopedGraphApi` and
//!   per-worker [`WorkerScheduler`](limen_core::scheduling::WorkerScheduler)
//!   instances to step nodes in parallel scoped threads.
//!
//! > **Status: skeleton.** Implementations are pending `RS1` / `Q1` work.

pub mod p2_concurrent;
pub mod p2_single;
