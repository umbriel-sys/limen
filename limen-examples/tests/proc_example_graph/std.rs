//! `std`-path tests for the proc-macro-generated `ProcExampleConcurrentGraph`.
//! Exercises `ConcurrentEdge` and `ScopedGraphApi` with proc-macro-emitted code.
use super::ProcExampleConcurrentGraph;

use limen_core::edge::spsc_concurrent::ConcurrentEdge;
use limen_core::edge::EdgeOccupancy;
use limen_core::graph::GraphApi as _;
use limen_core::graph::GraphNodeAccess;
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
use limen_core::policy::{SlidingWindow, WindowKind};
use limen_core::prelude::concurrent::{spawn_telemetry_core, TelemetrySender};
use limen_core::prelude::graph_telemetry::GraphTelemetry;
use limen_core::prelude::linux::NoStdLinuxMonotonicClock;
use limen_core::prelude::sink::IoLineWriter;
use limen_core::prelude::ConcurrentMemoryManager;
use limen_core::prelude::TestTensor;
use limen_core::runtime::bench::concurrent_runtime::TestScopedRuntime;
use limen_core::runtime::LimenRuntime;
use limen_core::telemetry::Telemetry;
use limen_core::types::{QoSClass, SequenceNumber, Ticks, TraceId};

// Concrete queue type used by the test pipelines
type Q32 = ConcurrentEdge;

type Mgr32 = limen_core::memory::concurrent_manager::ConcurrentMemoryManager<TestTensor>;

const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeTensor<TEST_MAX_BATCH>;

type NoStdTestClock = NoStdLinuxMonotonicClock;

type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<std::io::Stdout>>;
type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;

type StdRuntime = TestScopedRuntime<NoStdTestClock, StdTestTelemetry, 3, 3>;

const INGRESS_POLICY: EdgePolicy = EdgePolicy::new(
    QueueCaps::new(32, 32, None, None),
    AdmissionPolicy::DropOldest,
    OverBudgetAction::Drop,
);

const LARGE_DELTA_T: Ticks = Ticks::new(1_000_000_000_000u64);

#[test]
fn proc_macro_std_pipeline_steps_with_std_runtime() {
    let node_policy = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );

    // clock
    let clock = NoStdLinuxMonotonicClock::new();

    // nodes
    let mut src = TestCounterSourceTensor::new(
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
    src.produce_n_items_in_backlog(16);

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

    let mgr0: Mgr32 = Mgr32::new(35);
    let mgr1: Mgr32 = Mgr32::new(35);

    // telemetry: GraphTelemetry wrapped in a concurrent TelemetrySender
    let sink = IoLineWriter::<std::io::Stdout>::stdout_writer();
    let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
    let telemetry_core = spawn_telemetry_core(inner_telemetry);
    let telemetry: StdTestTelemetry = telemetry_core.sender();

    // graph (proc-macro std / concurrent flavor)
    let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);

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
        LimenRuntime::<ProcExampleConcurrentGraph, 3, 3>::occupancies(&runtime)
    );

    for _ in 0..9 {
        let _ = runtime.step(&mut graph).unwrap();

        println!(
            "--- [graph_occupancies] --- {:?}",
            LimenRuntime::<ProcExampleConcurrentGraph, 3, 3>::occupancies(&runtime)
        );
    }

    // request stop and run one final step
    LimenRuntime::<ProcExampleConcurrentGraph, 3, 3>::request_stop(&mut runtime);
    let _ =
        LimenRuntime::<ProcExampleConcurrentGraph, 3, 3>::step(&mut runtime, &mut graph).unwrap();

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

#[cfg(feature = "std")]
#[test]
fn proc_macro_std_pipeline_runs_with_std_runtime() {
    use std::io::Stdout;

    type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<std::io::Stdout>>;
    type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;

    // Concrete queue type used by the test pipelines
    type StdQ32 = ConcurrentEdge;

    type StdGraph = ProcExampleConcurrentGraph;
    type StdRuntime = TestScopedRuntime<NoStdTestClock, StdTestTelemetry, 3, 3>;

    let node_policy = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );

    // clock
    let clock = NoStdLinuxMonotonicClock::new();

    // nodes
    let mut src = TestCounterSourceTensor::new(
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
    src.produce_n_items_in_backlog(16);

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
    let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
    let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

    // graph
    let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);

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

    // Get stop handle via trait method, clone before run() borrows &mut self.
    let handle = LimenRuntime::<StdGraph, 3, 3>::stop_handle(&runtime).unwrap();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        handle.request_stop();
    });

    // run() → run_scoped: spawns one scoped thread per node, runs until stop.
    LimenRuntime::<StdGraph, 3, 3>::run(&mut runtime, &mut graph).unwrap();
    graph.validate_graph().unwrap();

    println!(
        "--- [final_graph_occupancies] --- {:?}",
        LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime)
    );

    // Safely inspect telemetry, if present.
    let _ = runtime.with_telemetry(|telemetry| {
        // Push a metrics snapshot into the sink and flush.
        telemetry.push_metrics();
        telemetry.flush();
    });

    // Shut down the telemetry core and flush everything.
    telemetry_core.shutdown_and_join();
}

// =====================================================================
// Std batch tests (TestScopedRuntime, step + run, 32-item capacity)
// =====================================================================

#[cfg(feature = "std")]
const BATCH_EDGE_POLICY: EdgePolicy = EdgePolicy::new(
    QueueCaps::new(32, 32, None, None),
    AdmissionPolicy::DropOldest,
    OverBudgetAction::Drop,
);

#[cfg(feature = "std")]
#[test]
fn batch_std_disjoint_fixed_n() {
    use std::io::Stdout;

    type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<Stdout>>;
    type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;
    type StdQ = ConcurrentEdge;
    type StdGraph = ProcExampleConcurrentGraph;
    type StdRuntime = TestScopedRuntime<NoStdTestClock, StdTestTelemetry, 3, 3>;

    let node_policy_src = NodePolicy::new(
        BatchingPolicy::fixed(3),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_map = NodePolicy::new(
        BatchingPolicy::fixed(3),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_snk = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );

    let clock = NoStdLinuxMonotonicClock::new();

    // --- Step portion ---
    println!("\n -> Starting graph step test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        for _ in 0..10 {
            let _ = runtime.step(&mut graph).unwrap();

            println!(
                "--- [graph_occupancies] --- {:?}",
                LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime)
            );
        }

        let occ = LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime);
        assert_eq!(*occ[1].items(), 0, "step: edge1 should be 0");
        assert_eq!(*occ[2].items(), 20, "step: edge2 should be 20");

        // Print final telemetry snapshot for the step portion.
        let _ = runtime.with_telemetry(|telemetry| {
            telemetry.push_metrics();
            telemetry.flush();
        });

        LimenRuntime::<StdGraph, 3, 3>::request_stop(&mut runtime);
        telemetry_core.shutdown_and_join();
    }

    // --- Run portion ---
    println!("\n -> Starting graph run test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(64);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(64);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        let handle = LimenRuntime::<StdGraph, 3, 3>::stop_handle(&runtime).unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            handle.request_stop();
        });

        LimenRuntime::<StdGraph, 3, 3>::run(&mut runtime, &mut graph).unwrap();
        graph.validate_graph().unwrap();

        let source_node = <ProcExampleConcurrentGraph as GraphNodeAccess<2>>::node_mut(&mut graph)
            .node_mut()
            .sink_mut();
        let processed_messages = source_node.processed();
        assert!(*processed_messages > 0, "run: zero messages pushed by sink");

        // Print final telemetry snapshot for the run portion (ensures metrics
        // are pushed and the writer is flushed before the telemetry core stops).
        runtime
            .with_telemetry(|telemetry| {
                telemetry.push_metrics();
                telemetry.flush();
            })
            .expect("runtime.with_telemetry failed after run");

        telemetry_core.shutdown_and_join();
    }
}

#[cfg(feature = "std")]
#[test]
fn batch_std_disjoint_max_delta_t() {
    use std::io::Stdout;

    type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<Stdout>>;
    type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;
    type StdQ = ConcurrentEdge;
    type StdGraph = ProcExampleConcurrentGraph;
    type StdRuntime = TestScopedRuntime<NoStdTestClock, StdTestTelemetry, 3, 3>;

    let node_policy_src = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_map = NodePolicy::new(
        BatchingPolicy::delta_t(LARGE_DELTA_T),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_snk = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );

    let clock = NoStdLinuxMonotonicClock::new();

    // --- Step portion ---
    println!("\n -> Starting graph step test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        for _ in 0..10 {
            let _ = runtime.step(&mut graph).unwrap();

            println!(
                "--- [graph_occupancies] --- {:?}",
                LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime)
            );
        }

        let occ = LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime);
        // delta_t-only degenerates: single-item, all nodes keep up
        assert_eq!(*occ[1].items(), 0, "step: edge1 should be 0");
        assert_eq!(*occ[2].items(), 0, "step: edge2 should be 0");

        // Print final telemetry snapshot for the step portion.
        let _ = runtime.with_telemetry(|telemetry| {
            telemetry.push_metrics();
            telemetry.flush();
        });

        LimenRuntime::<StdGraph, 3, 3>::request_stop(&mut runtime);
        telemetry_core.shutdown_and_join();
    }

    // --- Run portion ---
    println!("\n -> Starting graph run test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        let handle = LimenRuntime::<StdGraph, 3, 3>::stop_handle(&runtime).unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            handle.request_stop();
        });

        LimenRuntime::<StdGraph, 3, 3>::run(&mut runtime, &mut graph).unwrap();
        graph.validate_graph().unwrap();

        let source_node = <ProcExampleConcurrentGraph as GraphNodeAccess<2>>::node_mut(&mut graph)
            .node_mut()
            .sink_mut();
        let processed_messages = source_node.processed();
        assert!(*processed_messages > 0, "run: zero messages pushed by sink");

        // Print final telemetry snapshot for the run portion (ensures metrics
        // are pushed and the writer is flushed before the telemetry core stops).
        runtime
            .with_telemetry(|telemetry| {
                telemetry.push_metrics();
                telemetry.flush();
            })
            .expect("runtime.with_telemetry failed after run");

        telemetry_core.shutdown_and_join();
    }
}

#[cfg(feature = "std")]
#[test]
fn batch_std_disjoint_fixed_n_and_max_delta_t() {
    use std::io::Stdout;

    type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<Stdout>>;
    type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;
    type StdQ = ConcurrentEdge;
    type StdGraph = ProcExampleConcurrentGraph;
    type StdRuntime = TestScopedRuntime<NoStdTestClock, StdTestTelemetry, 3, 3>;

    let node_policy_src = NodePolicy::new(
        BatchingPolicy::fixed(3),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_map = NodePolicy::new(
        BatchingPolicy::fixed_and_delta_t(3, LARGE_DELTA_T),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_snk = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );

    let clock = NoStdLinuxMonotonicClock::new();

    // --- Step portion ---
    println!("\n -> Starting graph step test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        for _ in 0..10 {
            let _ = runtime.step(&mut graph).unwrap();

            println!(
                "--- [graph_occupancies] --- {:?}",
                LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime)
            );
        }

        let occ = LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime);
        // Same as pure fixed(3): nmax=3, within LARGE delta_t
        assert_eq!(*occ[1].items(), 0, "step: edge1 should be 0");
        assert_eq!(*occ[2].items(), 20, "step: edge2 should be 20");

        // Print final telemetry snapshot for the step portion.
        let _ = runtime.with_telemetry(|telemetry| {
            telemetry.push_metrics();
            telemetry.flush();
        });

        LimenRuntime::<StdGraph, 3, 3>::request_stop(&mut runtime);
        telemetry_core.shutdown_and_join();
    }

    // --- Run portion ---
    println!("\n -> Starting graph run test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        let handle = LimenRuntime::<StdGraph, 3, 3>::stop_handle(&runtime).unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            handle.request_stop();
        });

        LimenRuntime::<StdGraph, 3, 3>::run(&mut runtime, &mut graph).unwrap();
        graph.validate_graph().unwrap();

        let source_node = <ProcExampleConcurrentGraph as GraphNodeAccess<2>>::node_mut(&mut graph)
            .node_mut()
            .sink_mut();
        let processed_messages = source_node.processed();
        assert!(*processed_messages > 0, "run: zero messages pushed by sink");

        // Print final telemetry snapshot for the run portion (ensures metrics
        // are pushed and the writer is flushed before the telemetry core stops).
        runtime
            .with_telemetry(|telemetry| {
                telemetry.push_metrics();
                telemetry.flush();
            })
            .expect("runtime.with_telemetry failed after run");

        telemetry_core.shutdown_and_join();
    }
}

#[cfg(feature = "std")]
#[test]
fn batch_std_sliding_fixed_n() {
    use std::io::Stdout;

    type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<Stdout>>;
    type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;
    type StdQ = ConcurrentEdge;
    type StdGraph = ProcExampleConcurrentGraph;
    type StdRuntime = TestScopedRuntime<NoStdTestClock, StdTestTelemetry, 3, 3>;

    let node_policy_src = NodePolicy::new(
        BatchingPolicy::fixed(2),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_map = NodePolicy::new(
        BatchingPolicy::fixed_with_window(3, WindowKind::Sliding(SlidingWindow::new(1))),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_snk = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );

    let clock = NoStdLinuxMonotonicClock::new();

    // --- Step portion ---
    println!("\n -> Starting graph step test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        for _ in 0..10 {
            let _ = runtime.step(&mut graph).unwrap();

            println!(
                "--- [graph_occupancies] --- {:?}",
                LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime)
            );
        }

        let occ = LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime);
        // Sliding stride=1: edge1 accumulates +1/step, edge2 accumulates +2/step (approx)
        assert_eq!(*occ[1].items(), 10, "step: edge1 should be 10");
        assert_eq!(*occ[2].items(), 19, "step: edge2 should be 19");

        // Print final telemetry snapshot for the step portion.
        let _ = runtime.with_telemetry(|telemetry| {
            telemetry.push_metrics();
            telemetry.flush();
        });

        LimenRuntime::<StdGraph, 3, 3>::request_stop(&mut runtime);
        telemetry_core.shutdown_and_join();
    }

    // --- Run portion ---
    println!("\n -> Starting graph run test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        let handle = LimenRuntime::<StdGraph, 3, 3>::stop_handle(&runtime).unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            handle.request_stop();
        });

        LimenRuntime::<StdGraph, 3, 3>::run(&mut runtime, &mut graph).unwrap();
        graph.validate_graph().unwrap();

        let source_node = <ProcExampleConcurrentGraph as GraphNodeAccess<2>>::node_mut(&mut graph)
            .node_mut()
            .sink_mut();
        let processed_messages = source_node.processed();
        assert!(*processed_messages > 0, "run: zero messages pushed by sink");

        // Print final telemetry snapshot for the run portion (ensures metrics
        // are pushed and the writer is flushed before the telemetry core stops).
        runtime
            .with_telemetry(|telemetry| {
                telemetry.push_metrics();
                telemetry.flush();
            })
            .expect("runtime.with_telemetry failed after run");

        telemetry_core.shutdown_and_join();
    }
}

#[cfg(feature = "std")]
#[test]
fn batch_std_sliding_max_delta_t() {
    use std::io::Stdout;

    type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<Stdout>>;
    type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;
    type StdQ = ConcurrentEdge;
    type StdGraph = ProcExampleConcurrentGraph;
    type StdRuntime = TestScopedRuntime<NoStdTestClock, StdTestTelemetry, 3, 3>;

    let node_policy_src = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_map = NodePolicy::new(
        BatchingPolicy::delta_t_with_window(
            LARGE_DELTA_T,
            WindowKind::Sliding(SlidingWindow::new(1)),
        ),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_snk = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );

    let clock = NoStdLinuxMonotonicClock::new();

    // --- Step portion ---
    println!("\n -> Starting graph step test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        for _ in 0..10 {
            let _ = runtime.step(&mut graph).unwrap();

            println!(
                "--- [graph_occupancies] --- {:?}",
                LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime)
            );
        }

        let occ = LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime);
        assert_eq!(*occ[1].items(), 0, "step: edge1 should be 0");
        assert_eq!(*occ[2].items(), 0, "step: edge2 should be 0");

        // Print final telemetry snapshot for the step portion.
        let _ = runtime.with_telemetry(|telemetry| {
            telemetry.push_metrics();
            telemetry.flush();
        });

        LimenRuntime::<StdGraph, 3, 3>::request_stop(&mut runtime);
        telemetry_core.shutdown_and_join();
    }

    // --- Run portion ---
    println!("\n -> Starting graph run test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        let handle = LimenRuntime::<StdGraph, 3, 3>::stop_handle(&runtime).unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            handle.request_stop();
        });

        LimenRuntime::<StdGraph, 3, 3>::run(&mut runtime, &mut graph).unwrap();
        graph.validate_graph().unwrap();

        let source_node = <ProcExampleConcurrentGraph as GraphNodeAccess<2>>::node_mut(&mut graph)
            .node_mut()
            .sink_mut();
        let processed_messages = source_node.processed();
        assert!(*processed_messages > 0, "run: zero messages pushed by sink");

        // Print final telemetry snapshot for the run portion (ensures metrics
        // are pushed and the writer is flushed before the telemetry core stops).
        runtime
            .with_telemetry(|telemetry| {
                telemetry.push_metrics();
                telemetry.flush();
            })
            .expect("runtime.with_telemetry failed after run");

        telemetry_core.shutdown_and_join();
    }
}

#[cfg(feature = "std")]
#[test]
fn batch_std_sliding_fixed_n_and_max_delta_t() {
    use std::io::Stdout;

    type StdTestTelemetryInner = GraphTelemetry<3, 3, IoLineWriter<Stdout>>;
    type StdTestTelemetry = TelemetrySender<StdTestTelemetryInner>;
    type StdQ = ConcurrentEdge;
    type StdGraph = ProcExampleConcurrentGraph;
    type StdRuntime = TestScopedRuntime<NoStdTestClock, StdTestTelemetry, 3, 3>;

    let node_policy_src = NodePolicy::new(
        BatchingPolicy::fixed(3),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_map = NodePolicy::new(
        BatchingPolicy::with_window(
            Some(3),
            Some(LARGE_DELTA_T),
            WindowKind::Sliding(SlidingWindow::new(2)),
        ),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );
    let node_policy_snk = NodePolicy::new(
        BatchingPolicy::none(),
        BudgetPolicy::new(None, None),
        DeadlinePolicy::new(false, None, None),
    );

    let clock = NoStdLinuxMonotonicClock::new();

    // --- Step portion ---
    println!("\n -> Starting graph step test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        for _ in 0..10 {
            let _ = runtime.step(&mut graph).unwrap();

            println!(
                "--- [graph_occupancies] --- {:?}",
                LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime)
            );
        }

        let occ = LimenRuntime::<StdGraph, 3, 3>::occupancies(&runtime);
        // stride=2: edge1 grows +1/step, edge2 grows +2/step
        assert_eq!(*occ[1].items(), 10, "step: edge1 should be 10");
        assert_eq!(*occ[2].items(), 20, "step: edge2 should be 20");

        // Print final telemetry snapshot for the step portion.
        let _ = runtime.with_telemetry(|telemetry| {
            telemetry.push_metrics();
            telemetry.flush();
        });

        LimenRuntime::<StdGraph, 3, 3>::request_stop(&mut runtime);
        telemetry_core.shutdown_and_join();
    }

    // --- Run portion ---
    println!("\n -> Starting graph run test:");
    {
        let mut src = TestCounterSourceTensor::new(
            clock,
            0,
            TraceId::new(0u64),
            SequenceNumber::new(0u64),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            node_policy_src,
            [PlacementAcceptance::default()],
            BATCH_EDGE_POLICY,
        );
        src.produce_n_items_in_backlog(16);

        let map = MapNode::new(
            TestTensorBackend,
            (),
            node_policy_map,
            NodeCapabilities::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        )
        .unwrap();

        let snk = TestSinkNodeTensor::new(
            NodeCapabilities::default(),
            node_policy_snk,
            [PlacementAcceptance::default()],
            |s: &str| println!("--- [***Sink Output***] --- {}", s),
        );

        let q0: StdQ = StdQ::new(32);
        let q1: StdQ = StdQ::new(32);
        let sink = IoLineWriter::<Stdout>::stdout_writer();
        let inner_telemetry: StdTestTelemetryInner = StdTestTelemetryInner::new(0, true, sink);
        let telemetry_core = spawn_telemetry_core(inner_telemetry);
        let telemetry: StdTestTelemetry = telemetry_core.sender();
        let mgr0 = ConcurrentMemoryManager::<TestTensor>::new(35);
        let mgr1 = ConcurrentMemoryManager::<TestTensor>::new(35);

        let mut graph = ProcExampleConcurrentGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
        let mut runtime: StdRuntime = StdRuntime::new();
        runtime.init(&mut graph, clock, telemetry).unwrap();

        let handle = LimenRuntime::<StdGraph, 3, 3>::stop_handle(&runtime).unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            handle.request_stop();
        });

        LimenRuntime::<StdGraph, 3, 3>::run(&mut runtime, &mut graph).unwrap();
        graph.validate_graph().unwrap();

        let source_node = <ProcExampleConcurrentGraph as GraphNodeAccess<2>>::node_mut(&mut graph)
            .node_mut()
            .sink_mut();
        let processed_messages = source_node.processed();
        assert!(*processed_messages > 0, "run: zero messages pushed by sink");

        // Print final telemetry snapshot for the run portion (ensures metrics
        // are pushed and the writer is flushed before the telemetry core stops).
        runtime
            .with_telemetry(|telemetry| {
                telemetry.push_metrics();
                telemetry.flush();
            })
            .expect("runtime.with_telemetry failed after run");

        telemetry_core.shutdown_and_join();
    }
}
