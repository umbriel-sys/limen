//! Tests for Core runtime.

use crate::edge::EdgeOccupancy;
use crate::graph::GraphApi;
use crate::memory::PlacementAcceptance;
use crate::message::{Message, MessageFlags};
use crate::node::NodeCapabilities;
use crate::policy::{BatchingPolicy, BudgetPolicy, DeadlinePolicy, NodePolicy, WatermarkState};
use crate::runtime::LimenRuntime;
use crate::types::{QoSClass, SequenceNumber, Ticks, TraceId};

// Concrete queue type used by the test pipelines (matches your bench graphs)
type Q32 = crate::edge::bench::TestSpscRingBuf<Message<u32>, 8>;

// -------------------------------------------------------------
// core (no_std) pipeline + no_std test runtime (single-threaded)
// -------------------------------------------------------------
#[test]
fn core_pipeline_runs_with_nostd_runtime() {
    use crate::graph::bench::TestPipeline;
    use crate::node::bench::{TestIdentityModelNodeU32, TestSinkNodeU32, TestSourceNodeU32};
    use crate::runtime::bench::TestNoStdRuntime;

    // Put this right before you construct `snk` in each test:

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

    // nodes
    let src = TestSourceNodeU32::new(
        0,
        TraceId(0u64),
        SequenceNumber(0u64),
        Ticks(0u64),
        None,
        QoSClass::BestEffort,
        MessageFlags::empty(),
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
    );

    let map = TestIdentityModelNodeU32::new(
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
        [PlacementAcceptance::default()],
    );

    let snk = TestSinkNodeU32::new(
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
        printer,
    );

    // queues
    let q0: Q32 = Q32::default();
    let q1: Q32 = Q32::default();

    // graph
    let mut graph = TestPipeline::new(src, map, snk, q0, q1);

    // runtime
    let mut runtime: TestNoStdRuntime<3, 2> = TestNoStdRuntime::new();

    // init (no_std runtime doesn't move anything)
    runtime.init(&mut graph, (), ()).unwrap();

    // quick validation + snapshot
    graph.validate_graph().unwrap();
    let mut occ: [EdgeOccupancy; 2] = [EdgeOccupancy {
        items: 0,
        bytes: 0,
        watermark: WatermarkState::AtOrAboveHard,
    }; 2];
    graph.write_all_edge_occupancies(&mut occ).unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<3, 2> as crate::runtime::LimenRuntime<
                crate::graph::bench::TestPipeline,
                3,
                2,
            >>::occupancies(&runtime)
        );
    }

    // still valid
    graph.validate_graph().unwrap();
    assert!(!<TestNoStdRuntime<3, 2> as LimenRuntime<
        crate::graph::bench::TestPipeline,
        3,
        2,
    >>::is_stopping(&runtime));
}

// ----------------------------------------------------------------------
// std (concurrent) pipeline + std test runtime (one worker thread/node)
// ----------------------------------------------------------------------
#[cfg(feature = "std")]
#[test]
fn std_pipeline_runs_with_std_runtime() {
    use crate::graph::bench::concurrent_graph::TestPipelineStd;
    use crate::node::bench::{TestIdentityModelNodeU32, TestSinkNodeU32, TestSourceNodeU32};
    use crate::runtime::bench::concurrent_runtime::TestStdRuntime;

    // nodes
    let src = TestSourceNodeU32::new(
        0,
        TraceId(0u64),
        SequenceNumber(0u64),
        Ticks(0u64),
        None,
        QoSClass::BestEffort,
        MessageFlags::empty(),
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
    );

    let map = TestIdentityModelNodeU32::new(
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
        [PlacementAcceptance::default()],
    );

    let snk = TestSinkNodeU32::new(
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

    // graph
    let mut graph = TestPipelineStd::new(src, map, snk, q0, q1);

    // runtime
    let mut runtime: TestStdRuntime<3, 2> = TestStdRuntime::new();

    // init (moves bundles to worker threads)
    runtime.init(&mut graph, (), ()).unwrap();

    // graph remains valid (descriptors intact)
    graph.validate_graph().unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();

        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestStdRuntime<3, 2> as crate::runtime::LimenRuntime<
                crate::graph::bench::TestPipeline,
                3,
                2,
            >>::occupancies(&runtime)
        );
    }

    // request stop and run one final step to reattach bundles
    <crate::runtime::bench::concurrent_runtime::TestStdRuntime<3, 2> as LimenRuntime<
        crate::graph::bench::concurrent_graph::TestPipelineStd,
        3,
        2,
    >>::request_stop(&mut runtime);
    let _ = runtime.step(&mut graph).unwrap();

    // validate again (nodes reattached)
    graph.validate_graph().unwrap();

    // final snapshot
    {
        let mut occ: [EdgeOccupancy; 2] = [EdgeOccupancy {
            items: 0,
            bytes: 0,
            watermark: WatermarkState::AtOrAboveHard,
        }; 2];
        graph.write_all_edge_occupancies(&mut occ).unwrap();
    }
}
