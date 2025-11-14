use super::concurrent_graph::ProcExampleGraphStd;

use limen_core::edge::EdgeOccupancy;
use limen_core::graph::GraphApi as _;
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
use limen_core::runtime::bench::concurrent_runtime::TestStdRuntime;
use limen_core::runtime::LimenRuntime;
use limen_core::types::{QoSClass, SequenceNumber, Ticks, TraceId};

// Concrete queue type used by the test pipelines (matches bench graphs)
type Q32 = limen_core::edge::bench::TestSpscRingBuf<Message<u32>, 8>;

const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeU32_2<TEST_MAX_BATCH>;

#[test]
fn proc_macro_std_pipeline_runs_with_std_runtime() {
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
        NodePolicy {
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
        },
        NodeCapabilities::default(),
        [PlacementAcceptance::default()],
        [PlacementAcceptance::default()],
    )
    .unwrap();

    let snk = TestSinkNodeU32_2::new(
        NodeCapabilities::default(),
        NodePolicy {
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
        },
        [PlacementAcceptance::default()],
        |s: &str| println!("--- [***Sink Output***] --- {}", s),
    );

    // queues
    let q0: Q32 = Q32::default();
    let q1: Q32 = Q32::default();

    // graph (proc-macro std / concurrent flavor)
    let mut graph = ProcExampleGraphStd::new(src, map, snk, q0, q1);

    // runtime
    let mut runtime: TestStdRuntime<NoopClock, NoopTelemetry, 3, 3> = TestStdRuntime::new();

    // init (moves bundles to worker threads)
    runtime.init(&mut graph, NoopClock, NoopTelemetry).unwrap();

    // graph remains valid (descriptors intact)
    graph.validate_graph().unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();

        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestStdRuntime<NoopClock, NoopTelemetry, 3, 3> as LimenRuntime<
                ProcExampleGraphStd,
                3,
                3,
            >>::occupancies(&runtime)
        );
    }

    // request stop and run one final step to reattach bundles
    <TestStdRuntime<NoopClock, NoopTelemetry, 3, 3>
        as LimenRuntime<ProcExampleGraphStd, 3, 3>>::request_stop(&mut runtime);
    let _ = runtime.step(&mut graph).unwrap();

    // validate again (nodes reattached)
    graph.validate_graph().unwrap();

    // final snapshot
    {
        let mut occ: [EdgeOccupancy; 3] = [EdgeOccupancy {
            items: 0,
            bytes: 0,
            watermark: WatermarkState::AtOrAboveHard,
        }; 3];
        graph.write_all_edge_occupancies(&mut occ).unwrap();
    }
}
