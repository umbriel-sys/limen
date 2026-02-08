//! Integration tests for the proc-macro-generated simple example graph.

use limen_build::define_graph;
use limen_core::edge::Edge;
use limen_core::node::source::Source;
use limen_core::node::Node;
use limen_core::policy::{AdmissionPolicy, EdgePolicy, OverBudgetAction, QueueCaps};

/// Example edge policy used by the proc-macro graph.
const EXAMPLE_EDGE_POLICY: EdgePolicy = EdgePolicy::new(
    QueueCaps::new(8, 6, None, None),
    AdmissionPolicy::DropNewest,
    OverBudgetAction::Drop,
);

// The DSL is identical in shape to the codegen example graph; the only
// difference is that we invoke it via the `define_graph!` proc macro rather
// than writing it to $OUT_DIR and including it.
define_graph! {
    pub struct ProcExampleGraph;

    nodes {
        0: {
            ty: limen_core::node::bench::TestCounterSourceU32_2<limen_core::prelude::linux::NoStdLinuxMonotonicClock>,
            in_ports: 0,
            out_ports: 1,
            in_payload: (),
            out_payload: u32,
            name: Some("src"),
            ingress_policy: EXAMPLE_EDGE_POLICY
        },
        1: {
            ty: limen_core::node::bench::TestIdentityModelNodeU32_2<32>,
            in_ports: 1,
            out_ports: 1,
            in_payload: u32,
            out_payload: u32,
            name: Some("map")
        },
        2: {
            ty: limen_core::node::bench::TestSinkNodeU32_2,
            in_ports: 1,
            out_ports: 0,
            in_payload: u32,
            out_payload: (),
            name: Some("sink")
        },
    }

    edges {
        0: {
            ty: limen_core::edge::bench::TestSpscRingBuf<
                limen_core::message::Message<u32>, 8
            >,
            payload: u32,
            from: (0, 0),
            to: (1, 0),
            policy: EXAMPLE_EDGE_POLICY,
            name: Some("src->map")
        },
        1: {
            ty: limen_core::edge::bench::TestSpscRingBuf<
                limen_core::message::Message<u32>, 8
            >,
            payload: u32,
            from: (1, 0),
            to: (2, 0),
            policy: EXAMPLE_EDGE_POLICY,
            name: Some("map->sink")
        },
    }
}

// #[cfg(not(feature = "std"))]
#[path = "proc_example_graph/no_std.rs"]
mod no_std;

#[cfg(feature = "std")]
#[path = "proc_example_graph/std.rs"]
mod std;
