use super::ProcExampleGraph;

use limen_core::edge::EdgeOccupancy;
use limen_core::graph::GraphApi;
use limen_core::memory::PlacementAcceptance;
use limen_core::message::{Message, MessageFlags};
use limen_core::node::bench::{
    TestCounterSourceU32_2, TestIdentityModelNodeU32_2, TestSinkNodeU32_2, TestU32Backend,
};
use limen_core::node::NodeCapabilities;
use limen_core::policy::{
    BatchingPolicy, BudgetPolicy, DeadlinePolicy, NodePolicy, WatermarkState,
};
use limen_core::prelude::{NoopClock, NoopTelemetry};
use limen_core::runtime::bench::TestNoStdRuntime;
use limen_core::runtime::LimenRuntime;
use limen_core::types::{QoSClass, SequenceNumber, Ticks, TraceId};

// Concrete queue type used by the test pipelines (matches bench graphs)
type Q32 = limen_core::edge::bench::TestSpscRingBuf<Message<u32>, 8>;

const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeU32_2<TEST_MAX_BATCH>;

#[test]
fn proc_macro_core_pipeline_runs_with_nostd_runtime() {
    let printer: fn(&str) = {
        #[cfg(feature = "std")]
        {
            fn print_fn(s: &str) {
                println!("--- [***Sink Output***] --- {}", s);
            }
            print_fn
        }
        #[cfg(not(feature = "std"))]
        {
            fn noop(_: &str) {}
            noop
        }
    };

    let node_policy = NodePolicy {
        batching: BatchingPolicy {
            fixed_n: None,
            max_delta_t: None,
        },
        budget: BudgetPolicy {
            tick_budget: None,
            watchdog_ticks: None,
        },
        deadline: DeadlinePolicy {
            require_absolute_deadline: false,
            slack_tolerance_ns: None,
            default_deadline_ns: None,
        },
    };

    // nodes
    let src = TestCounterSourceU32_2::new(
        0,
        TraceId(0u64),
        SequenceNumber(0u64),
        Ticks(0u64),
        None,
        QoSClass::BestEffort,
        MessageFlags::empty(),
        NodeCapabilities::default(),
        node_policy,
        [PlacementAcceptance::default()],
    );

    let map = MapNode::new(
        TestU32Backend,
        (),
        node_policy,
        NodeCapabilities::default(),
        [PlacementAcceptance::default()],
        [PlacementAcceptance::default()],
    )
    .unwrap();

    let snk = TestSinkNodeU32_2::new(
        NodeCapabilities::default(),
        node_policy,
        [PlacementAcceptance::default()],
        printer,
    );

    // queues
    let q0: Q32 = Q32::default();
    let q1: Q32 = Q32::default();

    // graph (proc-macro non-std flavor)
    let mut graph = ProcExampleGraph::new(src, map, snk, q0, q1);

    // runtime
    let mut runtime: TestNoStdRuntime<NoopClock, NoopTelemetry, 3, 3> = TestNoStdRuntime::new();

    // init (no_std runtime does not move anything)
    runtime.init(&mut graph, NoopClock, NoopTelemetry).unwrap();

    // quick validation + snapshot
    graph.validate_graph().unwrap();
    let mut occ: [EdgeOccupancy; 3] = [EdgeOccupancy {
        items: 0,
        bytes: 0,
        watermark: WatermarkState::AtOrAboveHard,
    }; 3];
    graph.write_all_edge_occupancies(&mut occ).unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<NoopClock, NoopTelemetry, 3, 3>
                as limen_core::runtime::LimenRuntime<ProcExampleGraph, 3, 3>>::occupancies(
                &runtime
            )
        );
    }

    // still valid
    graph.validate_graph().unwrap();
    assert!(
        !<TestNoStdRuntime<NoopClock, NoopTelemetry, 3, 3> as LimenRuntime<
            ProcExampleGraph,
            3,
            3,
        >>::is_stopping(&runtime)
    );
}
