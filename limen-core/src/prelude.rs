//! Convenience re-exports for crate consumers and codegen output.
//!
//! Importing `limen_core::prelude::*` brings all public surface into scope,
//! gated by the same feature flags as the source modules:
//!
//! - (default / `no_std`) — [`Edge`], [`StaticRing`], [`StaticMemoryManager`],
//!   [`Tensor`], [`Batch`], [`Node`], [`GraphApi`],
//!   [`PlatformClock`], policy types, etc.
//! - `alloc` — `HeapMemoryManager`, `HeapRing`.
//! - `std` — `ConcurrentMemoryManager`, `ConcurrentEdge`, `ScopedEdge`,
//!   `ScopedGraphApi`, concurrent telemetry.
//! - `spsc_raw` — `SpscAtomicRing`.
//! - `bench` / `test` — test nodes, test edges, test graph, test runtime.

pub use crate::edge::{
    link::*, spsc_array::*, spsc_priority2, Edge, EdgeOccupancy, EnqueueResult, NoQueue,
};
pub use crate::errors::*;
pub use crate::graph::{validate::*, *};
pub use crate::memory::{header_store::*, manager::*, static_manager::*, *};
pub use crate::message::{batch::*, payload::*, tensor::*, *};
pub use crate::node::{
    link::*, model::*, sink::*, source::*, Node, NodeCapabilities, NodeKind, ProcessResult,
    StepContext, StepResult,
};
pub use crate::platform::{linux::*, *};
pub use crate::policy::*;
pub use crate::scheduling::*;
pub use crate::telemetry::{event_message::*, graph_telemetry::*, sink::*, *};
pub use crate::types::*;

#[cfg(feature = "alloc")]
pub use crate::memory::heap_manager::*;

#[cfg(feature = "std")]
pub use crate::memory::concurrent_manager::*;

#[cfg(feature = "std")]
pub use crate::telemetry::concurrent::*;

#[cfg(feature = "std")]
pub use crate::edge::spsc_concurrent::*;

#[cfg(feature = "std")]
pub use crate::edge::{EdgeHandleKind, ScopedEdge};

#[cfg(feature = "alloc")]
pub use crate::edge::spsc_vecdeque::*;

#[cfg(feature = "spsc_raw")]
pub use crate::edge::spsc_raw::*;

#[cfg(any(test, feature = "bench"))]
pub use crate::node::bench::*;

#[cfg(any(test, feature = "bench"))]
pub use crate::edge::bench::*;

#[cfg(any(test, feature = "bench"))]
pub use crate::graph::bench::*;

#[cfg(any(test, feature = "bench"))]
pub use crate::runtime::bench::*;
