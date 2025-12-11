use limen_core::edge::EdgeOccupancy;
use limen_core::graph::bench::concurrent_graph::TestPipelineStd;
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
use limen_core::prelude::concurrent::{spawn_telemetry_core, TelemetrySender};
use limen_core::prelude::graph_telemetry::GraphTelemetry;
use limen_core::prelude::linux::NoStdLinuxMonotonicClock;
use limen_core::prelude::sink::IoLineWriter;
use limen_core::runtime::bench::concurrent_runtime::TestStdRuntime;
use limen_core::runtime::LimenRuntime;
use limen_core::types::{QoSClass, SequenceNumber, TraceId};

// Concrete queue type used by the test pipelines (matches your bench graphs)
type Q32 = limen_core::edge::bench::TestSpscRingBuf<Message<u32>, 8>;

const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeU32_2<TEST_MAX_BATCH>;

type NoStdTestClock = NoStdLinuxMonotonicClock;

type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<std::io::Stdout>>;
type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;

type StdGraph = TestPipelineStd<NoStdTestClock>;
type StdRuntime = TestStdRuntime<StdGraph, NoStdTestClock, StdTestTelemetry, 3, 3>;

// ----------------------------------------------------------------------
// std (concurrent) pipeline + std test runtime (one worker thread/node)
// ----------------------------------------------------------------------
#[test]
fn std_pipeline_runs_with_std_runtime() {
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

        use limen_core::prelude::Telemetry as _;
        telemetry.push_metrics();
        telemetry.flush();
    });

    // Shut down the telemetry core and flush everything.
    telemetry_core.shutdown_and_join();
}
