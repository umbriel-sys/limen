#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![deny(unsafe_code)]
//! Limen e2e examples.

#[cfg(feature = "alloc")]
extern crate alloc;

/// Graphs generated at build time (via `build.rs`).
pub mod build_graphs {
    use limen_core::edge::Edge;
    use limen_core::node::source::Source;
    use limen_core::node::Node;

    /// Graph A:
    pub mod graph_a {
        use super::*;
        // Each file contains both flavors (non-std and cfg(std) concurrent_graph)
        include!(concat!(env!("OUT_DIR"), "/generated/build_graph_a.rs"));
    }
}
