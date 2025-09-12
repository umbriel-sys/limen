//! Lock-free single-producer/single-consumer queues for P2.
//!
//! Two variants are provided:
//! - [`LockFreeRing`]: a generic SPSC ring using atomics, safe for a single
//!   producer thread and a single consumer thread.
//! - [`Priority2`]: (feature `priority_lanes`) a two-lane wrapper composing
//!   two queues with priority-aware `try_pop` semantics.

pub mod lockfree_ring;
#[cfg(feature = "priority_lanes")]
pub mod priority2;
