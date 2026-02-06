//! Integration tests for the codegen-generated simple example graph.

mod generated {
    use limen_core::edge::Edge;
    use limen_core::node::source::Source;
    use limen_core::node::Node;

    // Pulled from build.rs: $OUT_DIR/generated/simple_example_graph.rs
    include!(concat!(
        env!("OUT_DIR"),
        "/generated/simple_example_graph.rs"
    ));
}

// #[cfg(not(feature = "std"))]
#[path = "codegen_example_graph/no_std.rs"]
mod no_std;

#[cfg(feature = "std")]
#[path = "codegen_example_graph/std.rs"]
mod std;
