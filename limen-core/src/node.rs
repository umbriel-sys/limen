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

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::{NodeError, QueueError};
use crate::memory::PlacementAcceptance;
use crate::message::{payload::Payload, Message};
use crate::policy::{BatchingPolicy, EdgePolicy, NodePolicy, SlidingWindow, WindowKind};
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

    /// Pop one message from input `port`, call `f` with a shared reference
    /// to it and an output context, then free the manager slot after `f` returns.
    ///
    /// Maps queue errors to `StepResult`/`NodeError`:
    /// - `Empty` → `Ok(StepResult::NoInput)`
    /// - `Backpressured`/`HardCap` → `Err(NodeError::backpressured())`
    /// - `Poisoned`/`Unsupported` → `Err(NodeError::execution_failed())`
    #[inline]
    pub fn pop_and_process<F>(&mut self, port: usize, f: F) -> Result<StepResult, NodeError>
    where
        F: FnOnce(
            &Message<InP>,
            &mut OutStepContext<'graph, '_, 'clock, OUT, OutP, OutQ, OutM, C, T>,
        ) -> Result<StepResult, NodeError>,
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

        // Disjoint field split — output fields + telemetry.
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

        let result = f(&*guard, &mut out)?;

        drop(guard);
        let _ = self.in_managers[port].free(token);

        if T::METRICS_ENABLED {
            self.telemetry.incr_counter(
                TelemetryKey::node(self.node_id, TelemetryKind::IngressMsgs),
                1,
            );
            let _ = self.in_occupancy(port);
        }

        Ok(result)
    }

    /// Pop a batch from input `port`. Set batch flags on popped tokens.
    /// Call `f` with a lazy message iterator and an output context.
    /// After `f` returns, free all popped tokens (first `stride` items).
    ///
    /// The callback receives a `BatchMessageIter` that yields `ReadGuard`s
    /// lazily — no upfront copy. Nodes can iterate one-at-a-time or collect
    /// into a scratch buffer for batch inference.
    ///
    /// For sliding-window batches, only the first `stride` tokens are freed;
    /// the rest were peeked and remain in the edge.
    #[inline]
    pub fn pop_batch_and_process<F>(
        &mut self,
        port: usize,
        nmax: usize,
        node_policy: &NodePolicy,
        f: F,
    ) -> Result<StepResult, NodeError>
    where
        F: FnOnce(
            BatchMessageIter<'_, '_, InP, InM>,
            &mut OutStepContext<'graph, '_, 'clock, OUT, OutP, OutQ, OutM, C, T>,
        ) -> Result<StepResult, NodeError>,
    {
        debug_assert!(port < IN);

        if nmax == 0 {
            return Err(NodeError::execution_failed());
        }

        // Build clamped batching policy from node policy.
        let requested_policy = {
            let nb = *node_policy.batching();
            BatchingPolicy::with_window(
                nb.fixed_n().map(|f| core::cmp::min(f, nmax)),
                *nb.max_delta_t(),
                match nb.window_kind() {
                    WindowKind::Disjoint => WindowKind::Disjoint,
                    WindowKind::Sliding(sw) => {
                        let size = nb.fixed_n().map(|f| core::cmp::min(f, nmax)).unwrap_or(1);
                        let stride = core::cmp::min(*sw.stride(), size);
                        WindowKind::Sliding(SlidingWindow::new(stride))
                    }
                },
            )
        };

        // Determine stride for free decisions.
        let stride = match requested_policy.window_kind() {
            WindowKind::Disjoint => usize::MAX, // free all
            WindowKind::Sliding(sw) => *sw.stride(),
        };

        // Sample pre-pop occupancy before the batch borrows inputs[port].
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

        // Disjoint field split.
        let in_mgr: &mut InM = &mut *self.in_managers[port];

        // Phase 1: set batch boundary flags on popped tokens (WriteGuard, short-lived).
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
                // WriteGuard drops here — mutable borrow released per iteration.
            }
        }

        // Phase 2: build iterator (shared ReadGuards) + OutStepContext, call callback.
        // All WriteGuards are dropped, so shared borrows are safe.
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

        let result = f(iter, &mut out)?;

        // Phase 3: free popped tokens (iterator + guards dropped, mutable borrow available).
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

        Ok(result)
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
    #[inline]
    pub fn out_try_push(&mut self, o: usize, m: Message<OutP>) -> EnqueueResult {
        debug_assert!(o < OUT);

        // Store message in output manager → get token
        let token = match self.out_managers[o].store(m) {
            Ok(t) => t,
            Err(_) => return EnqueueResult::Rejected, // No free slots
        };

        // Push token to output edge
        let res = self.outputs[o].try_push(token, &self.out_policies[o], &*self.out_managers[o]);

        match res {
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
            EnqueueResult::DroppedNewest => {
                // New message was dropped — free its token from manager
                let _ = self.out_managers[o].free(token);
                if T::METRICS_ENABLED {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
                EnqueueResult::DroppedNewest
            }
            EnqueueResult::Rejected => {
                let _ = self.out_managers[o].free(token);
                if T::METRICS_ENABLED {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
                EnqueueResult::Rejected
            }
            EnqueueResult::Evicted(evicted_token) => {
                // An older message was evicted — free the evicted token
                let _ = self.out_managers[o].free(evicted_token);
                if T::METRICS_ENABLED {
                    self.telemetry.incr_counter(
                        TelemetryKey::node(self.node_id, TelemetryKind::EgressMsgs),
                        1,
                    );
                    let _ = self.out_occupancy(o);
                }
                // The new message was enqueued (eviction made room)
                EnqueueResult::Enqueued
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
    #[inline]
    fn to_out_step_context<'ctx>(
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
    #[inline]
    pub fn out_try_push(&mut self, o: usize, m: Message<OutP>) -> EnqueueResult {
        debug_assert!(o < OUT);

        let token = match self.out_managers[o].store(m) {
            Ok(t) => t,
            Err(_) => return EnqueueResult::Rejected,
        };

        let res = self.outputs[o].try_push(token, &self.out_policies[o], &*self.out_managers[o]);

        match res {
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
            EnqueueResult::DroppedNewest => {
                let _ = self.out_managers[o].free(token);
                if T::METRICS_ENABLED {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
                EnqueueResult::DroppedNewest
            }
            EnqueueResult::Rejected => {
                let _ = self.out_managers[o].free(token);
                if T::METRICS_ENABLED {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
                EnqueueResult::Rejected
            }
            EnqueueResult::Evicted(evicted_token) => {
                let _ = self.out_managers[o].free(evicted_token);
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
    /// Note: we intentionally use an *anonymous* borrow `'_` for the second
    /// lifetime parameter of OutStepContext. This ensures each call to
    /// `process_message(..., &mut out)` creates a *fresh* short-lived mutable
    /// borrow of `out`, allowing repeated re-borrows inside a loop over a
    /// batch.
    fn process_message<'graph, 'clock, OutQ, OutM, C, Tel>(
        &mut self,
        msg: &Message<InP>,
        out_ctx: &mut OutStepContext<'graph, '_, 'clock, OUT, OutP, OutQ, OutM, C, Tel>,
    ) -> Result<StepResult, NodeError>
    where
        OutQ: Edge,
        OutM: MemoryManager<OutP>,
        C: PlatformClock + Sized,
        Tel: Telemetry + Sized;

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

        ctx.pop_and_process(port, |msg, out| self.process_message(msg, out))
    }

    /// Default batched-step implementation that honors all NodePolicy batching
    /// variants while delegating actual consumption to the implementor's
    /// single-message `process_message()` method.
    ///
    /// The callback receives a `BatchMessageIter` and iterates each message,
    /// calling `process_message` per item. Nodes that need all inputs
    /// simultaneously (e.g. `InferenceModel`) should override this method
    /// and collect from the iterator into a scratch buffer.
    ///
    /// Batch boundary flags (`first_in_batch`, `last_in_batch`) are set on
    /// popped tokens before the callback is invoked.
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

        ctx.pop_batch_and_process(port, nmax, &node_policy, |iter, out| {
            let mut any_made = false;
            for guard in iter {
                match self.process_message(&*guard, out)? {
                    StepResult::MadeProgress => any_made = true,
                    StepResult::NoInput => {}
                    StepResult::Backpressured => return Ok(StepResult::Backpressured),
                    StepResult::WaitingOnExternal => {
                        return Ok(StepResult::WaitingOnExternal);
                    }
                    StepResult::YieldUntil(t) => return Ok(StepResult::YieldUntil(t)),
                    StepResult::Terminal => return Ok(StepResult::Terminal),
                }
            }
            if any_made {
                Ok(StepResult::MadeProgress)
            } else {
                Ok(StepResult::NoInput)
            }
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

#[cfg(any(test, feature = "bench"))]
pub mod contract_tests {
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
    fn make_edge_links_for_node<const IN: usize, const OUT: usize, InP, OutP>(
        base_upstream_node: NodeIndex,
        base_downstream_node: NodeIndex,
    ) -> (
        [EdgeLink<TestSpscRingBuf<16>>; IN],
        [EdgeLink<TestSpscRingBuf<16>>; OUT],
    )
    where
        InP: Payload + Default,
        OutP: Payload + Default,
    {
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

        let inputs_ref: [&mut EdgeLink<TestSpscRingBuf<16>>; IN] = match inputs_ref_vec.into_array()
        {
            Ok(arr) => arr,
            Err(_) => panic!("inputs_ref_vec length mismatch"),
        };

        let outputs_ref: [&mut EdgeLink<TestSpscRingBuf<16>>; OUT] =
            match outputs_ref_vec.into_array() {
                Ok(arr) => arr,
                Err(_) => panic!("outputs_ref_vec length mismatch"),
            };

        let in_mgrs_ref: [&mut StaticMemoryManager<InP, 16>; IN] =
            match in_mgrs_ref_vec.into_array() {
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
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));
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
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));
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
                res == crate::node::StepResult::NoInput
                    || res == crate::node::StepResult::MadeProgress,
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
    pub fn run_step_pops_and_calls_process_message<
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
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));
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
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));
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
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));
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
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));
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
                crate::edge::EnqueueResult::DroppedNewest
                | crate::edge::EnqueueResult::Rejected => break,
                _ => break,
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
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));
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
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));
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
                    r == crate::node::StepResult::MadeProgress
                        || r == crate::node::StepResult::NoInput,
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
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));
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
    pub fn run_step_batch_fixed_n_max_delta_t_tests<
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
            let (mut in_links, mut out_links) = make_edge_links_for_node::<IN, OUT, InP, OutP>(
                NodeIndex::new(0),
                NodeIndex::new(1),
            );
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
            let (mut in_links, mut out_links) = make_edge_links_for_node::<IN, OUT, InP, OutP>(
                NodeIndex::new(0),
                NodeIndex::new(1),
            );
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
}
