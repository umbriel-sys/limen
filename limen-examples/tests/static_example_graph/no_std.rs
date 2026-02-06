use limen_core::edge::EdgeOccupancy;
use limen_core::graph::bench::TestPipeline;
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
use limen_core::prelude::graph_telemetry::GraphTelemetry;
use limen_core::prelude::linux::NoStdLinuxMonotonicClock;
use limen_core::prelude::sink::{fixed_buffer_line_writer, FixedBuffer, FmtLineWriter};
use limen_core::runtime::bench::TestNoStdRuntime;
use limen_core::runtime::LimenRuntime;
use limen_core::types::{QoSClass, SequenceNumber, TraceId};

// Concrete queue type used by the test pipelines (matches your bench graphs)
type Q32 = limen_core::edge::bench::TestSpscRingBuf<Message<u32>, 8>;

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
        TraceId(0u64),
        SequenceNumber(0u64),
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

    // init (no_std runtime does not move anything)
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
        <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as limen_core::runtime::LimenRuntime<
            limen_core::graph::bench::TestPipeline<NoStdTestClock>,
            3,
            3,
        >>::occupancies(&runtime)
    );

    for _ in 0..9 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as limen_core::runtime::LimenRuntime<
                limen_core::graph::bench::TestPipeline<NoStdTestClock>,
                3,
                3,
            >>::occupancies(&runtime)
        );
    }

    // still valid
    graph.validate_graph().unwrap();
    assert!(
        !<TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as LimenRuntime<
            limen_core::graph::bench::TestPipeline<NoStdTestClock>,
            3,
            3,
        >>::is_stopping(&runtime)
    );

    // Safely inspect telemetry, if present.
    #[cfg(feature = "std")]
    {
        let _ = runtime.with_telemetry(|telemetry| {
            // Push a metrics snapshot into the sink and flush.

            use limen_core::prelude::Telemetry as _;
            telemetry.push_metrics();
            telemetry.flush();

            // Access the fixed buffer and print it.
            let sink_ref = telemetry.writer();
            let buffer_ref = sink_ref.inner();

            println!("\n--- [telemetry buffer] ---\n{}", buffer_ref.as_str());
        });
    }
}
