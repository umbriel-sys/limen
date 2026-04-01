//! Uniform Node contract and lifecycle.
//!
//! Nodes are monomorphized by generics and const generics. There is **no dynamic
//! dispatch** in the hot path. Port schemas and policies are encoded on the Node.

pub mod link;
pub mod model;
pub mod sink;
pub mod source;

#[cfg(any(test, feature = "bench"))]
pub mod bench;

#[cfg(any(test, feature = "bench"))]
pub mod contract_tests;
#[cfg(any(test, feature = "bench"))]
pub use contract_tests::*;

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::{NodeError, QueueError};
use crate::memory::PlacementAcceptance;
use crate::message::{payload::Payload, Message};
use crate::policy::{
    AdmissionDecision, BatchingPolicy, EdgePolicy, NodePolicy, SlidingWindow, WindowKind,
};
use crate::prelude::{BatchMessageIter, MemoryManager, PlatformClock, TelemetryKey, TelemetryKind};
use crate::telemetry::Telemetry;
use crate::types::Ticks;

/// Categories of nodes used in graph descriptors and builders.
///
/// These capture the high-level role of a node in the dataflow graph.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A source node: 0 inputs / ≥1 outputs.
    ///
    /// Examples: sensors, file readers, external ingress points.
    Source,
    /// A processing node: ≥1 inputs / ≥1 outputs.
    ///
    /// Examples: stateless transforms, stateful operators, pre/post-processing.
    Process,
    /// A model node: ≥1 inputs / ≥1 outputs.
    ///
    /// Represents inference nodes bound to a `ComputeBackend` and a model.
    Model,
    /// A split (fan-out) node: ≥1 inputs / ≥2 outputs.
    ///
    /// Used to branch one stream into multiple downstream paths.
    Split,
    /// A join (fan-in) node: ≥2 inputs / ≥1 outputs.
    ///
    /// Used to merge multiple streams into a single downstream path.
    Join,
    /// A sink node: ≥1 inputs / 0 outputs.
    ///
    /// Examples: file writers, stdout, GPIO, MQTT, other terminal sinks.
    Sink,
    /// An external node: request/response via transport to a remote or coprocessor.
    External,
}

/// Node capability descriptor (ops, dtypes, layouts, streams).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NodeCapabilities {
    /// Whether the node can execute on device streams (P2).
    device_streams: bool,
    /// Whether mixed-precision or degrade tiers are available.
    degrade_tiers: bool,
}

impl NodeCapabilities {
    /// Construct a new `NodeCapabilities`.
    #[inline]
    pub const fn new(device_streams: bool, degrade_tiers: bool) -> Self {
        Self {
            device_streams,
            degrade_tiers,
        }
    }

    /// Whether the node can execute on device streams (P2).
    #[inline]
    pub fn device_streams(&self) -> &bool {
        &self.device_streams
    }

    /// Whether mixed-precision or degrade tiers are available.
    #[inline]
    pub fn degrade_tiers(&self) -> &bool {
        &self.degrade_tiers
    }
}

/// Result of a `step` call indicating progress and scheduling hints.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    /// Work was performed (messages consumed and/or produced).
    MadeProgress,
    /// No inputs were available to make progress.
    NoInput,
    /// Backpressure prevented progress.
    Backpressured,
    /// Waiting on external completion (device, transport).
    WaitingOnExternal,
    /// Yield until provided tick (cooperative scheduling hint).
    YieldUntil(Ticks),
    /// Node has completed and will not produce further outputs.
    Terminal,
}

/// Result of processing a single input message.
///
/// Returned by [`Node::process_message`] to indicate what the node produced.
/// The framework handles pushing to output edges; the node never interacts
/// with queues or managers directly.
#[non_exhaustive]
#[derive(Debug)]
pub enum ProcessResult<P: Payload> {
    /// Processed the message and produced output to push to port 0.
    Output(Message<P>),
    /// Consumed the input but produced no output (sinks, filters).
    Consumed,
    /// Nothing to process / skip.
    Skip,
}

/// A context provided to nodes during `step`, abstracting queues, managers,
/// and services.
///
/// The context is generic over input/output payload, queue, **memory manager**,
/// clock, and telemetry types to avoid trait objects.
pub struct StepContext<
    'graph,
    'telemetry,
    'clock,
    const IN: usize,
    const OUT: usize,
    InP,
    OutP,
    InQ,
    OutQ,
    InM,
    OutM,
    C,
    T,
> where
    InP: Payload,
    OutP: Payload,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    /// Arrays of inbound queues by input port index.
    inputs: [&'graph mut InQ; IN],
    /// Arrays of outbound queues by output port index.
    outputs: [&'graph mut OutQ; OUT],
    /// Memory managers for each input port (one per edge).
    in_managers: [&'graph mut InM; IN],
    /// Memory managers for each output port (one per edge).
    out_managers: [&'graph mut OutM; OUT],
    /// Edge policies for each input.
    in_policies: [EdgePolicy; IN],
    /// Edge policies for each output.
    out_policies: [EdgePolicy; OUT],

    /// Node identifier for automatic telemetry stamping.
    node_id: u32,
    /// Input edge identifiers for automatic telemetry stamping.
    in_edge_ids: [u32; IN],
    /// Output edge identifiers for automatic telemetry stamping.
    out_edge_ids: [u32; OUT],

    /// Platform clock or timer services.
    clock: &'clock C,
    /// Telemetry sink for counters/histograms.
    telemetry: &'telemetry mut T,
    /// Phantom type markers to keep payload types visible to the compiler.
    _marker: core::marker::PhantomData<(InP, OutP)>,
}

impl<
        'graph,
        'telemetry,
        'clock,
        const IN: usize,
        const OUT: usize,
        InP,
        OutP,
        InQ,
        OutQ,
        InM,
        OutM,
        C,
        T,
    > StepContext<'graph, 'telemetry, 'clock, IN, OUT, InP, OutP, InQ, OutQ, InM, OutM, C, T>
where
    InP: Payload,
    OutP: Payload,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    /// Create a new step context from queues, managers, policies, and services.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inputs: [&'graph mut InQ; IN],
        outputs: [&'graph mut OutQ; OUT],
        in_managers: [&'graph mut InM; IN],
        out_managers: [&'graph mut OutM; OUT],
        in_policies: [EdgePolicy; IN],
        out_policies: [EdgePolicy; OUT],
        node_id: u32,
        in_edge_ids: [u32; IN],
        out_edge_ids: [u32; OUT],
        clock: &'clock C,
        telemetry: &'telemetry mut T,
    ) -> Self {
        Self {
            inputs,
            outputs,
            in_managers,
            out_managers,
            in_policies,
            out_policies,
            node_id,
            in_edge_ids,
            out_edge_ids,
            clock,
            telemetry,
            _marker: core::marker::PhantomData,
        }
    }
}

impl<
        'graph,
        'telemetry,
        'clock,
        const IN: usize,
        const OUT: usize,
        InP,
        OutP,
        InQ,
        OutQ,
        InM,
        OutM,
        C,
        T,
    > StepContext<'graph, 'telemetry, 'clock, IN, OUT, InP, OutP, InQ, OutQ, InM, OutM, C, T>
where
    InP: Payload,
    OutP: Payload,
    InQ: Edge,
    OutQ: Edge,
    InM: MemoryManager<InP>,
    OutM: MemoryManager<OutP>,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    // ---------------------------------------------------------------
    // Input operations
    // ---------------------------------------------------------------

    /// Peek the front message header on the specified input port (non-consuming).
    ///
    /// Returns a guard that dereferences to `MessageHeader`. The guard must
    /// be dropped before any mutable operation on the same manager slot.
    #[inline]
    pub fn in_peek_header(&self, i: usize) -> Result<InM::HeaderGuard<'_>, QueueError> {
        debug_assert!(i < IN);
        let token = self.inputs[i].try_peek()?;
        self.in_managers[i]
            .peek_header(token)
            .map_err(|_| QueueError::Empty)
    }

    // ---------------------------------------------------------------
    // Callback-based input operations
    // ---------------------------------------------------------------

    /// Pop one message from input `port`, call `f` with a shared reference,
    /// then push any output and free the manager slot.
    pub fn pop_and_process<F>(&mut self, port: usize, f: F) -> Result<StepResult, NodeError>
    where
        F: FnOnce(&Message<InP>) -> Result<ProcessResult<OutP>, NodeError>,
    {
        debug_assert!(port < IN);

        let token = match self.inputs[port].try_pop(&*self.in_managers[port]) {
            Ok(t) => t,
            Err(QueueError::Empty) => return Ok(StepResult::NoInput),
            Err(QueueError::Backpressured) | Err(QueueError::AtOrAboveHardCap) => {
                return Err(NodeError::backpressured());
            }
            Err(QueueError::Poisoned) | Err(QueueError::Unsupported) => {
                return Err(NodeError::execution_failed());
            }
        };

        let guard = self.in_managers[port]
            .read(token)
            .map_err(|_| NodeError::execution_failed())?;

        let result = f(&*guard)?;

        drop(guard);
        let _ = self.in_managers[port].free(token);

        if T::METRICS_ENABLED {
            self.telemetry.incr_counter(
                TelemetryKey::node(self.node_id, TelemetryKind::IngressMsgs),
                1,
            );
            let _ = self.in_occupancy(port);
        }

        match result {
            ProcessResult::Output(out_msg) => self.push_output(0, out_msg),
            ProcessResult::Consumed => Ok(StepResult::MadeProgress),
            ProcessResult::Skip => Ok(StepResult::NoInput),
        }
    }

    /// Pop a batch from input `port`. Set batch flags on popped tokens.
    /// Call `f` for each message in the batch, pushing any outputs internally.
    /// After processing, free all consumed tokens.
    pub fn pop_batch_and_process<F>(
        &mut self,
        port: usize,
        nmax: usize,
        node_policy: &NodePolicy,
        mut f: F,
    ) -> Result<StepResult, NodeError>
    where
        F: FnMut(&Message<InP>) -> Result<ProcessResult<OutP>, NodeError>,
    {
        debug_assert!(port < IN);

        if nmax == 0 {
            return Err(NodeError::execution_failed());
        }

        // Build clamped batching policy from node policy.
        let requested_policy = {
            let nb = *node_policy.batching();
            BatchingPolicy::with_window(
                nb.fixed_n().map(|f_n| core::cmp::min(f_n, nmax)),
                *nb.max_delta_t(),
                match nb.window_kind() {
                    WindowKind::Disjoint => WindowKind::Disjoint,
                    WindowKind::Sliding(sw) => {
                        let size = nb
                            .fixed_n()
                            .map(|f_n| core::cmp::min(f_n, nmax))
                            .unwrap_or(1);
                        let stride = core::cmp::min(*sw.stride(), size);
                        WindowKind::Sliding(SlidingWindow::new(stride))
                    }
                },
            )
        };

        // Determine stride for free decisions.
        let stride = match requested_policy.window_kind() {
            WindowKind::Disjoint => usize::MAX,
            WindowKind::Sliding(sw) => *sw.stride(),
        };

        // Sample pre-pop occupancy.
        let occ_before = self.inputs[port].occupancy(&self.in_policies[port]);

        // Pop batch of tokens.
        let batch =
            match self.inputs[port].try_pop_batch(&requested_policy, &*self.in_managers[port]) {
                Ok(b) => b,
                Err(QueueError::Empty) => return Ok(StepResult::NoInput),
                Err(QueueError::Backpressured) | Err(QueueError::AtOrAboveHardCap) => {
                    return Err(NodeError::backpressured());
                }
                Err(QueueError::Poisoned) => {
                    return Err(NodeError::execution_failed().with_code(1));
                }
                Err(QueueError::Unsupported) => {
                    return Err(NodeError::execution_failed().with_code(2));
                }
            };

        let batch_len = batch.len();
        if batch_len == 0 {
            return Ok(StepResult::NoInput);
        }
        let actual_stride = core::cmp::min(stride, batch_len);

        // Disjoint field split for input manager.
        let in_mgr: &mut InM = &mut *self.in_managers[port];

        // Phase 1: set batch boundary flags on popped tokens.
        for (idx, &token) in batch.as_slice().iter().enumerate() {
            if idx < actual_stride {
                if let Ok(mut wg) = in_mgr.read_mut(token) {
                    if idx == 0 {
                        wg.header_mut().set_first_in_batch();
                    }
                    if idx == batch_len - 1 || batch_len == 1 {
                        wg.header_mut().set_last_in_batch();
                    }
                }
            }
        }

        // Phase 2: build OutStepContext for disjoint output access, then
        // iterate batch calling process_message per item.
        let iter =
            BatchMessageIter::new(batch.as_slice().iter(), &*in_mgr, actual_stride, batch_len);

        let out_policies = self.out_policies;
        let out_edge_ids = self.out_edge_ids;
        let node_id = self.node_id;
        let clock = self.clock;
        let telemetry: &mut T = &mut *self.telemetry;
        let outputs = &mut self.outputs;
        let out_managers = &mut self.out_managers;

        let mut out = OutStepContext {
            outputs,
            out_managers,
            out_policies,
            out_edge_ids,
            node_id,
            clock,
            telemetry,
            _marker: core::marker::PhantomData,
        };

        let mut any_made = false;
        let mut backpressured = false;
        for guard in iter {
            if backpressured {
                // Once backpressured, skip remaining messages but keep iterating
                // to drop the iterator cleanly.
                drop(guard);
                continue;
            }
            match f(&*guard)? {
                ProcessResult::Output(out_msg) => {
                    drop(guard);
                    match out.out_try_push(0, out_msg) {
                        EnqueueResult::Enqueued => {
                            any_made = true;
                        }
                        EnqueueResult::DroppedNewest | EnqueueResult::Rejected => {
                            backpressured = true;
                        }
                    }
                }
                ProcessResult::Consumed => {
                    drop(guard);
                    any_made = true;
                }
                ProcessResult::Skip => {
                    drop(guard);
                }
            }
        }

        // Phase 3: free consumed tokens.
        for (idx, &token) in batch.as_slice().iter().enumerate() {
            if idx < actual_stride {
                let _ = in_mgr.free(token);
            }
        }

        // Telemetry.
        if T::METRICS_ENABLED {
            let telemetry = &mut *out.telemetry;
            telemetry.incr_counter(
                TelemetryKey::node(node_id, TelemetryKind::IngressMsgs),
                actual_stride as u64,
            );
            let after_items = occ_before.items().saturating_sub(actual_stride);
            telemetry.set_gauge(
                TelemetryKey::edge(self.in_edge_ids[port], TelemetryKind::QueueDepth),
                after_items as u64,
            );
        }

        if backpressured {
            Ok(StepResult::Backpressured)
        } else if any_made {
            Ok(StepResult::MadeProgress)
        } else {
            Ok(StepResult::NoInput)
        }
    }

    /// Return a snapshot of occupancy of the specified input queue.
    #[inline]
    pub fn in_occupancy(&mut self, i: usize) -> EdgeOccupancy {
        debug_assert!(i < IN);
        let occ = self.inputs[i].occupancy(&self.in_policies[i]);
        if T::METRICS_ENABLED {
            self.telemetry.set_gauge(
                TelemetryKey::edge(self.in_edge_ids[i], TelemetryKind::QueueDepth),
                *occ.items() as u64,
            );
        }
        occ
    }

    /// Return the policy of the specified input queue.
    #[inline]
    pub fn in_policy(&mut self, i: usize) -> EdgePolicy {
        debug_assert!(i < IN);
        self.in_policies[i]
    }

    // ---------------------------------------------------------------
    // Output operations
    // ---------------------------------------------------------------

    /// Push a message to the specified output port.
    ///
    /// Stores the message in the output memory manager, then pushes the
    /// resulting token to the edge. Handles eviction: if DropOldest evicts
    /// a token, the evicted token is freed from the manager.
    pub fn out_try_push(&mut self, o: usize, m: Message<OutP>) -> EnqueueResult {
        debug_assert!(o < OUT);

        let token = match self.out_managers[o].store(m) {
            Ok(t) => t,
            Err(_) => return EnqueueResult::Rejected,
        };

        // Pre-eviction: query admission and pop+free any tokens that must make room.
        let decision = self.outputs[o].get_admission_decision(
            &self.out_policies[o],
            token,
            &*self.out_managers[o],
        );
        match decision {
            AdmissionDecision::Evict(n) => {
                for _ in 0..n {
                    match self.outputs[o].try_pop(&*self.out_managers[o]) {
                        Ok(evicted) => {
                            let _ = self.out_managers[o].free(evicted);
                        }
                        Err(_) => break,
                    }
                }
            }
            AdmissionDecision::EvictUntilBelowHard => loop {
                let occ = self.outputs[o].occupancy(&self.out_policies[o]);
                if !self.out_policies[o]
                    .caps
                    .at_or_above_hard(*occ.items(), *occ.bytes())
                {
                    break;
                }
                match self.outputs[o].try_pop(&*self.out_managers[o]) {
                    Ok(evicted) => {
                        let _ = self.out_managers[o].free(evicted);
                    }
                    Err(_) => break,
                }
            },
            AdmissionDecision::DropNewest => {
                let _ = self.out_managers[o].free(token);
                if T::METRICS_ENABLED {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
                return EnqueueResult::DroppedNewest;
            }
            AdmissionDecision::Reject | AdmissionDecision::Block => {
                let _ = self.out_managers[o].free(token);
                if T::METRICS_ENABLED {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
                return EnqueueResult::Rejected;
            }
            AdmissionDecision::Admit => {}
        }

        match self.outputs[o].try_push(token, &self.out_policies[o], &*self.out_managers[o]) {
            EnqueueResult::Enqueued => {
                if T::METRICS_ENABLED {
                    self.telemetry.incr_counter(
                        TelemetryKey::node(self.node_id, TelemetryKind::EgressMsgs),
                        1,
                    );
                    let _ = self.out_occupancy(o);
                }
                EnqueueResult::Enqueued
            }
            EnqueueResult::DroppedNewest | EnqueueResult::Rejected => {
                let _ = self.out_managers[o].free(token);
                if T::METRICS_ENABLED {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
                EnqueueResult::Rejected
            }
        }
    }

    /// Push an output message to the specified output port and map the result
    /// to a `StepResult`.
    ///
    /// Stores the message in the output memory manager, performs pre-eviction
    /// (popping and freeing all slots the admission policy requires), then
    /// pushes the token to the edge. Residual eviction from a concurrent race
    /// is also freed. This guarantees zero manager slot leaks across all edge
    /// tiers and admission policies.
    pub fn push_output(
        &mut self,
        port: usize,
        msg: Message<OutP>,
    ) -> Result<StepResult, NodeError> {
        debug_assert!(port < OUT);

        // Store message first so the token's header is available for admission
        // decisions.
        let token = self.out_managers[port]
            .store(msg)
            .map_err(|_| NodeError::execution_failed())?;

        // Pre-eviction: check what the policy requires and pop+free ALL
        // tokens that need to be evicted before the push. This prevents
        // manager slot leaks when Evict(n > 1) or EvictUntilBelowHard fires.
        let decision = self.outputs[port].get_admission_decision(
            &self.out_policies[port],
            token,
            &*self.out_managers[port],
        );
        match decision {
            AdmissionDecision::Evict(n) => {
                for _ in 0..n {
                    match self.outputs[port].try_pop(&*self.out_managers[port]) {
                        Ok(evicted) => {
                            let _ = self.out_managers[port].free(evicted);
                        }
                        Err(_) => break,
                    }
                }
            }
            AdmissionDecision::EvictUntilBelowHard => loop {
                let occ = self.outputs[port].occupancy(&self.out_policies[port]);
                if !self.out_policies[port]
                    .caps
                    .at_or_above_hard(*occ.items(), *occ.bytes())
                {
                    break;
                }
                match self.outputs[port].try_pop(&*self.out_managers[port]) {
                    Ok(evicted) => {
                        let _ = self.out_managers[port].free(evicted);
                    }
                    Err(_) => break,
                }
            },
            AdmissionDecision::DropNewest
            | AdmissionDecision::Reject
            | AdmissionDecision::Block => {
                // Not admitted — free the stored token and signal backpressure.
                let _ = self.out_managers[port].free(token);
                return Ok(StepResult::Backpressured);
            }
            AdmissionDecision::Admit => {}
        }

        // After pre-eviction the edge should Admit. Push and handle any
        // residual (e.g. concurrent race on a future ConcurrentEdge).
        match self.outputs[port].try_push(
            token,
            &self.out_policies[port],
            &*self.out_managers[port],
        ) {
            EnqueueResult::Enqueued => {
                if T::METRICS_ENABLED {
                    self.telemetry.incr_counter(
                        TelemetryKey::node(self.node_id, TelemetryKind::EgressMsgs),
                        1,
                    );
                    let _ = self.out_occupancy(port);
                }
                Ok(StepResult::MadeProgress)
            }
            EnqueueResult::DroppedNewest | EnqueueResult::Rejected => {
                let _ = self.out_managers[port].free(token);
                Ok(StepResult::Backpressured)
            }
        }
    }

    /// Return a snapshot of occupancy of the specified output queue.
    #[inline]
    pub fn out_occupancy(&mut self, o: usize) -> EdgeOccupancy {
        debug_assert!(o < OUT);
        let occ = self.outputs[o].occupancy(&self.out_policies[o]);
        if T::METRICS_ENABLED {
            self.telemetry.set_gauge(
                TelemetryKey::edge(self.out_edge_ids[o], TelemetryKind::QueueDepth),
                *occ.items() as u64,
            );
        }
        occ
    }

    /// Return the policy of the specified output queue.
    #[inline]
    pub fn out_policy(&mut self, i: usize) -> EdgePolicy {
        debug_assert!(i < OUT);
        self.out_policies[i]
    }

    // ---------------------------------------------------------------
    // Clock / telemetry
    // ---------------------------------------------------------------

    /// Access the platform clock used for timing and conversions.
    #[inline]
    pub fn clock(&self) -> &C {
        self.clock
    }

    /// Borrow the telemetry sink to emit custom counters/gauges/histograms.
    #[inline]
    pub fn telemetry_mut(&mut self) -> &mut T {
        self.telemetry
    }

    /// Current monotonic tick value from the platform clock.
    #[inline]
    pub fn now_ticks(&self) -> Ticks {
        self.clock.now_ticks()
    }

    /// Current time in nanoseconds per the clock's tick-to-ns mapping.
    #[inline]
    pub fn now_nanos(&self) -> u64 {
        self.clock.ticks_to_nanos(self.clock.now_ticks())
    }

    /// Convert clock ticks to nanoseconds using the clock's scale.
    #[inline]
    pub fn ticks_to_nanos(&self, t: Ticks) -> u64 {
        self.clock.ticks_to_nanos(t)
    }

    /// Convert nanoseconds to clock ticks using the clock's scale.
    #[inline]
    pub fn nanos_to_ticks(&self, ns: u64) -> Ticks {
        self.clock.nanos_to_ticks(ns)
    }

    // ---------------------------------------------------------------
    // Batch readiness
    // ---------------------------------------------------------------

    /// Return `true` if the input edge `port` can produce a batch under `policy`.
    ///
    /// Uses the input manager's `HeaderStore` to peek creation ticks for
    /// span validation when both `fixed_n` and `max_delta_t` are set.
    #[inline]
    pub fn input_edge_has_batch(&mut self, port: usize, policy: &NodePolicy) -> bool {
        debug_assert!(port < IN);

        let occ = self.in_occupancy(port);
        if occ.items() == &0 {
            return false;
        }

        let fixed_opt = *policy.batching().fixed_n();
        let delta_opt = *policy.batching().max_delta_t();

        match (fixed_opt, delta_opt) {
            (Some(fixed_n), None) => *occ.items() >= fixed_n,
            (None, Some(_max_delta_t)) => true,
            (Some(fixed_n), Some(max_delta_t)) => {
                if *occ.items() < fixed_n {
                    return false;
                }
                // Peek front and last token, look up creation ticks via HeaderStore.
                let first_token = match self.inputs[port].try_peek_at(0) {
                    Ok(t) => t,
                    Err(_) => return false,
                };
                let last_token = match self.inputs[port].try_peek_at(fixed_n - 1) {
                    Ok(t) => t,
                    Err(_) => return false,
                };

                let first_ticks = match self.in_managers[port].peek_header(first_token) {
                    Ok(h) => *h.creation_tick(),
                    Err(_) => return false,
                };
                let last_ticks = match self.in_managers[port].peek_header(last_token) {
                    Ok(h) => *h.creation_tick(),
                    Err(_) => return false,
                };

                let span = last_ticks.saturating_sub(first_ticks);
                span <= max_delta_t
            }
            (None, None) => true,
        }
    }

    /// Construct an `OutStepContext` by borrowing only the output-related
    /// fields and telemetry from `self`.
    #[allow(dead_code)]
    #[inline]
    fn as_out_step_context<'ctx>(
        &'ctx mut self,
    ) -> OutStepContext<'graph, 'ctx, 'clock, OUT, OutP, OutQ, OutM, C, T>
    where
        EdgePolicy: Copy,
    {
        let out_policies = self.out_policies;
        let out_edge_ids = self.out_edge_ids;
        let node_id = self.node_id;
        let clock = self.clock;
        let telemetry = &mut *self.telemetry;
        let outputs: &'ctx mut [&'graph mut OutQ; OUT] = &mut self.outputs;
        let out_managers: &'ctx mut [&'graph mut OutM; OUT] = &mut self.out_managers;

        OutStepContext {
            outputs,
            out_managers,
            out_policies,
            out_edge_ids,
            node_id,
            clock,
            telemetry,
            _marker: core::marker::PhantomData,
        }
    }
}

/// A `StepContext` *view* that only exposes outputs / managers / clock / telemetry.
///
/// Explicitly **does not** provide access to input queues or input managers.
pub struct OutStepContext<'graph, 'ctx, 'clock, const OUT: usize, OutP, OutQ, OutM, C, T>
where
    OutP: Payload,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    /// Mutable borrow of the outputs array from the original StepContext.
    outputs: &'ctx mut [&'graph mut OutQ; OUT],
    /// Mutable borrow of the output managers array.
    out_managers: &'ctx mut [&'graph mut OutM; OUT],
    /// Copy of per-output policies (EdgePolicy: Copy).
    out_policies: [EdgePolicy; OUT],
    /// Copy of output edge ids.
    out_edge_ids: [u32; OUT],
    /// Node id for telemetry.
    node_id: u32,
    /// Borrow the clock (shared).
    clock: &'clock C,
    /// Mutable borrow of telemetry from the StepContext (reborrowed).
    telemetry: &'ctx mut T,
    /// Phantom to keep OutP visible to the compiler.
    _marker: core::marker::PhantomData<OutP>,
}

impl<'graph, 'ctx, 'clock, const OUT: usize, OutP, OutQ, OutM, C, T>
    OutStepContext<'graph, 'ctx, 'clock, OUT, OutP, OutQ, OutM, C, T>
where
    OutP: Payload,
    OutQ: Edge,
    OutM: MemoryManager<OutP>,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    /// Push a message to an output queue via the memory manager.
    ///
    /// Stores the message in the manager, pushes the token to the edge,
    /// and handles eviction (frees evicted tokens from the manager).
    pub fn out_try_push(&mut self, o: usize, m: Message<OutP>) -> EnqueueResult {
        debug_assert!(o < OUT);

        let token = match self.out_managers[o].store(m) {
            Ok(t) => t,
            Err(_) => return EnqueueResult::Rejected,
        };

        // Pre-eviction: query admission and pop+free any tokens that must make room.
        let decision = self.outputs[o].get_admission_decision(
            &self.out_policies[o],
            token,
            &*self.out_managers[o],
        );
        match decision {
            AdmissionDecision::Evict(n) => {
                for _ in 0..n {
                    match self.outputs[o].try_pop(&*self.out_managers[o]) {
                        Ok(evicted) => {
                            let _ = self.out_managers[o].free(evicted);
                        }
                        Err(_) => break,
                    }
                }
            }
            AdmissionDecision::EvictUntilBelowHard => loop {
                let occ = self.outputs[o].occupancy(&self.out_policies[o]);
                if !self.out_policies[o]
                    .caps
                    .at_or_above_hard(*occ.items(), *occ.bytes())
                {
                    break;
                }
                match self.outputs[o].try_pop(&*self.out_managers[o]) {
                    Ok(evicted) => {
                        let _ = self.out_managers[o].free(evicted);
                    }
                    Err(_) => break,
                }
            },
            AdmissionDecision::DropNewest => {
                let _ = self.out_managers[o].free(token);
                if T::METRICS_ENABLED {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
                return EnqueueResult::DroppedNewest;
            }
            AdmissionDecision::Reject | AdmissionDecision::Block => {
                let _ = self.out_managers[o].free(token);
                if T::METRICS_ENABLED {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
                return EnqueueResult::Rejected;
            }
            AdmissionDecision::Admit => {}
        }

        match self.outputs[o].try_push(token, &self.out_policies[o], &*self.out_managers[o]) {
            EnqueueResult::Enqueued => {
                if T::METRICS_ENABLED {
                    self.telemetry.incr_counter(
                        TelemetryKey::node(self.node_id, TelemetryKind::EgressMsgs),
                        1,
                    );
                    let occ = self.outputs[o].occupancy(&self.out_policies[o]);
                    self.telemetry.set_gauge(
                        TelemetryKey::edge(self.out_edge_ids[o], TelemetryKind::QueueDepth),
                        *occ.items() as u64,
                    );
                }
                EnqueueResult::Enqueued
            }
            EnqueueResult::DroppedNewest | EnqueueResult::Rejected => {
                let _ = self.out_managers[o].free(token);
                if T::METRICS_ENABLED {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
                EnqueueResult::Rejected
            }
        }
    }

    /// Snapshot occupancy for the given output edge.
    #[inline]
    pub fn out_occupancy(&mut self, o: usize) -> EdgeOccupancy {
        debug_assert!(o < OUT);
        let occ = self.outputs[o].occupancy(&self.out_policies[o]);
        if T::METRICS_ENABLED {
            self.telemetry.set_gauge(
                TelemetryKey::edge(self.out_edge_ids[o], TelemetryKind::QueueDepth),
                *occ.items() as u64,
            );
        }
        occ
    }

    /// Return the policy for the given output (copy).
    #[inline]
    pub fn out_policy(&mut self, o: usize) -> EdgePolicy {
        debug_assert!(o < OUT);
        self.out_policies[o]
    }

    /// Access the platform clock.
    #[inline]
    pub fn clock(&self) -> &C {
        self.clock
    }

    /// Borrow the telemetry sink.
    #[inline]
    pub fn telemetry_mut(&mut self) -> &mut T {
        self.telemetry
    }

    /// Current monotonic tick value.
    #[inline]
    pub fn now_ticks(&self) -> Ticks {
        self.clock.now_ticks()
    }

    /// Current time in nanoseconds.
    #[inline]
    pub fn now_nanos(&self) -> u64 {
        self.clock.ticks_to_nanos(self.clock.now_ticks())
    }

    /// Convert clock ticks to nanoseconds.
    #[inline]
    pub fn ticks_to_nanos(&self, t: Ticks) -> u64 {
        self.clock.ticks_to_nanos(t)
    }

    /// Convert nanoseconds to clock ticks.
    #[inline]
    pub fn nanos_to_ticks(&self, ns: u64) -> Ticks {
        self.clock.nanos_to_ticks(ns)
    }
}

/// The uniform node contract.
///
/// Nodes are parameterized by:
/// - `IN`: number of input ports; `OUT`: number of output ports;
/// - `InP`: input payload type; `OutP`: output payload type.
///
/// Queue and manager types are introduced on each method via `where` clauses
/// rather than on the trait itself, keeping the trait payload-focused and
/// avoiding an explosion of type parameters on the `impl`.
pub trait Node<const IN: usize, const OUT: usize, InP, OutP>
where
    InP: Payload,
    OutP: Payload,
{
    /// Return the node's capability descriptor.
    fn describe_capabilities(&self) -> NodeCapabilities;

    /// Return the node's port placement acceptances (zero-copy compatibility).
    fn input_acceptance(&self) -> [PlacementAcceptance; IN];

    /// Return the node's output placement preferences (zero-copy compatibility).
    fn output_acceptance(&self) -> [PlacementAcceptance; OUT];

    /// Return the node's policy bundle.
    fn policy(&self) -> NodePolicy;

    /// **TEST ONLY** method used to override batching policies for node contract tests.
    #[cfg(any(test, feature = "bench"))]
    fn set_policy(&mut self, policy: NodePolicy);

    /// Return the type of node (Model, processing, source, sink).
    fn node_kind(&self) -> NodeKind;

    /// Prepare internal state, acquire buffers, and register telemetry series.
    fn initialize<C, Tel>(&mut self, clock: &C, telemetry: &mut Tel) -> Result<(), NodeError>
    where
        Tel: Telemetry;

    /// Optional warm-up (e.g., compile kernels, prime pools). Default: no-op.
    fn start<C, Tel>(&mut self, _clock: &C, _telemetry: &mut Tel) -> Result<(), NodeError>
    where
        Tel: Telemetry;

    /// Per-message processing hook.
    ///
    /// Receives a shared reference to the input message and returns a
    /// `ProcessResult` indicating what output (if any) was produced.
    /// The framework handles pushing outputs to edges; the node never
    /// interacts with queues or managers directly.
    fn process_message<C>(
        &mut self,
        msg: &Message<InP>,
        sys_clock: &C,
    ) -> Result<ProcessResult<OutP>, NodeError>
    where
        C: PlatformClock + Sized;

    /// Execute one cooperative step using the provided context.
    ///
    /// The default implementation:
    /// 1. Finds a ready input port via `input_edge_has_batch`.
    /// 2. Pops a single message via `pop_and_process`.
    /// 3. Delegates to `process_message` inside the callback.
    fn step<'graph, 'telemetry, 'clock, InQ, OutQ, InM, OutM, C, Tel>(
        &mut self,
        ctx: &mut StepContext<
            'graph,
            'telemetry,
            'clock,
            IN,
            OUT,
            InP,
            OutP,
            InQ,
            OutQ,
            InM,
            OutM,
            C,
            Tel,
        >,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge,
        OutQ: Edge,
        InM: MemoryManager<InP>,
        OutM: MemoryManager<OutP>,
        C: PlatformClock + Sized,
        Tel: Telemetry + Sized,
    {
        let node_policy = self.policy();
        let port = match (0..IN).find(|&p| ctx.input_edge_has_batch(p, &node_policy)) {
            Some(p) => p,
            None => return Ok(StepResult::NoInput),
        };

        ctx.pop_and_process(port, |msg| self.process_message(msg, ctx.clock))
    }

    /// Default batched-step implementation that honors all NodePolicy batching
    /// variants while delegating actual consumption to the implementor's
    /// single-message `process_message()` method.
    fn step_batch<'graph, 'telemetry, 'clock, InQ, OutQ, InM, OutM, C, Tel>(
        &mut self,
        ctx: &mut StepContext<
            'graph,
            'telemetry,
            'clock,
            IN,
            OUT,
            InP,
            OutP,
            InQ,
            OutQ,
            InM,
            OutM,
            C,
            Tel,
        >,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge,
        OutQ: Edge,
        InM: MemoryManager<InP>,
        OutM: MemoryManager<OutP>,
        C: PlatformClock + Sized,
        Tel: Telemetry + Sized,
    {
        let node_policy = self.policy();
        let port = match (0..IN).find(|&p| ctx.input_edge_has_batch(p, &node_policy)) {
            Some(p) => p,
            None => return Ok(StepResult::NoInput),
        };
        let nmax = node_policy.batching().fixed_n().unwrap_or(1);

        ctx.pop_batch_and_process(port, nmax, &node_policy, |msg| {
            self.process_message(msg, ctx.clock)
        })
    }

    /// Handle watchdog timeouts by applying over-budget policy (degrade/default/skip).
    fn on_watchdog_timeout<C, Tel>(
        &mut self,
        _clock: &C,
        _telemetry: &mut Tel,
    ) -> Result<StepResult, NodeError>
    where
        C: PlatformClock + Sized,
        Tel: Telemetry;

    /// Flush and release resources, if any. Default: no-op.
    fn stop<C, Tel>(&mut self, _clock: &C, _telemetry: &mut Tel) -> Result<(), NodeError>
    where
        Tel: Telemetry;
}
