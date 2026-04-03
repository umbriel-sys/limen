//! Single-thread P2 runtime.
//!
//! Drives a graph by calling `step` on one node per tick, selected by a
//! pluggable [`DequeuePolicy`](limen_core::scheduling::DequeuePolicy).
//! The policy (EDF or throughput) is a generic parameter — no `dyn` dispatch.
//!
//! > **Status: stub.** Implementation pending `RS1` runtime lifecycle work.
