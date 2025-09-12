//! P2 runtimes (single-thread and concurrent) and graph traits.
//!
//! The **single-thread** runtime mirrors P0/P1 shapes (ease of switching).
//! The **concurrent** runtime requires a graph that exposes interior-mutability
//! stepping; this isolates threading concerns in the graph without adding
//! dynamic dispatch or unsafe in the core runtime.

pub mod p2_single;
pub mod p2_concurrent;
