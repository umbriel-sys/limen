//! Tests for Core runtime.

use crate::edge::EdgeOccupancy;
use crate::graph::bench::TestPipeline;
use crate::graph::GraphApi;
use crate::memory::PlacementAcceptance;
use crate::message::MessageFlags;
use crate::node::bench::{
    TestCounterSourceTensor, TestIdentityModelNodeTensor, TestSinkNodeTensor, TestTensorBackend,
};
use crate::node::NodeCapabilities;
use crate::policy::{
    BatchingPolicy, BudgetPolicy, DeadlinePolicy, EdgePolicy, NodePolicy, WatermarkState,
};
use crate::prelude::graph_telemetry::GraphTelemetry;
use crate::prelude::linux::NoStdLinuxMonotonicClock;
use crate::prelude::sink::{fixed_buffer_line_writer, FixedBuffer, FmtLineWriter};
use crate::prelude::TestTensor;
use crate::runtime::bench::TestNoStdRuntime;
use crate::runtime::LimenRuntime;
use crate::types::{QoSClass, SequenceNumber, TraceId};

// Concrete queue type used by the test pipelines
type Q32 = crate::edge::bench::TestSpscRingBuf<8>;

const INGRESS_POLICY: EdgePolicy = EdgePolicy {
    caps: crate::policy::QueueCaps {
        max_items: 8,
        soft_items: 8,
        max_bytes: None,
        soft_bytes: None,
    },
    over_budget: crate::policy::OverBudgetAction::Drop,
    admission: crate::policy::AdmissionPolicy::DropOldest,
};

const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeTensor<TEST_MAX_BATCH>;

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

    let node_policy = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
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
        node_policy,
        NodeCapabilities::default(),
        [PlacementAcceptance::default()],
        [PlacementAcceptance::default()],
    )
    .unwrap();

    let snk = TestSinkNodeTensor::new(
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

    // managers
    let mgr0 = crate::memory::static_manager::StaticMemoryManager::<TestTensor, 8>::new();
    let mgr1 = crate::memory::static_manager::StaticMemoryManager::<TestTensor, 8>::new();

    // graph
    let mut graph = TestPipeline::new(src, map, snk, q0, q1, mgr0, mgr1);

    // runtime
    let mut runtime: TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> =
        TestNoStdRuntime::new();

    // init (no_std runtime doesn't move anything)
    runtime.init(&mut graph, clock, telemetry).unwrap();

    // quick validation + snapshot
    graph.validate_graph().unwrap();
    let mut occ: [EdgeOccupancy; 3] = [EdgeOccupancy::new(0, 0, WatermarkState::AtOrAboveHard); 3];
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
    use std::io::Stdout;

    use crate::{
        graph::bench::concurrent_graph::TestPipelineStd,
        prelude::{concurrent::spawn_telemetry_core, sink::IoLineWriter, ConcurrentEdge},
        runtime::bench::concurrent_runtime::TestScopedRuntime,
        telemetry::concurrent::TelemetrySender,
    };

    type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<std::io::Stdout>>;
    type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;

    // Concrete queue type used by the test pipelines
    type StdQ32 = ConcurrentEdge;

    type StdGraph = TestPipelineStd<NoStdTestClock>;
    type StdRuntime = TestScopedRuntime<NoStdTestClock, StdTestTelemetry, 3, 3>;

    let node_policy = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
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
    let q0: StdQ32 = StdQ32::new(32);
    let q1: StdQ32 = StdQ32::new(32);

    // telemetry: GraphTelemetry wrapped in a concurrent TelemetrySender
    let sink = IoLineWriter::<Stdout>::stdout_writer();
    let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
    let telemetry_core = spawn_telemetry_core(inner_telemetry);
    let telemetry: StdTestTelemetry = telemetry_core.sender();

    // managers
    let mgr0 = crate::memory::concurrent_manager::ConcurrentMemoryManager::<TestTensor>::new(8);
    let mgr1 = crate::memory::concurrent_manager::ConcurrentMemoryManager::<TestTensor>::new(8);

    // graph
    let mut graph = TestPipelineStd::new(src, map, snk, q0, q1, mgr0, mgr1);

    // runtime
    let mut runtime: StdRuntime = StdRuntime::new();

    // init (moves bundles to worker threads)
    runtime.init(&mut graph, clock, telemetry).unwrap();

    // graph remains valid (descriptors intact)
    graph.validate_graph().unwrap();
    let mut occ: [EdgeOccupancy; 3] = [EdgeOccupancy::new(0, 0, WatermarkState::AtOrAboveHard); 3];
    graph.write_all_edge_occupancies(&mut occ).unwrap();
    println!(
        "--- [initial_graph_occupancies] --- {:?}\n",
        <TestScopedRuntime<
            NoStdLinuxMonotonicClock,
            TelemetrySender<GraphTelemetry<3, 3, IoLineWriter<Stdout>>>,
            3,
            3,
        > as LimenRuntime<StdGraph, 3, 3>>::occupancies(&runtime)
    );

    for _ in 0..9 {
        let _ = runtime.step(&mut graph).unwrap();

        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestScopedRuntime<
                NoStdLinuxMonotonicClock,
                TelemetrySender<GraphTelemetry<3, 3, IoLineWriter<Stdout>>>,
                3,
                3,
            > as LimenRuntime<StdGraph, 3, 3>>::occupancies(&runtime)
        );
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
