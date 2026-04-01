use limen_core::edge::spsc_concurrent::ConcurrentEdge;
use limen_core::edge::EdgeOccupancy;
use limen_core::graph::bench::concurrent_graph::TestPipelineStd;
use limen_core::graph::GraphApi;
use limen_core::memory::PlacementAcceptance;
use limen_core::message::MessageFlags;
use limen_core::node::bench::{
    TestCounterSourceTensor, TestIdentityModelNodeTensor, TestSinkNodeTensor, TestTensorBackend,
};
use limen_core::node::NodeCapabilities;
use limen_core::policy::{
    AdmissionPolicy, BatchingPolicy, BudgetPolicy, DeadlinePolicy, EdgePolicy, NodePolicy,
    OverBudgetAction, QueueCaps, WatermarkState,
};
use limen_core::prelude::concurrent::{spawn_telemetry_core, TelemetrySender};
use limen_core::prelude::graph_telemetry::GraphTelemetry;
use limen_core::prelude::linux::NoStdLinuxMonotonicClock;
use limen_core::prelude::sink::IoLineWriter;
use limen_core::prelude::TestTensor;
use limen_core::runtime::bench::concurrent_runtime::TestScopedRuntime;
use limen_core::runtime::LimenRuntime;
use limen_core::types::{QoSClass, SequenceNumber, TraceId};

// Concrete queue type used by the test pipelines
type Q32 = ConcurrentEdge;

type Mgr32 = limen_core::memory::concurrent_manager::ConcurrentMemoryManager<TestTensor>;

const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeTensor<TEST_MAX_BATCH>;

type NoStdTestClock = NoStdLinuxMonotonicClock;

type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<std::io::Stdout>>;
type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;

type StdGraph = TestPipelineStd<NoStdTestClock>;
type StdRuntime = TestScopedRuntime<NoStdTestClock, StdTestTelemetry, 3, 3>;

// ----------------------------------------------------------------------
// std (concurrent) pipeline + std test runtime (one worker thread/node)
// ----------------------------------------------------------------------
#[test]
fn std_pipeline_runs_with_std_runtime() {
    let node_policy = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );

    const INGRESS_POLICY: EdgePolicy = EdgePolicy::new(
        QueueCaps::new(8, 8, None, None),
        AdmissionPolicy::DropOldest,
        OverBudgetAction::Drop,
    );

    // clock
    let clock = NoStdLinuxMonotonicClock::new();

    // nodes
    let src = TestCounterSourceTensor::new(
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
        INGRESS_POLICY,
    );

    let map = MapNode::new(
        TestTensorBackend,
        (),
        NodePolicy::new(
            BatchingPolicy::none(),
            BudgetPolicy::new(None, None),
            DeadlinePolicy::new(false, None, None),
        ),
        NodeCapabilities::default(),
        [PlacementAcceptance::default()],
        [PlacementAcceptance::default()],
    )
    .unwrap();

    let snk = TestSinkNodeTensor::new(
        NodeCapabilities::default(),
        NodePolicy::new(
            BatchingPolicy::none(),
            BudgetPolicy::new(None, None),
            DeadlinePolicy::new(false, None, None),
        ),
        [PlacementAcceptance::default()],
        |s: &str| println!("--- [***Sink Output***] --- {}", s),
    );

    // queues
    let q0: Q32 = Q32::new(32);
    let q1: Q32 = Q32::new(32);

    let mgr0: Mgr32 = Mgr32::new(8);
    let mgr1: Mgr32 = Mgr32::new(8);

    // telemetry: GraphTelemetry wrapped in a concurrent TelemetrySender
    let sink = IoLineWriter::<std::io::Stdout>::stdout_writer();
    let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
    let telemetry_core = spawn_telemetry_core(inner_telemetry);
    let telemetry: StdTestTelemetry = telemetry_core.sender();

    // graph
    let mut graph = TestPipelineStd::new(src, map, snk, q0, q1, mgr0, mgr1);

    // runtime
    let mut runtime: StdRuntime = StdRuntime::new();

    // init
    runtime.init(&mut graph, clock, telemetry).unwrap();

    // graph remains valid (descriptors intact)
    graph.validate_graph().unwrap();
    let mut occ: [EdgeOccupancy; 3] = [EdgeOccupancy::new(0, 0, WatermarkState::AtOrAboveHard); 3];
    graph.write_all_edge_occupancies(&mut occ).unwrap();
    println!(
        "--- [initial_graph_occupancies] --- {:?}\n",
        LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime)
    );

    for _ in 0..9 {
        let _ = runtime.step(&mut graph).unwrap();

        println!(
            "--- [graph_occupancies] --- {:?}",
            LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime)
        );
    }

    // request stop and run one final step
    LimenRuntime::<StdGraph, 3, 3>::request_stop(&mut runtime);
    let _ = LimenRuntime::<StdGraph, 3, 3>::step(&mut runtime, &mut graph).unwrap();

    // validate again
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
