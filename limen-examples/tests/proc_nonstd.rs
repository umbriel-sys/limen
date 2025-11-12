//! Non-std tests for proc-macro (limen-build) graph.

mod common;

use limen_build::define_graph;
use limen_core::graph::GraphApi;
use limen_core::prelude::{NoopClock, NoopTelemetry};
use limen_core::runtime::bench::TestNoStdRuntime;

use common::{example_edge_policy, make_nodes, Q};

// The DSL accepts general expressions, so we can call the helper directly.
define_graph! {
    pub struct ProcGraphA;

    nodes {
        0: {
            ty: limen_core::node::bench::TestCounterSourceU32_2,
            in_ports: 0,
            out_ports: 1,
            in_payload: (),
            out_payload: u32,
            name: Some("src"),
            ingress_policy: example_edge_policy()
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
            ty: limen_core::edge::spsc_ringbuf::SpscRingbuf<limen_core::message::Message<u32>>,
            payload: u32,
            from: (0, 0),
            to: (1, 0),
            policy: example_edge_policy(),
            name: Some("src->map")
        },
        1: {
            ty: limen_core::edge::spsc_ringbuf::SpscRingbuf<limen_core::message::Message<u32>>,
            payload: u32,
            from: (1, 0),
            to: (2, 0),
            policy: example_edge_policy(),
            name: Some("map->sink")
        },
    }
}

#[test]
fn proc_macro_graph_nonstd_runs() {
    let (src, map, snk) = make_nodes();
    let q0: Q = Default::default();
    let q1: Q = Default::default();

    let mut graph = ProcGraphA::new(src, map, snk, q0, q1);

    let mut runtime: TestNoStdRuntime<NoopClock, NoopTelemetry, 3, 3> = TestNoStdRuntime::new();
    runtime.init(&mut graph, NoopClock, NoopTelemetry).unwrap();

    graph.validate_graph().unwrap();
    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();
    }
    graph.validate_graph().unwrap();
}
