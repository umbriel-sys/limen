//! Scheduler policies for P2.
//!
//! Implements the `DequeuePolicy` trait from `limen-core` for:
//! - **EDF** (latency-first): picks the ready node with the earliest deadline.
//! - **Throughput**: favors nodes that are ready and not backpressured; falls
//!   back to under-pressure nodes; ties are broken by simple round-robin.

pub mod edf;
pub mod throughput;
