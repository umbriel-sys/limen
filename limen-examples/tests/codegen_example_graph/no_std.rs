//! `no_std`-path tests for the build-script-generated `SimpleExampleNoStdGraph`.
use super::generated::SimpleExampleNoStdGraph;

use limen_core::edge::EdgeOccupancy;
use limen_core::graph::GraphApi;
use limen_core::memory::PlacementAcceptance;
use limen_core::message::MessageFlags;
use limen_core::node::bench::{
    TestCounterSourceTensor, TestIdentityModelNodeTensor, TestSinkNodeTensor, TestTensorBackend,
};
use limen_core::node::NodeCapabilities;
use limen_core::policy::{
    AdmissionPolicy, BatchingPolicy, BudgetPolicy, DeadlinePolicy, EdgePolicy, NodePolicy,
    OverBudgetAction, QueueCaps, SlidingWindow, WatermarkState, WindowKind,
};
use limen_core::prelude::graph_telemetry::GraphTelemetry;
use limen_core::prelude::linux::NoStdLinuxMonotonicClock;
use limen_core::prelude::sink::{fixed_buffer_line_writer, FixedBuffer, FmtLineWriter};
use limen_core::prelude::{StaticMemoryManager, TestTensor};
use limen_core::runtime::bench::TestNoStdRuntime;
use limen_core::runtime::LimenRuntime;
use limen_core::types::{QoSClass, SequenceNumber, Ticks, TraceId};

// Concrete queue type used by the test pipelines
type Q32 = limen_core::edge::bench::TestSpscRingBuf<32>;

type Mgr32 = limen_core::memory::static_manager::StaticMemoryManager<TestTensor, 35>;

const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeTensor<TEST_MAX_BATCH>;

type NoStdTestTelemetry = GraphTelemetry<3, 3, FmtLineWriter<FixedBuffer<2048>>>;

type NoStdTestClock = NoStdLinuxMonotonicClock;

const INGRESS_POLICY: EdgePolicy = EdgePolicy::new(
    QueueCaps::new(32, 32, None, None),
    AdmissionPolicy::DropOldest,
    OverBudgetAction::Drop,
);

const LARGE_DELTA_T: Ticks = Ticks::new(1_000_000_000_000u64);

#[test]
fn codegen_core_pipeline_runs_with_nostd_runtime() {
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

    let mgr0: Mgr32 = Mgr32::default();
    let mgr1: Mgr32 = Mgr32::default();

    // telemetry
    let sink = fixed_buffer_line_writer::<2048>();
    let telemetry: NoStdTestTelemetry = NoStdTestTelemetry::new(0, true, sink);

    // graph (codegen non-std flavor)
    let mut graph = SimpleExampleNoStdGraph::new(src, map, snk, q0, q1, mgr0, mgr1);

    // runtime
    let mut runtime: TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> =
        TestNoStdRuntime::new();

    // init (no_std runtime does not move anything)
    runtime.init(&mut graph, clock, telemetry).unwrap();

    // quick validation + snapshot
    graph.validate_graph().unwrap();
    let mut occ: [EdgeOccupancy; 3] = [EdgeOccupancy::new(0, 0, WatermarkState::AtOrAboveHard); 3];
    graph.write_all_edge_occupancies(&mut occ).unwrap();

    #[cfg(feature = "std")]
    println!(
        "--- [initial_graph_occupancies] --- {:?}\n",
        <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as limen_core::runtime::LimenRuntime<
            SimpleExampleNoStdGraph,
            3,
            3,
        >>::occupancies(&runtime)
    );

    for _ in 0..9 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3>
                as limen_core::runtime::LimenRuntime<SimpleExampleNoStdGraph, 3, 3>>::occupancies(
                &runtime
            )
        );
    }

    // still valid
    graph.validate_graph().unwrap();
    assert!(
        !<TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as LimenRuntime<
            SimpleExampleNoStdGraph,
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

// =====================================================================
// No-std batch step tests (TestNoStdRuntime, round-robin, 10 steps)
// =====================================================================

#[test]
fn batch_nostd_disjoint_fixed_n() {
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
        INGRESS_POLICY,
    );
    src.produce_n_items_in_backlog(16);

    let map = TestIdentityModelNodeTensor::<TEST_MAX_BATCH>::new(
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
        printer,
    );

    let q0: Q32 = Q32::default();
    let q1: Q32 = Q32::default();
    let sink = fixed_buffer_line_writer::<2048>();
    let telemetry: NoStdTestTelemetry = NoStdTestTelemetry::new(0, true, sink);
    let mgr0 = StaticMemoryManager::<TestTensor, 35>::new();
    let mgr1 = StaticMemoryManager::<TestTensor, 35>::new();

    let mut graph = SimpleExampleNoStdGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
    let mut runtime: TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> =
        TestNoStdRuntime::new();
    runtime.init(&mut graph, clock, telemetry).unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as LimenRuntime<
                SimpleExampleNoStdGraph,
                3,
                3,
            >>::occupancies(&runtime)
        );
    }

    let occ = LimenRuntime::<SimpleExampleNoStdGraph, 3, 3>::occupancies(&runtime);
    // edge index 0 is ingress (unused), 1 is src->map, 2 is map->snk
    assert_eq!(*occ[1].items(), 3, "edge1 (src->map) should have 3 items");
    assert_eq!(*occ[2].items(), 6, "edge2 (map->snk) should have 6 items");

    // Safely inspect telemetry, if present (prints to stdout under `std`).
    #[cfg(feature = "std")]
    {
        let _ = runtime.with_telemetry(|telemetry| {
            // Push a metrics snapshot and flush the writer.

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

#[test]
fn batch_nostd_disjoint_max_delta_t() {
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
        INGRESS_POLICY,
    );
    src.produce_n_items_in_backlog(16);

    let map = TestIdentityModelNodeTensor::<TEST_MAX_BATCH>::new(
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
        printer,
    );

    let q0: Q32 = Q32::default();
    let q1: Q32 = Q32::default();
    let sink = fixed_buffer_line_writer::<2048>();
    let telemetry: NoStdTestTelemetry = NoStdTestTelemetry::new(0, true, sink);
    let mgr0 = StaticMemoryManager::<TestTensor, 35>::new();
    let mgr1 = StaticMemoryManager::<TestTensor, 35>::new();

    let mut graph = SimpleExampleNoStdGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
    let mut runtime: TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> =
        TestNoStdRuntime::new();
    runtime.init(&mut graph, clock, telemetry).unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as LimenRuntime<
                SimpleExampleNoStdGraph,
                3,
                3,
            >>::occupancies(&runtime)
        );
    }

    let occ = LimenRuntime::<SimpleExampleNoStdGraph, 3, 3>::occupancies(&runtime);
    // delta_t-only degenerates to single-item for model (nmax=1 fallthrough)
    assert_eq!(*occ[1].items(), 1, "edge1 should have 1 item");
    assert_eq!(*occ[2].items(), 0, "edge2 should have 0 items");

    // Safely inspect telemetry, if present (prints to stdout under `std`).
    #[cfg(feature = "std")]
    {
        let _ = runtime.with_telemetry(|telemetry| {
            // Push a metrics snapshot and flush the writer.

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

#[test]
fn batch_nostd_disjoint_fixed_n_and_max_delta_t() {
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
        INGRESS_POLICY,
    );
    src.produce_n_items_in_backlog(16);

    let map = TestIdentityModelNodeTensor::<TEST_MAX_BATCH>::new(
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
        printer,
    );

    let q0: Q32 = Q32::default();
    let q1: Q32 = Q32::default();
    let sink = fixed_buffer_line_writer::<2048>();
    let telemetry: NoStdTestTelemetry = NoStdTestTelemetry::new(0, true, sink);
    let mgr0 = StaticMemoryManager::<TestTensor, 35>::new();
    let mgr1 = StaticMemoryManager::<TestTensor, 35>::new();

    let mut graph = SimpleExampleNoStdGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
    let mut runtime: TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> =
        TestNoStdRuntime::new();
    runtime.init(&mut graph, clock, telemetry).unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as LimenRuntime<
                SimpleExampleNoStdGraph,
                3,
                3,
            >>::occupancies(&runtime)
        );
    }

    let occ = LimenRuntime::<SimpleExampleNoStdGraph, 3, 3>::occupancies(&runtime);
    assert_eq!(*occ[1].items(), 3, "edge1 (src->map) should have 3 items");
    assert_eq!(*occ[2].items(), 6, "edge2 (map->snk) should have 6 items");

    // Safely inspect telemetry, if present (prints to stdout under `std`).
    #[cfg(feature = "std")]
    {
        let _ = runtime.with_telemetry(|telemetry| {
            // Push a metrics snapshot and flush the writer.

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

#[test]
fn batch_nostd_sliding_fixed_n() {
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
        INGRESS_POLICY,
    );
    src.produce_n_items_in_backlog(16);

    let map = TestIdentityModelNodeTensor::<TEST_MAX_BATCH>::new(
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
        printer,
    );

    let q0: Q32 = Q32::default();
    let q1: Q32 = Q32::default();
    let sink = fixed_buffer_line_writer::<2048>();
    let telemetry: NoStdTestTelemetry = NoStdTestTelemetry::new(0, true, sink);
    let mgr0 = StaticMemoryManager::<TestTensor, 35>::new();
    let mgr1 = StaticMemoryManager::<TestTensor, 35>::new();

    let mut graph = SimpleExampleNoStdGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
    let mut runtime: TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> =
        TestNoStdRuntime::new();
    runtime.init(&mut graph, clock, telemetry).unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as LimenRuntime<
                SimpleExampleNoStdGraph,
                3,
                3,
            >>::occupancies(&runtime)
        );
    }

    let occ = LimenRuntime::<SimpleExampleNoStdGraph, 3, 3>::occupancies(&runtime);
    // Sliding stride=1: edge1 accumulates, edge2 grows from window pushes
    assert_eq!(*occ[1].items(), 5, "edge1 (src->map) should have 5 items");
    assert_eq!(*occ[2].items(), 5, "edge2 (map->snk) should have 5 items");

    // Safely inspect telemetry, if present (prints to stdout under `std`).
    #[cfg(feature = "std")]
    {
        let _ = runtime.with_telemetry(|telemetry| {
            // Push a metrics snapshot and flush the writer.

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

#[test]
fn batch_nostd_sliding_max_delta_t() {
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
        INGRESS_POLICY,
    );
    src.produce_n_items_in_backlog(16);

    let map = TestIdentityModelNodeTensor::<TEST_MAX_BATCH>::new(
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
        printer,
    );

    let q0: Q32 = Q32::default();
    let q1: Q32 = Q32::default();
    let sink = fixed_buffer_line_writer::<2048>();
    let telemetry: NoStdTestTelemetry = NoStdTestTelemetry::new(0, true, sink);
    let mgr0 = StaticMemoryManager::<TestTensor, 35>::new();
    let mgr1 = StaticMemoryManager::<TestTensor, 35>::new();

    let mut graph = SimpleExampleNoStdGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
    let mut runtime: TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> =
        TestNoStdRuntime::new();
    runtime.init(&mut graph, clock, telemetry).unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as LimenRuntime<
                SimpleExampleNoStdGraph,
                3,
                3,
            >>::occupancies(&runtime)
        );
    }

    let occ = LimenRuntime::<SimpleExampleNoStdGraph, 3, 3>::occupancies(&runtime);
    // delta_t-only with sliding still degenerates for model node (nmax=1)
    assert_eq!(*occ[1].items(), 1, "edge1 should have 1 item");
    assert_eq!(*occ[2].items(), 0, "edge2 should have 0 items");

    // Safely inspect telemetry, if present (prints to stdout under `std`).
    #[cfg(feature = "std")]
    {
        let _ = runtime.with_telemetry(|telemetry| {
            // Push a metrics snapshot and flush the writer.

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

#[test]
fn batch_nostd_sliding_fixed_n_and_max_delta_t() {
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
        INGRESS_POLICY,
    );
    src.produce_n_items_in_backlog(16);

    let map = TestIdentityModelNodeTensor::<TEST_MAX_BATCH>::new(
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
        printer,
    );

    let q0: Q32 = Q32::default();
    let q1: Q32 = Q32::default();
    let sink = fixed_buffer_line_writer::<2048>();
    let telemetry: NoStdTestTelemetry = NoStdTestTelemetry::new(0, true, sink);
    let mgr0 = StaticMemoryManager::<TestTensor, 35>::new();
    let mgr1 = StaticMemoryManager::<TestTensor, 35>::new();

    let mut graph = SimpleExampleNoStdGraph::new(src, map, snk, q0, q1, mgr0, mgr1);
    let mut runtime: TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> =
        TestNoStdRuntime::new();
    runtime.init(&mut graph, clock, telemetry).unwrap();

    for _ in 0..10 {
        let _ = runtime.step(&mut graph).unwrap();
        #[cfg(feature = "std")]
        println!(
            "--- [graph_occupancies] --- {:?}",
            <TestNoStdRuntime<NoStdTestClock, NoStdTestTelemetry, 3, 3> as LimenRuntime<
                SimpleExampleNoStdGraph,
                3,
                3,
            >>::occupancies(&runtime)
        );
    }

    let occ = LimenRuntime::<SimpleExampleNoStdGraph, 3, 3>::occupancies(&runtime);
    // stride=2: edge1 grows net +1/cycle (3 in, 2 out), edge2 grows +2/cycle (3 in, 1 out)
    assert_eq!(*occ[1].items(), 6, "edge1 (src->map) should have 6 items");
    assert_eq!(*occ[2].items(), 6, "edge2 (map->snk) should have 6 items");

    // Safely inspect telemetry, if present (prints to stdout under `std`).
    #[cfg(feature = "std")]
    {
        let _ = runtime.with_telemetry(|telemetry| {
            // Push a metrics snapshot and flush the writer.

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
