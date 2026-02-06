//! Tests for Core runtime.

use crate::edge::EdgeOccupancy;
use crate::graph::bench::TestPipeline;
use crate::graph::GraphApi;
use crate::memory::PlacementAcceptance;
use crate::message::{Message, MessageFlags};
use crate::node::bench::{
    TestCounterSourceU32_2, TestIdentityModelNodeU32_2, TestSinkNodeU32_2, TestU32Backend,
};
use crate::node::NodeCapabilities;
use crate::policy::{BatchingPolicy, BudgetPolicy, DeadlinePolicy, NodePolicy, WatermarkState};
use crate::prelude::graph_telemetry::GraphTelemetry;
use crate::prelude::linux::NoStdLinuxMonotonicClock;
use crate::prelude::sink::{fixed_buffer_line_writer, FixedBuffer, FmtLineWriter};
use crate::runtime::bench::TestNoStdRuntime;
use crate::runtime::LimenRuntime;
use crate::types::{QoSClass, SequenceNumber, TraceId};

// Concrete queue type used by the test pipelines
type Q32 = crate::edge::bench::TestSpscRingBuf<Message<u32>, 8>;

const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeU32_2<TEST_MAX_BATCH>;

type NoStdTestTelemetry = GraphTelemetry<3, 3, FmtLineWriter<FixedBuffer<2048>>>;

type NoStdTestClock = NoStdLinuxMonotonicClock;

// -------------------------------------------------------------
// core (no_std) pipeline + no_std test runtime (single-threaded)
// -------------------------------------------------------------
#[test]
fn core_pipeline_runs_with_nostd_runtime() {
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

    // clock
    let clock = NoStdLinuxMonotonicClock::new();

    // nodes
    let src = TestCounterSourceU32_2::new(
        clock,
        0,
        TraceId::new(0u64),
        SequenceNumber::new(0u64),
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

    // telemetry
    let sink = fixed_buffer_line_writer::<2048>();
    let telemetry: NoStdTestTelemetry = NoStdTestTelemetry::new(0, true, sink);

    // graph
    let mut graph = TestPipeline::new(src, map, snk, q0, q1);

    // runtime
    let mut runtime: TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> =
        TestNoStdRuntime::new();

    // init (no_std runtime doesn't move anything)
    runtime.init(&mut graph, clock, telemetry).unwrap();

    // quick validation + snapshot
    graph.validate_graph().unwrap();
    let mut occ: [EdgeOccupancy; 3] = [EdgeOccupancy {
        items: 0,
        bytes: 0,
        watermark: WatermarkState::AtOrAboveHard,
    }; 3];
    graph.write_all_edge_occupancies(&mut occ).unwrap();

    #[cfg(feature = "std")]
    println!(
        "--- [initial_graph_occupancies] --- {:?}\n",
        <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as crate::runtime::LimenRuntime<
            crate::graph::bench::TestPipeline<NoStdTestClock>,
            3,
            3,
        >>::occupancies(&runtime)
    );

    for _ in 0..9 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as crate::runtime::LimenRuntime<
                crate::graph::bench::TestPipeline<NoStdTestClock>,
                3,
                3,
            >>::occupancies(&runtime)
        );
    }

    // still valid
    graph.validate_graph().unwrap();
    assert!(
        !<TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as LimenRuntime<
            crate::graph::bench::TestPipeline<NoStdTestClock>,
            3,
            3,
        >>::is_stopping(&runtime)
    );

    <TestNoStdRuntime<
        NoStdTestClock,
        GraphTelemetry<3, 3, FmtLineWriter<FixedBuffer<2048>>>,
        3,
        3,
    > as LimenRuntime<TestPipeline<NoStdTestClock>, 3, 3>>::request_stop(&mut runtime);

    // Safely inspect telemetry, if present.
    #[cfg(feature = "std")]
    {
        let _ = runtime.with_telemetry(|telemetry| {
            // Push a metrics snapshot into the sink and flush.

            use crate::prelude::Telemetry as _;
            telemetry.push_metrics();
            telemetry.flush();

            // Access the fixed buffer and print it.
            let sink_ref = telemetry.writer();
            let buffer_ref = sink_ref.inner();

            println!("\n--- [telemetry buffer] ---\n{}", buffer_ref.as_str());
        });
    }
}

// ----------------------------------------------------------------------
// std (concurrent) pipeline + std test runtime (one worker thread/node)
// ----------------------------------------------------------------------
#[cfg(feature = "std")]
#[test]
fn std_pipeline_runs_with_std_runtime() {
    use crate::{
        graph::bench::concurrent_graph::TestPipelineStd,
        prelude::{concurrent::spawn_telemetry_core, sink::IoLineWriter},
        runtime::bench::concurrent_runtime::TestStdRuntime,
        telemetry::concurrent::TelemetrySender,
    };

    type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<std::io::Stdout>>;
    type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;

    type StdGraph = TestPipelineStd<NoStdTestClock>;
    type StdRuntime = TestStdRuntime<StdGraph, NoStdTestClock, StdTestTelemetry, 3, 3>;

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

    // clock
    let clock = NoStdLinuxMonotonicClock::new();

    // nodes
    let src = TestCounterSourceU32_2::new(
        clock,
        0,
        TraceId::new(0u64),
        SequenceNumber::new(0u64),
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

    // telemetry: GraphTelemetry wrapped in a concurrent TelemetrySender
    let sink = IoLineWriter::<std::io::Stdout>::stdout_writer();
    let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
    let telemetry_core = spawn_telemetry_core(inner_telemetry);
    let telemetry: StdTestTelemetry = telemetry_core.sender();

    // graph
    let mut graph = TestPipelineStd::new(src, map, snk, q0, q1);

    // runtime
    let mut runtime: StdRuntime = StdRuntime::new();

    // init (moves bundles to worker threads)
    runtime.init(&mut graph, clock, telemetry).unwrap();

    // graph remains valid (descriptors intact)
    graph.validate_graph().unwrap();
    let mut occ: [EdgeOccupancy; 3] = [EdgeOccupancy {
        items: 0,
        bytes: 0,
        watermark: WatermarkState::AtOrAboveHard,
    }; 3];
    graph.write_all_edge_occupancies(&mut occ).unwrap();
    println!(
        "--- [initial_graph_occupancies] --- {:?}\n",
        runtime.occupancies()
    );

    for _ in 0..9 {
        let _ = runtime.step(&mut graph).unwrap();

        println!("--- [graph_occupancies] --- {:?}", runtime.occupancies());
    }

    // request stop and run one final step to reattach bundles
    LimenRuntime::<StdGraph, 3, 3>::request_stop(&mut runtime);
    let _ = LimenRuntime::<StdGraph, 3, 3>::step(&mut runtime, &mut graph).unwrap();

    // validate again (nodes reattached)
    graph.validate_graph().unwrap();

    // Safely inspect telemetry, if present.
    let _ = runtime.with_telemetry(|telemetry| {
        // Push a metrics snapshot into the sink and flush.
        use crate::prelude::Telemetry as _;
        telemetry.push_metrics();
        telemetry.flush();
    });

    // Shut down the telemetry core and flush everything.
    telemetry_core.shutdown_and_join();
}
