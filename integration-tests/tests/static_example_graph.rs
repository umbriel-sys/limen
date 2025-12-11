//! Integration tests for the static simple example graph, implementd in limen-core.

// #[cfg(not(feature = "std"))]
#[path = "static_example_graph/no_std.rs"]
mod no_std;

#[cfg(feature = "std")]
#[path = "static_example_graph/std.rs"]
mod std;
