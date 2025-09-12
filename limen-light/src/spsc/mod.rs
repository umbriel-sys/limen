//! Single-producer/single-consumer queue implementations for P0 and P1.
//!
//! The data-path trait comes from `limen-core` as [`SpscQueue`](limen_core::queue::SpscQueue).
//! P0 uses a **static ring** (`StaticRing`) with const generics, while P1 uses
//! a **heap ring** (`HeapRing`) with bounded capacity in items (and tracked bytes).
//!
//! Both implementations support item and byte watermarks and implement basic
//! admission logic, including `DropOldest` on pressure.

#[cfg(feature = "alloc")]
pub mod heap_ring;
pub mod static_ring;
