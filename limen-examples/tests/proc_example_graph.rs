//! Integration tests for the proc-macro-generated simple example graph.

use limen_build::define_graph;
use limen_core::edge::Edge;
use limen_core::node::source::Source;
use limen_core::node::Node;
use limen_core::policy::{AdmissionPolicy, EdgePolicy, OverBudgetAction, QueueCaps};
use limen_core::prelude::TestTensor;

/// Example edge policy used by the proc-macro graph.
const EXAMPLE_EDGE_POLICY: EdgePolicy = EdgePolicy::new(
    QueueCaps::new(8, 6, None, None),
    AdmissionPolicy::DropNewest,
    OverBudgetAction::Drop,
);

define_graph! {
    pub struct ProcExampleNoStdGraph;

    nodes {
        0: {
            ty: limen_core::node::bench::TestCounterSourceTensor<limen_core::prelude::linux::NoStdLinuxMonotonicClock, 32>,
            in_ports: 0,
            out_ports: 1,
            in_payload: (),
            out_payload: TestTensor,
            name: Some("src"),
            ingress_policy: EXAMPLE_EDGE_POLICY
        },
        1: {
            ty: limen_core::node::bench::TestIdentityModelNodeTensor<32>,
            in_ports: 1,
            out_ports: 1,
            in_payload: TestTensor,
            out_payload: TestTensor,
            name: Some("map")
        },
        2: {
            ty: limen_core::node::bench::TestSinkNodeTensor,
            in_ports: 1,
            out_ports: 0,
            in_payload: TestTensor,
            out_payload: (),
            name: Some("sink")
        },
    }

    edges {
        0: {
            ty: limen_core::edge::bench::TestSpscRingBuf<8>,
            payload: TestTensor,
            manager: limen_core::memory::static_manager::StaticMemoryManager<TestTensor, 8>,
            from: (0, 0),
            to: (1, 0),
            policy: EXAMPLE_EDGE_POLICY,
            name: Some("src->map")
        },
        1: {
            ty: limen_core::edge::bench::TestSpscRingBuf<8>,
            payload: TestTensor,
            manager: limen_core::memory::static_manager::StaticMemoryManager<TestTensor, 8>,
            from: (1, 0),
            to: (2, 0),
            policy: EXAMPLE_EDGE_POLICY,
            name: Some("map->sink")
        },
    }
}

// Concurrent graph (std-only)
#[cfg(feature = "std")]
define_graph! {
    pub struct ProcExampleConcurrentGraph;

    nodes {
        0: {
            ty: limen_core::node::bench::TestCounterSourceTensor<limen_core::prelude::linux::NoStdLinuxMonotonicClock, 32>,
            in_ports: 0,
            out_ports: 1,
            in_payload: (),
            out_payload: TestTensor,
            name: Some("src"),
            ingress_policy: EXAMPLE_EDGE_POLICY
        },
        1: {
            ty: limen_core::node::bench::TestIdentityModelNodeTensor<32>,
            in_ports: 1,
            out_ports: 1,
            in_payload: TestTensor,
            out_payload: TestTensor,
            name: Some("map")
        },
        2: {
            ty: limen_core::node::bench::TestSinkNodeTensor,
            in_ports: 1,
            out_ports: 0,
            in_payload: TestTensor,
            out_payload: (),
            name: Some("sink")
        },
    }

    edges {
        0: {
            ty: limen_core::edge::spsc_concurrent::ConcurrentEdge,
            payload: TestTensor,
            manager: limen_core::memory::concurrent_manager::ConcurrentMemoryManager<TestTensor>,
            from: (0, 0),
            to: (1, 0),
            policy: EXAMPLE_EDGE_POLICY,
            name: Some("src->map")
        },
        1: {
            ty: limen_core::edge::spsc_concurrent::ConcurrentEdge,
            payload: TestTensor,
            manager: limen_core::memory::concurrent_manager::ConcurrentMemoryManager<TestTensor>,
            from: (1, 0),
            to: (2, 0),
            policy: EXAMPLE_EDGE_POLICY,
            name: Some("map->sink")
        },
    }

    concurrent;
}

// #[cfg(not(feature = "std"))]
#[path = "proc_example_graph/no_std.rs"]
mod no_std;

#[cfg(feature = "std")]
#[path = "proc_example_graph/std.rs"]
mod std;
