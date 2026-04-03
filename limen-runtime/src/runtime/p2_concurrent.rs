//! Concurrent P2 runtime (`std` only).
//!
//! Drives a graph using `ScopedGraphApi`: each node gets a dedicated worker
//! thread within a `std::thread::scope`. Each worker calls its own
//! [`WorkerScheduler`](limen_core::scheduling::WorkerScheduler) to decide
//! whether to step, wait, or stop — no global lock, no `dyn` dispatch.
//!
//! > **Status: stub.** Implementation pending `RS1` runtime lifecycle work.
