//! Node contract test fixtures and helpers.
//!
//! This module contains a reusable, *single* contract test-suite that validates
//! the `Node` trait behaviour expected by Limen runtimes.  The tests exercise:
//!
//! - Node lifecycle: `initialize`, `start`, `on_watchdog_timeout`, `stop`.
//! - Per-message behaviour: `process_message` and the `step()` path that pops a
//!   single input and delegates to `process_message`.
//! - Batched behaviour: `step_batch()` semantics for fixed-size, sliding and
//!   fixed+max_delta_t windowing, including span validation based on
//!   `MessageHeader::creation_tick`.
//! - Error mapping: `QueueError` → `StepResult` / `NodeError` rules and how
//!   enqueue results map to progress/backpressure semantics.
//! - Telemetry and occupancy hooks exercised via `EdgeLink` + `GraphTelemetry`.
//!
//! Usage
//! -----
//! Implementors should provide:
//! - a `NodeLink` factory: `|| -> NodeLink<N, IN, OUT, InP, OutP>` that returns
//!   a fresh `NodeLink` owning the concrete node under test, and
//! - ensure payload types implement `Default + Clone` so tests can construct
//!   `Message::<P>::new(Header, P::default())` values.
//!
//! Then call the test macro from your crate tests:
//
//! ```text
//! run_node_contract_tests!(my_node_contracts, { make_nodelink: || create_mynode_link() });
//! ```
//!
//! The suite is *adaptive*: it skips or changes assertions for `IN == 0`
//! (sources), `OUT == 0` (sinks) or when `node.node_kind()` indicates a
//! wrapper-specific implementation (`Source`, `Sink`, `Model`). The tests only
//! assert node-level semantics (counts/StepResult/telemetry) and do **not**
//! inspect or require node-specific payload contents.
//!
//! ---------------------------------------------------------------------------
//! Planned node/context tests — NOT YET IMPLEMENTED
//!
//! C2 (N→M node arity — planned/C2.md):
//!   When C2 lands (multiple input ports, multiple output ports, optional
//!   inputs):
//!   - run_fan_in_two_inputs_both_available: step() with two filled input ports
//!     triggers the node and both tokens are consumed.
//!   - run_fan_in_one_input_absent_blocks: with an optional-absent policy,
//!     step() can still fire on one input.
//!   - run_fan_out_two_outputs: a single step produces messages on two
//!     distinct output ports; both queues gain one item.
//!   - run_fan_out_one_backpressured_maps_to_backpressure: if output port 1
//!     is at capacity and output port 0 succeeds, the step returns Backpressured.
//!
//! R1 (urgency ordering — planned/R1.md):
//!   - run_step_pop_respects_urgency_order: fill input queue with messages of
//!     mixed QoSClass; step() must process the highest-urgency item first.
//!
//! R2 (freshness / expiry — planned/R2.md):
//!   - run_step_skips_expired_inputs: push a message whose deadline is already
//!     elapsed; step() must not pass it to process_message (or must decrement
//!     a stale counter and skip).
//!
//! R4 (mailbox semantics — planned/R4.md):
//!   - run_step_mailbox_overwrites_previous: rapid pushes to a mailbox-mode
//!     input leave only the most recent; step() sees only the latest value.
//!
//! R5/R6 (liveness policies — planned/R5.md, R6.md):
//!   - run_step_liveness_check_fires_on_timeout: if no input arrives within
//!     the configured liveness interval, the node must emit a watchdog event
//!     or return a liveness-timeout StepResult.
//!
//! P1 (PlatformBackend / controllable clock — planned/P1.md):
//!  - run_batch_delta_t_with_controllable_clock: inject a fake clock that
//!    advances in discrete steps; verify delta_t batching respects injected
//!    timestamps rather than wall time.
//!
//! RS1 (runtime lifecycle — planned/RS1.md):
//!   - run_stop_during_inflight_batch: call stop() while a batch is partially
//!     consumed; verify all tokens are freed and no manager slots leak.
//! ---------------------------------------------------------------------------

use super::*;
use crate::{
    message::MessageHeader,
    policy::{AdmissionPolicy, OverBudgetAction, QueueCaps},
    prelude::{
        fixed_buffer_line_writer, EdgeLink, FixedBuffer, FmtLineWriter, GraphTelemetry,
        NoStdLinuxMonotonicClock, NodeLink, StaticMemoryManager, TestSpscRingBuf,
    },
    types::{EdgeIndex, NodeIndex, PortId, PortIndex, Ticks},
};

use heapless::Vec;

// Fixed buffer sizes for test telemetry
const TELE_NODES: usize = 8;
const TELE_EDGES: usize = 16;
const TELE_BUF_BYTES: usize = 1024;

const TEST_EDGE_POLICY: EdgePolicy = EdgePolicy::new(
    QueueCaps::new(16, 14, None, None),
    AdmissionPolicy::DropNewest,
    OverBudgetAction::Drop,
);

/// Edge policy for tests that exercise DropOldest / pre-eviction paths.
/// caps: max_items=4, soft_items=2 — narrow enough to trigger eviction quickly.
const TEST_DROP_OLDEST_POLICY: EdgePolicy = EdgePolicy::new(
    QueueCaps::new(4, 2, None, None),
    AdmissionPolicy::DropOldest,
    OverBudgetAction::Drop,
);

/// Expand a canonical suite of node contract tests for a `NodeLink` factory.
///
/// # Purpose
/// Generate a focused `#[test]` module that runs the standard Node contract
/// fixtures (lifecycle, single-message, and batched behaviour, error/backpressure
/// mapping, and wrapper-specific checks for `Source`, `Sink`, and `Model`).
///
/// # Required argument
/// - `make_nodelink: || -> NodeLink<N, IN, OUT, InP, OutP>`
///   A zero-argument closure that returns a **fresh** `NodeLink` owning the
///   concrete node under test. The closure is invoked **once per generated test**
///   (i.e., the factory must return a new, independent `NodeLink` each time).
///
/// # Type requirements
/// - The node produced by your `NodeLink` must implement `Node<IN, OUT, InP, OutP>`.
/// - Payload types `InP` and `OutP` must implement `Payload + Default + Clone` so
///   the tests can synthesize `Message::new(header, P::default())` values.
///
/// # Behaviour
/// The macro expands to a `mod $mod_name { ... }` containing these tests:
/// - `initialize_start_stop_roundtrip`
/// - `process_message_enqueues_and_made_progress`
/// - `step_on_empty_returns_noinput`
/// - `step_pops_and_calls_process_message`
/// - `step_batch_respects_fixed_n_disjoint`
/// - `step_batch_respects_sliding_window`
/// - `step_maps_backpressure_and_errors`
/// - `source_specific_behaviour`
/// - `sink_specific_behaviour`
/// - `model_specific_batching_behaviour`
/// - `fixed_n_with_max_delta_t_behaviour`
///
/// Each generated test delegates to a fixture in `node::contract_tests`. The
/// suite is *adaptive*: fixtures will skip or adjust assertions for `IN == 0`
/// (sources), `OUT == 0` (sinks), or when the node reports `NodeKind::Source`,
/// `NodeKind::Sink`, or `NodeKind::Model`.
///
/// # Usage
/// ```rust
/// use limen_core::run_node_contract_tests;
///
/// run_node_contract_tests!(my_node_contracts, {
///     make_nodelink: || create_my_node_link()
/// });
/// ```
///
/// Put the macro invocation in your crate's tests (or `#[cfg(test)]` module).
/// Keep the `make_nodelink` factory cheap and deterministic so the per-test
/// instances are reliable.
///
/// # Notes
/// - Tests assert node-level semantics (counts, `StepResult`, telemetry) and do
///   **not** inspect payload internals. Implementors should ensure payloads
///   derive/implement `Default + Clone` for compatibility with the suite.
#[macro_export]
macro_rules! run_node_contract_tests {
    ($mod_name:ident, {
            make_nodelink: $make_nodelink:expr
        }) => {
        #[cfg(test)]
        mod $mod_name {
            use super::*;
            use $crate::node::contract_tests as fixtures;

            #[test]
            fn initialize_start_stop_roundtrip() {
                fixtures::run_initialize_start_stop_roundtrip(|| $make_nodelink());
            }

            #[test]
            fn process_message_enqueues_and_made_progress() {
                fixtures::run_process_message_enqueues_and_made_progress(|| $make_nodelink());
            }

            #[test]
            fn step_on_empty_returns_noinput() {
                fixtures::run_step_on_empty_returns_noinput(|| $make_nodelink());
            }

            #[test]
            fn step_pops_and_calls_process_message() {
                fixtures::run_step_pops_and_calls_process_message(|| $make_nodelink());
            }

            #[test]
            fn step_batch_respects_fixed_n_disjoint() {
                fixtures::run_step_batch_fixed_n_disjoint(|| $make_nodelink());
            }

            #[test]
            fn step_batch_respects_sliding_window() {
                fixtures::run_step_batch_sliding_window(|| $make_nodelink());
            }

            #[test]
            fn step_maps_backpressure_and_errors() {
                fixtures::run_step_maps_backpressure_and_errors(|| $make_nodelink());
            }

            #[test]
            fn source_specific_behaviour() {
                fixtures::run_source_specific_tests(|| $make_nodelink());
            }

            #[test]
            fn sink_specific_behaviour() {
                fixtures::run_sink_specific_tests(|| $make_nodelink());
            }

            #[test]
            fn model_specific_batching_behaviour() {
                fixtures::run_model_batching_tests(|| $make_nodelink());
            }

            #[test]
            fn fixed_n_with_max_delta_t_behaviour() {
                fixtures::run_step_batch_fixed_n_max_delta_t_tests(|| $make_nodelink());
            }

            #[test]
            fn push_output_drop_oldest_evicts_oldest_once() {
                fixtures::run_push_output_drop_oldest_evicts_oldest_once(|| $make_nodelink());
            }

            #[test]
            fn push_output_no_token_leak_on_backpressure() {
                fixtures::run_push_output_no_token_leak_on_backpressure(|| $make_nodelink());
            }

            #[test]
            fn push_output_evict_until_below_hard_no_double_eviction() {
                fixtures::run_push_output_evict_until_below_hard_no_double_eviction(|| {
                    $make_nodelink()
                });
            }
        }
    };
}

// -----------------------
// helpers
// -----------------------

/// Create a small `GraphTelemetry` instance used by contract tests.
///
/// The returned telemetry has fixed-size internal buffers suitable for unit
/// tests and enables node telemetry paths so tests can assert `processed()`
/// and other node/edge counters. Use this to construct a `StepContext`.
fn make_graph_telemetry(
) -> GraphTelemetry<TELE_NODES, TELE_EDGES, FmtLineWriter<FixedBuffer<TELE_BUF_BYTES>>> {
    GraphTelemetry::new(0u32, true, fixed_buffer_line_writer::<TELE_BUF_BYTES>())
}

/// Construct input/output `EdgeLink` arrays backed by `TestSpscRingBuf`.
///
/// Returns `(inputs, outputs)` arrays of length `IN` and `OUT` respectively.
/// Each `EdgeLink` uses `TEST_EDGE_POLICY` and deterministic `EdgeIndex` and
/// `PortId` values so tests are reproducible.
///
/// # Type constraints
/// `InP` and `OutP` must implement `Payload + Default + Clone`.
///
/// This helper is the canonical way to produce testable queues that implement
/// the `Edge` contract and integrate with `StepContext`.
#[allow(clippy::type_complexity)]
fn make_edge_links_for_node<const IN: usize, const OUT: usize>(
    base_upstream_node: NodeIndex,
    base_downstream_node: NodeIndex,
) -> (
    [EdgeLink<TestSpscRingBuf<16>>; IN],
    [EdgeLink<TestSpscRingBuf<16>>; OUT],
) {
    let inputs = core::array::from_fn(|i| {
        let queue = TestSpscRingBuf::<16>::new();
        let id = EdgeIndex::new(i + 1);
        let upstream_port = PortId::new(base_upstream_node, PortIndex::new(i));
        let downstream_port = PortId::new(base_downstream_node, PortIndex::new(i));
        EdgeLink::new(
            queue,
            id,
            upstream_port,
            downstream_port,
            TEST_EDGE_POLICY,
            Some("in"),
        )
    });

    let outputs = core::array::from_fn(|o| {
        let queue = TestSpscRingBuf::<16>::new();
        let id = EdgeIndex::new(o + 1);
        let upstream_port = PortId::new(base_upstream_node, PortIndex::new(o));
        let downstream_port = PortId::new(base_downstream_node, PortIndex::new(o));
        EdgeLink::new(
            queue,
            id,
            upstream_port,
            downstream_port,
            TEST_EDGE_POLICY,
            Some("out"),
        )
    });

    (inputs, outputs)
}

/// Build a `StepContext` from the provided `EdgeLink` arrays, clock and telemetry.
///
/// The returned `StepContext` wraps the given input/output `EdgeLink` arrays,
/// populates per-port `EdgePolicy` arrays with `TEST_EDGE_POLICY`, and provides
/// `node_id`, `in_edge_ids`, and `out_edge_ids` derived from the `EdgeLink`s.
///
/// # Notes
/// - `inputs` and `outputs` must be arrays of exactly `IN` and `OUT` length.
/// - `InP` / `OutP` must implement `Default + Clone` so tests can craft messages.
/// - Use this helper to produce the context passed to `Node::step` /
///   `Node::step_batch` in fixtures.
#[allow(clippy::type_complexity)]
fn build_step_context<
    'graph,
    'telemetry,
    'clock,
    const IN: usize,
    const OUT: usize,
    InP,
    OutP,
    C,
    T,
>(
    inputs: &'graph mut [EdgeLink<TestSpscRingBuf<16>>; IN],
    outputs: &'graph mut [EdgeLink<TestSpscRingBuf<16>>; OUT],
    in_managers: &'graph mut [StaticMemoryManager<InP, 16>; IN],
    out_managers: &'graph mut [StaticMemoryManager<OutP, 16>; OUT],
    clock: &'clock C,
    telemetry: &'telemetry mut T,
) -> crate::node::StepContext<
    'graph,
    'telemetry,
    'clock,
    IN,
    OUT,
    InP,
    OutP,
    EdgeLink<TestSpscRingBuf<16>>,
    EdgeLink<TestSpscRingBuf<16>>,
    StaticMemoryManager<InP, 16>,
    StaticMemoryManager<OutP, 16>,
    C,
    T,
>
where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    let in_policies = core::array::from_fn(|_| TEST_EDGE_POLICY);
    let out_policies = core::array::from_fn(|_| TEST_EDGE_POLICY);

    let mut inputs_ref_vec: Vec<&mut EdgeLink<TestSpscRingBuf<16>>, IN> = Vec::new();
    for elem in inputs.iter_mut() {
        assert!(inputs_ref_vec.push(elem).is_ok(), "inputs_ref_vec overflow");
    }

    let mut outputs_ref_vec: Vec<&mut EdgeLink<TestSpscRingBuf<16>>, OUT> = Vec::new();
    for elem in outputs.iter_mut() {
        assert!(
            outputs_ref_vec.push(elem).is_ok(),
            "outputs_ref_vec overflow"
        );
    }

    let mut in_mgrs_ref_vec: Vec<&mut StaticMemoryManager<InP, 16>, IN> = Vec::new();
    for elem in in_managers.iter_mut() {
        assert!(
            in_mgrs_ref_vec.push(elem).is_ok(),
            "in_mgrs_ref_vec overflow"
        );
    }

    let mut out_mgrs_ref_vec: Vec<&mut StaticMemoryManager<OutP, 16>, OUT> = Vec::new();
    for elem in out_managers.iter_mut() {
        assert!(
            out_mgrs_ref_vec.push(elem).is_ok(),
            "out_mgrs_ref_vec overflow"
        );
    }

    let inputs_ref: [&mut EdgeLink<TestSpscRingBuf<16>>; IN] = match inputs_ref_vec.into_array() {
        Ok(arr) => arr,
        Err(_) => panic!("inputs_ref_vec length mismatch"),
    };

    let outputs_ref: [&mut EdgeLink<TestSpscRingBuf<16>>; OUT] = match outputs_ref_vec.into_array()
    {
        Ok(arr) => arr,
        Err(_) => panic!("outputs_ref_vec length mismatch"),
    };

    let in_mgrs_ref: [&mut StaticMemoryManager<InP, 16>; IN] = match in_mgrs_ref_vec.into_array() {
        Ok(arr) => arr,
        Err(_) => panic!("in_mgrs_ref_vec length mismatch"),
    };

    let out_mgrs_ref: [&mut StaticMemoryManager<OutP, 16>; OUT] =
        match out_mgrs_ref_vec.into_array() {
            Ok(arr) => arr,
            Err(_) => panic!("out_mgrs_ref_vec length mismatch"),
        };

    let in_edge_ids = core::array::from_fn(|i| *inputs_ref[i].id().as_usize() as u32);
    let out_edge_ids = core::array::from_fn(|o| *outputs_ref[o].id().as_usize() as u32);

    crate::node::StepContext::new(
        inputs_ref,
        outputs_ref,
        in_mgrs_ref,
        out_mgrs_ref,
        in_policies,
        out_policies,
        0u32,
        in_edge_ids,
        out_edge_ids,
        clock,
        telemetry,
    )
}

/// Like `build_step_context` but applies `out_policy` to every output port.
/// Use this when a test needs to exercise eviction or backpressure paths that
/// differ from the default `TEST_EDGE_POLICY`.
#[allow(clippy::type_complexity)]
fn build_step_context_with_out_policy<
    'graph,
    'telemetry,
    'clock,
    const IN: usize,
    const OUT: usize,
    InP,
    OutP,
    C,
    T,
>(
    inputs: &'graph mut [EdgeLink<TestSpscRingBuf<16>>; IN],
    outputs: &'graph mut [EdgeLink<TestSpscRingBuf<16>>; OUT],
    in_managers: &'graph mut [StaticMemoryManager<InP, 16>; IN],
    out_managers: &'graph mut [StaticMemoryManager<OutP, 16>; OUT],
    out_policy: EdgePolicy,
    clock: &'clock C,
    telemetry: &'telemetry mut T,
) -> crate::node::StepContext<
    'graph,
    'telemetry,
    'clock,
    IN,
    OUT,
    InP,
    OutP,
    EdgeLink<TestSpscRingBuf<16>>,
    EdgeLink<TestSpscRingBuf<16>>,
    StaticMemoryManager<InP, 16>,
    StaticMemoryManager<OutP, 16>,
    C,
    T,
>
where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    let in_policies = core::array::from_fn(|_| TEST_EDGE_POLICY);
    let out_policies = core::array::from_fn(|_| out_policy); // <-- only difference

    let mut inputs_ref_vec: Vec<&mut EdgeLink<TestSpscRingBuf<16>>, IN> = Vec::new();
    for elem in inputs.iter_mut() {
        assert!(inputs_ref_vec.push(elem).is_ok(), "inputs_ref_vec overflow");
    }
    let mut outputs_ref_vec: Vec<&mut EdgeLink<TestSpscRingBuf<16>>, OUT> = Vec::new();
    for elem in outputs.iter_mut() {
        assert!(
            outputs_ref_vec.push(elem).is_ok(),
            "outputs_ref_vec overflow"
        );
    }
    let mut in_mgrs_ref_vec: Vec<&mut StaticMemoryManager<InP, 16>, IN> = Vec::new();
    for elem in in_managers.iter_mut() {
        assert!(
            in_mgrs_ref_vec.push(elem).is_ok(),
            "in_mgrs_ref_vec overflow"
        );
    }
    let mut out_mgrs_ref_vec: Vec<&mut StaticMemoryManager<OutP, 16>, OUT> = Vec::new();
    for elem in out_managers.iter_mut() {
        assert!(
            out_mgrs_ref_vec.push(elem).is_ok(),
            "out_mgrs_ref_vec overflow"
        );
    }

    let inputs_ref: [&mut EdgeLink<TestSpscRingBuf<16>>; IN] = match inputs_ref_vec.into_array() {
        Ok(arr) => arr,
        Err(_) => panic!("inputs_ref_vec length mismatch"),
    };
    let outputs_ref: [&mut EdgeLink<TestSpscRingBuf<16>>; OUT] = match outputs_ref_vec.into_array()
    {
        Ok(arr) => arr,
        Err(_) => panic!("outputs_ref_vec length mismatch"),
    };
    let in_mgrs_ref: [&mut StaticMemoryManager<InP, 16>; IN] = match in_mgrs_ref_vec.into_array() {
        Ok(arr) => arr,
        Err(_) => panic!("in_mgrs_ref_vec length mismatch"),
    };
    let out_mgrs_ref: [&mut StaticMemoryManager<OutP, 16>; OUT] =
        match out_mgrs_ref_vec.into_array() {
            Ok(arr) => arr,
            Err(_) => panic!("out_mgrs_ref_vec length mismatch"),
        };

    let in_edge_ids = core::array::from_fn(|i| *inputs_ref[i].id().as_usize() as u32);
    let out_edge_ids = core::array::from_fn(|o| *outputs_ref[o].id().as_usize() as u32);

    crate::node::StepContext::new(
        inputs_ref,
        outputs_ref,
        in_mgrs_ref,
        out_mgrs_ref,
        in_policies,
        out_policies,
        0u32,
        in_edge_ids,
        out_edge_ids,
        clock,
        telemetry,
    )
}

// -----------------------
// Fixtures
// -----------------------

/// Lifecycle: `initialize` → `start` → `on_watchdog_timeout` → `stop`.
///
/// Verifies that a fresh `NodeLink` can be initialized and started without
/// error, that calling `on_watchdog_timeout` returns a valid `StepResult`, and
/// that `stop` returns `Ok(())`. This test asserts only success paths and is
/// intended as a basic lifecycle smoke-test.
pub fn run_initialize_start_stop_roundtrip<N, const IN: usize, const OUT: usize, InP, OutP>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    let mut nlink = make_nodelink();
    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();

    nlink.initialize(&clock, &mut tele).expect("init ok");
    nlink.start(&clock, &mut tele).expect("start ok");

    let _ = nlink
        .on_watchdog_timeout(&clock, &mut tele)
        .expect("watchdog ok");

    nlink.stop(&clock, &mut tele).expect("stop ok");
}

/// Single-message `process_message` path and basic egress semantics.
///
/// - Pushes one `Message<InP>` (header creation tick sourced from the clock)
///   into input port 0 (skipped when `IN == 0`).
/// - Calls `step()` and asserts the result is not `NoInput`.
/// - If `OUT > 0`, asserts at least one message was enqueued on output 0.
/// - Asserts the node telemetry `processed()` counter increased.
///
/// This fixture tests the canonical single-message processing path without
/// asserting message payload contents.
pub fn run_process_message_enqueues_and_made_progress<
    N,
    const IN: usize,
    const OUT: usize,
    InP,
    OutP,
>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    let mut nlink = make_nodelink();
    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();

    nlink.initialize(&clock, &mut tele).expect("init ok");

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    if IN == 0 {
        return;
    }

    let mut hdr = MessageHeader::empty();
    hdr.set_creation_tick(clock.now_ticks());
    let msg = Message::new(hdr, InP::default());

    let in_policy = TEST_EDGE_POLICY;
    let token = in_mgrs[0].store(msg).expect("store ok");
    assert_eq!(
        in_links[0].try_push(token, &in_policy, &in_mgrs[0]),
        crate::edge::EnqueueResult::Enqueued
    );

    let mut ctx = build_step_context(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        &clock,
        &mut tele,
    );

    let res = nlink.step(&mut ctx).expect("step ok");
    assert!(res != crate::node::StepResult::NoInput);

    if OUT > 0 {
        let mut pushed = 0usize;
        loop {
            match out_links[0].try_pop(&out_mgrs[0]) {
                Ok(_token) => pushed += 1,
                Err(QueueError::Empty) => break,
                Err(e) => panic!("unexpected queue error: {:?}", e),
            }
        }
        assert!(
            pushed > 0,
            "expected node to push at least one message on output 0"
        );
    }

    let metrics = tele.metrics();
    let processed = metrics.nodes()[0].processed();
    assert!(
        *processed >= 1u64,
        "expected processed >= 1, got {}",
        processed
    );
}

/// `step()` on empty inputs must return `StepResult::NoInput`.
///
/// Builds empty input queues and confirms `step()` returns `NoInput`. This
/// verifies scheduler-readiness predicates and the node's empty-input fast
/// path.
pub fn run_step_on_empty_returns_noinput<N, const IN: usize, const OUT: usize, InP, OutP>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    let mut nlink = make_nodelink();
    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    let mut ctx = build_step_context(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        &clock,
        &mut tele,
    );

    let res = nlink.step(&mut ctx).expect("step ok");

    if IN == 0 {
        assert!(
            res == crate::node::StepResult::NoInput || res == crate::node::StepResult::MadeProgress,
            "expected NoInput or MadeProgress for zero-input node, got {:?}",
            res
        );
    } else {
        assert_eq!(res, crate::node::StepResult::NoInput);
    }
}

/// `step()` pops one message and delegates to `process_message`.
///
/// - Pushes a single message into input port 0 (skipped when `IN == 0`).
/// - Calls `step()` and asserts the node made progress (not `NoInput`).
/// - If `OUT > 0`, asserts at least one output item was produced.
/// - Asserts telemetry processed counter incremented.
///
/// Ensures the node honors `step()` semantics and emits telemetry as expected.
pub fn run_step_pops_and_calls_process_message<N, const IN: usize, const OUT: usize, InP, OutP>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    let mut nlink = make_nodelink();
    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    if IN == 0 {
        return;
    }

    let mut hdr = MessageHeader::empty();
    hdr.set_creation_tick(clock.now_ticks());
    let msg = Message::new(hdr, InP::default());

    let policy = TEST_EDGE_POLICY;
    let token = in_mgrs[0].store(msg).expect("store ok");
    assert_eq!(
        in_links[0].try_push(token, &policy, &in_mgrs[0]),
        crate::edge::EnqueueResult::Enqueued
    );

    let mut ctx = build_step_context(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        &clock,
        &mut tele,
    );

    let res = nlink.step(&mut ctx).expect("step ok");
    assert!(res != crate::node::StepResult::NoInput);

    if OUT > 0 {
        let mut popped = 0usize;
        while let Ok(_token) = out_links[0].try_pop(&out_mgrs[0]) {
            popped += 1;
        }
        assert!(popped > 0, "expected output items");
    }

    let metrics = tele.metrics();
    assert!(*metrics.nodes()[0].processed() >= 1u64);
}

/// `step_batch()` with fixed-N disjoint semantics.
///
/// - Pushes `fixed_n + 1` messages into input port 0 (skipped when `IN == 0`).
/// - Calls `step_batch()` and asserts it returned a progress result.
/// - If `fixed_n > 0` and `OUT > 0`, asserts the node produced at least
///   `fixed_n` outputs (i.e., full fixed-size batch processed).
///
/// This checks the default fixed-size batch behaviour and that the node
/// consumes and emits the expected number of items under disjoint semantics.
pub fn run_step_batch_fixed_n_disjoint<N, const IN: usize, const OUT: usize, InP, OutP>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    let mut nlink = make_nodelink();
    const TEST_FIXED_N: usize = 3;

    let base_policy = nlink.node().policy();
    let batching = crate::policy::BatchingPolicy::with_window(
        Some(TEST_FIXED_N),
        None,
        crate::policy::WindowKind::Disjoint,
    );
    let new_policy = NodePolicy::new(batching, *base_policy.budget(), *base_policy.deadline());
    nlink.set_policy(new_policy);

    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    if IN == 0 {
        return;
    }

    let fixed_n = nlink.node().policy().batching().fixed_n().unwrap_or(1usize);

    let policy = TEST_EDGE_POLICY;
    for t in 1u64..=(fixed_n as u64 + 1) {
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(Ticks::new(t));
        let m = Message::new(hdr, InP::default());
        let token = in_mgrs[0].store(m).expect("store ok");
        assert_eq!(
            in_links[0].try_push(token, &policy, &in_mgrs[0]),
            crate::edge::EnqueueResult::Enqueued
        );
    }

    let in_before = *in_links[0].occupancy(&policy).items();

    let mut ctx = build_step_context(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        &clock,
        &mut tele,
    );

    let res = nlink.step(&mut ctx).expect("step_batch ok");
    assert!(res != crate::node::StepResult::NoInput);

    let in_after = *ctx.in_occupancy(0).items();
    assert_eq!(
        in_before.saturating_sub(in_after),
        fixed_n,
        "expected fixed_n items popped from input"
    );

    if OUT > 0 {
        let mut out_count = 0usize;
        while let Ok(_token) = out_links[0].try_pop(&out_mgrs[0]) {
            out_count += 1;
        }
        if fixed_n > 0 {
            assert_eq!(
                out_count, fixed_n,
                "expected out_count == fixed_n (got {}, fixed_n={})",
                out_count, fixed_n
            );
        } else {
            assert!(out_count >= 1, "expected at least one output");
        }
    }

    let metrics = tele.metrics();
    if fixed_n > 1 {
        assert_eq!(
            *metrics.nodes()[0].processed(),
            fixed_n as u64,
            "expected processed == fixed_n for batched step"
        );
    } else {
        assert!(*metrics.nodes()[0].processed() >= 1u64);
    }
}

/// `step_batch()` with sliding-window semantics (smoke test).
///
/// - Pushes several messages into input port 0 to exercise sliding-window
///   batch processing and stride semantics.
/// - Calls `step_batch()` and asserts the node returned progress.
/// - Verifies telemetry processed counter incremented.
///
/// This is a behavioural smoke-test rather than a precise numerical check of
/// popped/produced counts because sliding windows and backpressure can affect
/// exact numbers.
pub fn run_step_batch_sliding_window<N, const IN: usize, const OUT: usize, InP, OutP>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    let mut nlink = make_nodelink();

    const TEST_FIXED_N: usize = 4;
    const TEST_STRIDE: usize = 2;

    let base_policy = nlink.node().policy();
    let batching = crate::policy::BatchingPolicy::with_window(
        Some(TEST_FIXED_N),
        None,
        crate::policy::WindowKind::Sliding(crate::policy::SlidingWindow::new(TEST_STRIDE)),
    );
    let new_policy = NodePolicy::new(batching, *base_policy.budget(), *base_policy.deadline());
    nlink.set_policy(new_policy);

    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    if IN == 0 {
        return;
    }

    let policy = TEST_EDGE_POLICY;
    for t in 1u64..=6u64 {
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(Ticks::new(t));
        let m = Message::new(hdr, InP::default());
        let token = in_mgrs[0].store(m).expect("store ok");
        assert_eq!(
            in_links[0].try_push(token, &policy, &in_mgrs[0]),
            crate::edge::EnqueueResult::Enqueued
        );
    }

    let in_before = *in_links[0].occupancy(&policy).items();

    let mut ctx = build_step_context(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        &clock,
        &mut tele,
    );

    let res = nlink.step(&mut ctx).expect("step_batch ok");
    assert!(res != crate::node::StepResult::NoInput);

    let in_after = *ctx.in_occupancy(0).items();

    let stride_to_pop = core::cmp::min(TEST_STRIDE, in_before);
    let removed = in_before.saturating_sub(in_after);

    assert_eq!(
        removed, stride_to_pop,
        "unexpected number popped: removed={}, expected stride {}",
        removed, stride_to_pop
    );

    let fixed_n = nlink.node().policy().batching().fixed_n().unwrap_or(1usize);
    let expected_present = core::cmp::min(in_before, fixed_n);

    if OUT > 0 {
        let mut out_count = 0usize;
        while let Ok(_token) = out_links[0].try_pop(&out_mgrs[0]) {
            out_count += 1;
        }
        assert_eq!(
            out_count, expected_present,
            "expected out_count == expected_present (got {}, expected {})",
            out_count, expected_present
        );
    }

    let metrics = tele.metrics();
    if fixed_n > 1 {
        assert_eq!(
            *metrics.nodes()[0].processed(),
            fixed_n as u64,
            "expected processed == fixed_n for batched step"
        );
    } else {
        assert!(*metrics.nodes()[0].processed() >= 1u64);
    }
}

/// Mapping of output backpressure and queue errors to node-level results.
///
/// - Prefills an output queue to cause admission/backpressure conditions.
/// - Pushes a single input and calls `step()`.
/// - Accepts either:
///     - `Ok(StepResult::Backpressured|MadeProgress|… )`, or
///     - `Err(NodeError::backpressured())` / `Err(NodeError::execution_failed())`,
///       depending on whether the implementation surfaces backpressure as a
///       `StepResult` or an error.
///
/// Ensures the node maps queue/enqueue failures into the documented contract
/// (progress vs. backpressure vs. execution failure).
pub fn run_step_maps_backpressure_and_errors<N, const IN: usize, const OUT: usize, InP, OutP>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    if IN == 0 || OUT == 0 {
        return;
    }

    let mut nlink = make_nodelink();
    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    // prefill output 0 until it rejects or drops
    let policy = TEST_EDGE_POLICY;
    loop {
        let dummy_out_msg = Message::new(MessageHeader::empty(), OutP::default());
        let token = match out_mgrs[0].store(dummy_out_msg) {
            Ok(t) => t,
            Err(_) => break, // manager full
        };
        match out_links[0].try_push(token, &policy, &out_mgrs[0]) {
            crate::edge::EnqueueResult::Enqueued => continue,
            crate::edge::EnqueueResult::DroppedNewest | crate::edge::EnqueueResult::Rejected => {
                break
            }
        }
    }

    // push a single input so step will attempt to push to the full output
    let mut hdr = MessageHeader::empty();
    hdr.set_creation_tick(clock.now_ticks());
    let msg = Message::new(hdr, InP::default());
    let token = in_mgrs[0].store(msg).expect("store ok");
    assert_eq!(
        in_links[0].try_push(token, &policy, &in_mgrs[0]),
        crate::edge::EnqueueResult::Enqueued
    );

    let mut ctx = build_step_context(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        &clock,
        &mut tele,
    );

    match nlink.step(&mut ctx) {
        Ok(res) => {
            assert!(res != crate::node::StepResult::NoInput);
        }
        Err(_e) => {
            // Error is acceptable
        }
    }
}

// -----------------------
// Source-specific Fixtures
// -----------------------

/// Source-node specific checks.
///
/// Applicable only when `node.node_kind() == NodeKind::Source` (and `IN == 0`).
/// - Calls `step()` and asserts `NoInput` or `MadeProgress` (sources can
///   produce at most one item per `step()`).
/// - Calls `step_batch()` to ensure it is callable and does not panic; this
///   also exercises ingress occupancy/peek semantics indirectly.
///
/// Does not assert source payload contents — only node-level readiness and
/// non-panicking behaviour.
pub fn run_source_specific_tests<N, const IN: usize, const OUT: usize, InP, OutP>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    let mut nlink = make_nodelink();
    let kind = nlink.node().node_kind();
    if kind != crate::node::NodeKind::Source {
        return;
    }

    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    let mut ctx = build_step_context(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        &clock,
        &mut tele,
    );

    let res = nlink.step(&mut ctx).expect("step ok");
    assert!(
        res == crate::node::StepResult::NoInput || res == crate::node::StepResult::MadeProgress,
        "source.step should return NoInput or MadeProgress"
    );

    let _ = nlink.step(&mut ctx);
}

// -----------------------
// Sink-specific Fixtures
// -----------------------

/// Sink-node specific checks.
///
/// Applicable only when `node.node_kind() == NodeKind::Sink`.
/// - Pushes a message into input port 0 and calls `step()`.
/// - Asserts the sink either returns `MadeProgress` (consumed) or `NoInput`,
///   or returns an execution error to indicate failure of the underlying
///   sink implementation.
///
/// This fixture verifies the adapter `SinkNode` invokes `Sink::consume` and
/// maps sink errors to `NodeError::execution_failed()` as appropriate.
pub fn run_sink_specific_tests<N, const IN: usize, const OUT: usize, InP, OutP>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    let mut nlink = make_nodelink();
    let kind = nlink.node().node_kind();
    if kind != crate::node::NodeKind::Sink {
        return;
    }

    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    if IN == 0 {
        return;
    }

    let mut hdr = MessageHeader::empty();
    hdr.set_creation_tick(clock.now_ticks());
    let msg = Message::new(hdr, InP::default());
    let policy = TEST_EDGE_POLICY;
    let token = in_mgrs[0].store(msg).expect("store ok");
    assert_eq!(
        in_links[0].try_push(token, &policy, &in_mgrs[0]),
        crate::edge::EnqueueResult::Enqueued
    );

    let mut ctx = build_step_context(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        &clock,
        &mut tele,
    );

    let res = nlink.step(&mut ctx);
    match res {
        Ok(r) => {
            assert!(
                r == crate::node::StepResult::MadeProgress || r == crate::node::StepResult::NoInput,
                "sink.step returned unexpected StepResult"
            );
        }
        Err(_e) => {}
    }
}

// -----------------------
// Model-specific Fixtures
// -----------------------

/// Model-node batching smoke-test.
///
/// Applicable when `node.node_kind() == NodeKind::Model` and the node is `1×1`.
/// - Pushes `fixed_n` messages (from the node policy) and calls `step_batch()`.
/// - Asserts progress and that at least one output is produced, and not more
///   than the requested `fixed_n`.
///
/// This validates that `InferenceModel`-style nodes honor the node's fixed
/// batching hints and produce a reasonable number of outputs. It intentionally
/// does not assert payload contents or exact clamping by backend caps (those
/// are implementation-specific).
pub fn run_model_batching_tests<N, const IN: usize, const OUT: usize, InP, OutP>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    let mut nlink = make_nodelink();
    if nlink.node().node_kind() != crate::node::NodeKind::Model || IN != 1 || OUT != 1 {
        return;
    }

    const TEST_FIXED_N: usize = 4;
    let base_policy = nlink.node().policy();
    let batching = crate::policy::BatchingPolicy::with_window(
        Some(TEST_FIXED_N),
        None,
        crate::policy::WindowKind::Disjoint,
    );
    let new_policy = NodePolicy::new(batching, *base_policy.budget(), *base_policy.deadline());
    nlink.set_policy(new_policy);

    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    let requested_fixed = nlink.node().policy().batching().fixed_n().unwrap_or(1usize);

    let policy = TEST_EDGE_POLICY;
    for t in 1u64..=(requested_fixed as u64) {
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(Ticks::new(t));
        let m = Message::new(hdr, InP::default());
        let token = in_mgrs[0].store(m).expect("store ok");
        assert_eq!(
            in_links[0].try_push(token, &policy, &in_mgrs[0]),
            crate::edge::EnqueueResult::Enqueued
        );
    }

    let mut ctx = build_step_context(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        &clock,
        &mut tele,
    );

    let res = nlink.step(&mut ctx).expect("step_batch ok");
    assert!(
        res != crate::node::StepResult::NoInput,
        "model.step_batch returned NoInput"
    );

    if OUT > 0 {
        let mut out_count = 0usize;
        while let Ok(_token) = out_links[0].try_pop(&out_mgrs[0]) {
            out_count += 1;
        }

        assert!(
            out_count >= 1,
            "expected at least one output from model batching"
        );

        assert!(
            out_count <= requested_fixed,
            "unexpectedly produced more outputs ({}) than requested_fixed ({})",
            out_count,
            requested_fixed
        );
    }

    let metrics = tele.metrics();
    assert_eq!(
        *metrics.nodes()[0].processed(),
        requested_fixed as u64,
        "expected processed == requested_fixed for a model batched step"
    );
}

// -----------------------
// Fixed-N + max_delta_t Fixtures
// -----------------------

/// Tests for `fixed_n + max_delta_t` span validation.
///
/// - **Valid span**: pushes `fixed_n` messages whose `creation_tick` values
///   lie within `max_delta_t` and asserts `step_batch()` processes a full
///   batch (exactly `fixed_n` outputs if `OUT > 0`).
/// - **Invalid span**: pushes `fixed_n` messages with creation ticks spaced
///   farther apart than `max_delta_t` and asserts `step_batch()` either
///   returns `NoInput` (preferred) or makes partial progress (allowed).
///
/// This fixture verifies the scheduler readiness predicate and peek-based
/// span validation used by batched nodes.
pub fn run_step_batch_fixed_n_max_delta_t_tests<N, const IN: usize, const OUT: usize, InP, OutP>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    if IN == 0 {
        return;
    }

    let mut nlink = make_nodelink();

    const TEST_FIXED_N: usize = 4;
    const TEST_MAX_DELTA_TICKS: u64 = 5u64;

    let base_policy = nlink.node().policy();
    let batching = crate::policy::BatchingPolicy::with_window(
        Some(TEST_FIXED_N),
        Some(crate::types::Ticks::new(TEST_MAX_DELTA_TICKS)),
        crate::policy::WindowKind::Disjoint,
    );
    let new_policy = NodePolicy::new(batching, *base_policy.budget(), *base_policy.deadline());
    nlink.set_policy(new_policy);

    let policy_installed = *nlink.node().policy().batching();
    let fixed_opt = *policy_installed.fixed_n();
    let delta_opt = *policy_installed.max_delta_t();
    if fixed_opt.is_none() || delta_opt.is_none() {
        return;
    }
    let fixed_n = fixed_opt.unwrap();
    let max_delta = *delta_opt.unwrap().as_u64();

    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    // 1) VALID SPAN
    {
        let (mut in_links, mut out_links) =
            make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
        let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
            core::array::from_fn(|_| StaticMemoryManager::new());
        let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
            core::array::from_fn(|_| StaticMemoryManager::new());

        let policy = TEST_EDGE_POLICY;

        for i in 0..fixed_n {
            let tick = i as u64;
            let mut hdr = MessageHeader::empty();
            hdr.set_creation_tick(Ticks::new(tick));
            let m = Message::new(hdr, InP::default());
            let token = in_mgrs[0].store(m).expect("store ok");
            assert_eq!(
                in_links[0].try_push(token, &policy, &in_mgrs[0]),
                crate::edge::EnqueueResult::Enqueued
            );
        }

        let metrics_before = tele.metrics();
        let processed_before = *metrics_before.nodes()[0].processed();

        let mut ctx = build_step_context(
            &mut in_links,
            &mut out_links,
            &mut in_mgrs,
            &mut out_mgrs,
            &clock,
            &mut tele,
        );

        let res = nlink.step(&mut ctx).expect("step_batch ok (valid span)");
        assert!(
            res != crate::node::StepResult::NoInput,
            "expected batch processed for valid span"
        );

        if OUT > 0 {
            let mut out_count = 0usize;
            while let Ok(_token) = out_links[0].try_pop(&out_mgrs[0]) {
                out_count += 1;
            }
            assert_eq!(
                out_count, fixed_n,
                "expected exactly fixed_n outputs ({}) for valid span, got {}",
                fixed_n, out_count
            );
        }

        let metrics_after = tele.metrics();
        let processed_after = *metrics_after.nodes()[0].processed();
        assert_eq!(
            processed_after.saturating_sub(processed_before),
            fixed_n as u64,
            "expected telemetry processed to increase by fixed_n for valid span"
        );
    }

    // 2) INVALID SPAN
    {
        let (mut in_links, mut out_links) =
            make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
        let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
            core::array::from_fn(|_| StaticMemoryManager::new());
        let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
            core::array::from_fn(|_| StaticMemoryManager::new());

        let policy = TEST_EDGE_POLICY;
        for i in 0..fixed_n {
            let tick = (i as u64) * (max_delta + 1000u64);
            let mut hdr = MessageHeader::empty();
            hdr.set_creation_tick(Ticks::new(tick));
            let m = Message::new(hdr, InP::default());
            let token = in_mgrs[0].store(m).expect("store ok");
            assert_eq!(
                in_links[0].try_push(token, &policy, &in_mgrs[0]),
                crate::edge::EnqueueResult::Enqueued
            );
        }

        let metrics_before_invalid = tele.metrics();
        let processed_before_invalid = *metrics_before_invalid.nodes()[0].processed();

        let mut ctx = build_step_context(
            &mut in_links,
            &mut out_links,
            &mut in_mgrs,
            &mut out_mgrs,
            &clock,
            &mut tele,
        );

        let res = nlink.step(&mut ctx).expect("step_batch ok (invalid span)");

        if res == crate::node::StepResult::NoInput {
            let metrics_after_invalid = tele.metrics();
            let processed_after_invalid = *metrics_after_invalid.nodes()[0].processed();
            assert_eq!(
                processed_after_invalid, processed_before_invalid,
                "expected no telemetry change when invalid span results in NoInput"
            );
        } else {
            assert_eq!(
                res,
                crate::node::StepResult::MadeProgress,
                "unexpected StepResult for invalid span: {:?}",
                res
            );

            if OUT > 0 {
                let mut out_count = 0usize;
                while let Ok(_token) = out_links[0].try_pop(&out_mgrs[0]) {
                    out_count += 1;
                }
                assert!(
                    out_count > 0 && out_count < fixed_n,
                    "expected partial progress for invalid span (0 < out_count < fixed_n), got {}",
                    out_count
                );
            }
        }
    }
}

/// Regression: push_output with DropOldest must evict exactly once when the output
/// queue is between soft and hard cap (Evict(1) decision), not twice.
///
/// Pre-fills output to 3 items (soft=2, hard=4 → BetweenSoftAndHard → Evict(1)).
/// push_output pre-evicts 1 (queue: 3→2), then calls try_push.
/// OLD try_push: called get_admission_decision again, saw 2 items ≥ soft=2, evicted
/// a second time (2→1), then pushed → 2 items. Double-eviction.
/// NEW try_push: Evict branch does not pop. Checks at_or_above_hard(2,_) = false,
/// enqueues → 3 items. Correct.
pub fn run_push_output_drop_oldest_evicts_oldest_once<
    N,
    const IN: usize,
    const OUT: usize,
    InP,
    OutP,
>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    if IN == 0 || OUT == 0 {
        return;
    }

    let mut nlink = make_nodelink();
    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    // Pre-fill output port 0 with 3 items (between soft=2 and hard=4 for
    // TEST_DROP_OLDEST_POLICY). Use TEST_EDGE_POLICY for the push — its caps
    // (max=16, soft=14) admit all items unconditionally at this fill level.
    for i in 0u64..3 {
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(Ticks::new(i + 1));
        let tok = out_mgrs[0]
            .store(Message::new(hdr, OutP::default()))
            .expect("store filler");
        assert_eq!(
            out_links[0].try_push(tok, &TEST_EDGE_POLICY, &out_mgrs[0]),
            crate::edge::EnqueueResult::Enqueued,
        );
    }

    // Push one input so step() fires process_message → Output → push_output.
    let in_tok = {
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(clock.now_ticks());
        in_mgrs[0]
            .store(Message::new(hdr, InP::default()))
            .expect("store input")
    };
    assert_eq!(
        in_links[0].try_push(in_tok, &TEST_EDGE_POLICY, &in_mgrs[0]),
        crate::edge::EnqueueResult::Enqueued,
    );

    // Output policy is TEST_DROP_OLDEST_POLICY (max=4, soft=2, DropOldest).
    // With 3 items pre-filled, push_output sees Evict(1).
    let mut ctx = build_step_context_with_out_policy(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        TEST_DROP_OLDEST_POLICY,
        &clock,
        &mut tele,
    );

    let res = nlink.step(&mut ctx).expect("step ok");
    assert_eq!(res, crate::node::StepResult::MadeProgress);

    // 3 pre-filled − 1 evicted + 1 pushed = 3. Double-eviction gives 2.
    let occ = out_links[0].occupancy(&TEST_DROP_OLDEST_POLICY);
    assert_eq!(
        *occ.items(),
        3,
        "expected 3 items (exactly 1 evicted, 1 pushed); double-eviction gives 2"
    );
}

/// push_output must free the manager slot when output is backpressured (DropNewest).
/// A leaked token would exhaust manager capacity and eventually panic on store.
/// Uses MemoryManager::available() before and after to verify the invariant.
pub fn run_push_output_no_token_leak_on_backpressure<
    N,
    const IN: usize,
    const OUT: usize,
    InP,
    OutP,
>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    if IN == 0 || OUT == 0 {
        return;
    }

    let mut nlink = make_nodelink();
    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    // Tight DropNewest: max=2, soft=1. One item in queue puts it at soft,
    // so the next push triggers DropNewest immediately.
    let tight_drop_newest = EdgePolicy::new(
        QueueCaps::new(2, 1, None, None),
        AdmissionPolicy::DropNewest,
        OverBudgetAction::Drop,
    );

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    // Pre-fill output port 0 with 1 item, using tight_drop_newest to confirm
    // the first push is admitted (queue is empty → below soft).
    {
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(Ticks::new(1));
        let tok = out_mgrs[0]
            .store(Message::new(hdr, OutP::default()))
            .expect("store filler");
        assert_eq!(
            out_links[0].try_push(tok, &tight_drop_newest, &out_mgrs[0]),
            crate::edge::EnqueueResult::Enqueued,
        );
    }

    // Record manager availability after pre-fill. After the backpressured step
    // this must be identical: push_output stores 1 token then frees it on DropNewest.
    let available_before = out_mgrs[0].available();

    // Push one input so step() fires.
    let in_tok = {
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(clock.now_ticks());
        in_mgrs[0]
            .store(Message::new(hdr, InP::default()))
            .expect("store input")
    };
    assert_eq!(
        in_links[0].try_push(in_tok, &TEST_EDGE_POLICY, &in_mgrs[0]),
        crate::edge::EnqueueResult::Enqueued,
    );

    let mut ctx = build_step_context_with_out_policy(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        tight_drop_newest,
        &clock,
        &mut tele,
    );

    // step() → push_output: stores new token (1 slot used), hits DropNewest
    // (queue already at soft=1), must free the stored token, return Backpressured.
    let res = nlink.step(&mut ctx).expect("step ok");
    assert_eq!(res, crate::node::StepResult::Backpressured);

    // Slot must have been freed: available is unchanged.
    assert_eq!(
        out_mgrs[0].available(),
        available_before,
        "manager slot leaked: push_output must free token on DropNewest backpressure"
    );

    // Queue occupancy is also unchanged (the dropped item was never enqueued).
    let occ = out_links[0].occupancy(&tight_drop_newest);
    assert_eq!(
        *occ.items(),
        1,
        "queue occupancy must not change on backpressure"
    );
}

/// Regression: push_output with DropOldest at hard cap (EvictUntilBelowHard) must
/// not double-evict when the caller's pre-eviction leaves the queue above soft.
///
/// Pre-fills output to 4 items (max=4 for TEST_DROP_OLDEST_POLICY → AtOrAboveHard
/// → EvictUntilBelowHard). push_output drains until below hard (4→3), then calls
/// try_push. 3 items > soft=2 → try_push decision is Evict(1).
/// OLD try_push: popped again (3→2), then pushed → 3 items. Double-eviction.
/// NEW try_push: Evict branch checks at_or_above_hard(3,_) = false, enqueues → 4.
pub fn run_push_output_evict_until_below_hard_no_double_eviction<
    N,
    const IN: usize,
    const OUT: usize,
    InP,
    OutP,
>(
    mut make_nodelink: impl FnMut() -> NodeLink<N, IN, OUT, InP, OutP>,
) where
    InP: crate::message::payload::Payload + Default + Clone,
    OutP: crate::message::payload::Payload + Default + Clone,
    N: crate::node::Node<IN, OUT, InP, OutP>,
{
    if IN == 0 || OUT == 0 {
        return;
    }

    let mut nlink = make_nodelink();
    let clock = NoStdLinuxMonotonicClock::new();
    let mut tele = make_graph_telemetry();
    nlink.initialize(&clock, &mut tele).expect("init ok");

    let (mut in_links, mut out_links) =
        make_edge_links_for_node::<IN, OUT>(NodeIndex::new(0), NodeIndex::new(1));
    let mut in_mgrs: [StaticMemoryManager<InP, 16>; IN] =
        core::array::from_fn(|_| StaticMemoryManager::new());
    let mut out_mgrs: [StaticMemoryManager<OutP, 16>; OUT] =
        core::array::from_fn(|_| StaticMemoryManager::new());

    // Pre-fill output port 0 to hard cap (4 items = max for TEST_DROP_OLDEST_POLICY).
    // Use TEST_EDGE_POLICY for the push (large caps → always admits at this level).
    for i in 0u64..4 {
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(Ticks::new(i + 1));
        let tok = out_mgrs[0]
            .store(Message::new(hdr, OutP::default()))
            .expect("store filler");
        assert_eq!(
            out_links[0].try_push(tok, &TEST_EDGE_POLICY, &out_mgrs[0]),
            crate::edge::EnqueueResult::Enqueued,
        );
    }

    // Push one input so step() fires.
    let in_tok = {
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(clock.now_ticks());
        in_mgrs[0]
            .store(Message::new(hdr, InP::default()))
            .expect("store input")
    };
    assert_eq!(
        in_links[0].try_push(in_tok, &TEST_EDGE_POLICY, &in_mgrs[0]),
        crate::edge::EnqueueResult::Enqueued,
    );

    // Output policy is TEST_DROP_OLDEST_POLICY (max=4, soft=2, DropOldest).
    // With 4 items pre-filled, push_output sees EvictUntilBelowHard.
    let mut ctx = build_step_context_with_out_policy(
        &mut in_links,
        &mut out_links,
        &mut in_mgrs,
        &mut out_mgrs,
        TEST_DROP_OLDEST_POLICY,
        &clock,
        &mut tele,
    );

    let res = nlink.step(&mut ctx).expect("step ok");
    assert_eq!(res, crate::node::StepResult::MadeProgress);

    // 4 pre-filled − 1 evicted (EvictUntilBelowHard stops at 3 < hard=4) + 1 pushed = 4.
    // Double-eviction (old try_push): evicts 1 more on Evict(1) branch → 3.
    let occ = out_links[0].occupancy(&TEST_DROP_OLDEST_POLICY);
    assert_eq!(
        *occ.items(),
        4,
        "expected 4 items (1 pre-evicted, 1 pushed, net stable); \
           double-eviction (old try_push Evict branch) gives 3"
    );
}
