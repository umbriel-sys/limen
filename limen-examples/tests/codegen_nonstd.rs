//! Non-std tests for build-script (codegen) graph.

mod common;

use limen_core::edge::EdgeOccupancy;
use limen_core::graph::GraphApi;
use limen_core::policy::WatermarkState;
use limen_core::prelude::{NoopClock, NoopTelemetry};
use limen_core::runtime::bench::TestNoStdRuntime;
use limen_core::runtime::LimenRuntime;

use common::{make_nodes, Q};

#[test]
fn codegen_graph_nonstd_runs() {
    use limen_examples::build_graphs::graph_a::BuildGraphA;

    let (src, map, snk) = make_nodes();
    let q0: Q = Default::default();
    let q1: Q = Default::default();

    let mut graph = BuildGraphA::new(src, map, snk, q0, q1);

    let mut runtime: TestNoStdRuntime<NoopClock, NoopTelemetry, 3, 3> = TestNoStdRuntime::new();
    runtime.init(&mut graph, NoopClock, NoopTelemetry).unwrap();

    graph.validate_graph().unwrap();

    // 1 ingress + 2 real edges
    let mut occupancies: [EdgeOccupancy; 3] = [EdgeOccupancy {
        items: 0,
        bytes: 0,
        watermark: WatermarkState::AtOrAboveHard,
    }; 3];
    graph.write_all_edge_occupancies(&mut occupancies).unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();
    }

    graph.validate_graph().unwrap();
}
