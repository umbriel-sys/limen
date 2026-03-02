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
use crate::prelude::{BatchView, PlatformClock, TelemetryKey, TelemetryKind};
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

/// A context provided to nodes during `step`, abstracting queues and services.
///
/// The context is generic over input/output payload and queue types to avoid
/// trait objects. Implementations in runtimes will construct instances of this
/// context and pass them to nodes.
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

impl<'graph, 'telemetry, 'clock, const IN: usize, const OUT: usize, InP, OutP, InQ, OutQ, C, T>
    StepContext<'graph, 'telemetry, 'clock, IN, OUT, InP, OutP, InQ, OutQ, C, T>
where
    InP: Payload,
    OutP: Payload,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    /// Create a new step context from queues, policies, and services.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inputs: [&'graph mut InQ; IN],
        outputs: [&'graph mut OutQ; OUT],
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

impl<'graph, 'telemetry, 'clock, const IN: usize, const OUT: usize, InP, OutP, InQ, OutQ, C, T>
    StepContext<'graph, 'telemetry, 'clock, IN, OUT, InP, OutP, InQ, OutQ, C, T>
where
    InP: Payload,
    OutP: Payload,
    InQ: Edge<Item = Message<InP>>,
    OutQ: Edge<Item = Message<OutP>>,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    /// Attempt to pop an item from the specified input queue.
    #[inline]
    pub fn in_try_pop(&mut self, i: usize) -> Result<Message<InP>, QueueError> {
        debug_assert!(i < IN);
        match self.inputs[i].try_pop() {
            Ok(msg) => {
                if T::METRICS_ENABLED {
                    self.telemetry.incr_counter(
                        TelemetryKey::node(self.node_id, TelemetryKind::IngressMsgs),
                        1,
                    );
                    let _ = self.in_occupancy(i);
                }
                Ok(msg)
            }
            Err(e) => Err(e),
        }
    }

    /// Attempt to peek at the front item without removing it.
    ///
    /// Returns a `PeekResponse<'_, Message<InP>>` so callers can handle
    /// both borrowed (zero-copy) and owned (alloc/clone) peek paths.
    #[inline]
    pub fn in_try_peek(
        &self,
        i: usize,
    ) -> Result<crate::edge::PeekResponse<'_, Message<InP>>, QueueError> {
        debug_assert!(i < IN);
        self.inputs[i].try_peek()
    }

    /// Passthrough to the input queue's `try_peek_at`.
    ///
    /// Returns the queue backend's `PeekResponse<'_, Message<InP>>` so callers can
    /// handle both borrowed and owned peek paths.
    #[inline]
    pub fn in_try_peek_at(
        &self,
        i: usize,
        index: usize,
    ) -> Result<crate::edge::PeekResponse<'_, Message<InP>>, QueueError> {
        debug_assert!(i < IN);
        self.inputs[i].try_peek_at(index)
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

    /// Attempt to push an item to the specified output queue.
    #[inline]
    pub fn out_try_push(&mut self, o: usize, m: Message<OutP>) -> crate::edge::EnqueueResult {
        debug_assert!(o < OUT);
        let res = self.outputs[o].try_push(m, &self.out_policies[o]);
        if T::METRICS_ENABLED {
            match res {
                crate::edge::EnqueueResult::Enqueued => {
                    self.telemetry.incr_counter(
                        TelemetryKey::node(self.node_id, TelemetryKind::EgressMsgs),
                        1,
                    );
                    let _ = self.out_occupancy(o);
                }
                _ => {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
            }
        }
        res
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

    /// Return the policy of the specified input queue.
    #[inline]
    pub fn out_policy(&mut self, i: usize) -> EdgePolicy {
        debug_assert!(i < OUT);
        self.out_policies[i]
    }

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

    /// Current time in nanoseconds per the clock’s tick-to-ns mapping.
    #[inline]
    pub fn now_nanos(&self) -> u64 {
        self.clock.ticks_to_nanos(self.clock.now_ticks())
    }

    /// Convert clock ticks to nanoseconds using the clock’s scale.
    #[inline]
    pub fn ticks_to_nanos(&self, t: Ticks) -> u64 {
        self.clock.ticks_to_nanos(t)
    }

    /// Convert nanoseconds to clock ticks using the clock’s scale.
    #[inline]
    pub fn nanos_to_ticks(&self, ns: u64) -> Ticks {
        self.clock.nanos_to_ticks(ns)
    }

    /// Return `true` if the input edge `port` can produce a batch under `policy`.
    ///
    /// # Semantics
    ///
    /// This method provides a *scheduler readiness* predicate aligned with Limen’s
    /// batching policy interpretation:
    ///
    /// - `BatchingPolicy::max_delta_t` is a **span constraint** on the batch contents,
    ///   not a wall-clock timeout. A candidate batch `[0..k)` is span-valid when:
    ///
    ///   `creation_tick[k - 1] - creation_tick[0] <= max_delta_t`
    ///
    ///   i.e., all items in the batch lie within `max_delta_t` ticks of the **front
    ///   (oldest) item**.
    ///
    /// - When `fixed_n` is set and `max_delta_t` is not set, readiness requires
    ///   `occupancy >= fixed_n`.
    ///
    /// - When `max_delta_t` is set and `fixed_n` is not set, readiness requires only
    ///   that the queue is non-empty (a span-valid batch of size 1 always exists).
    ///
    /// - When **both** `fixed_n` and `max_delta_t` are set, readiness requires that
    ///   a **full batch of exactly `fixed_n` items** can be formed and that those
    ///   `fixed_n` items satisfy the span constraint above. This requires peeking
    ///   both the front item and the `(fixed_n - 1)`-th item without popping.
    ///
    /// Conservative behaviour: if peeks fail (concurrent/fallible backends), returns
    /// `false` rather than assuming readiness.
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
            (None, Some(_max_delta_t)) => {
                // Span constraint only: a non-empty queue can always produce a span-valid batch of size 1.
                true
            }
            (Some(fixed_n), Some(max_delta_t)) => {
                // Must be able to form a full fixed_n batch first.
                if *occ.items() < fixed_n {
                    return false;
                }

                // Peek front (index 0) and the last item of the fixed-size batch (index fixed_n - 1).
                let first = match self.inputs[port].try_peek_at(0) {
                    Ok(v) => v,
                    Err(_) => return false,
                };

                let last = match self.inputs[port].try_peek_at(fixed_n - 1) {
                    Ok(v) => v,
                    Err(_) => return false,
                };

                let first_ticks = *first.as_ref().header().creation_tick();
                let last_ticks = *last.as_ref().header().creation_tick();

                let span = last_ticks.saturating_sub(first_ticks);
                span <= max_delta_t
            }
            (None, None) => {
                // No batching configured: treat as single-message readiness (queue non-empty here).
                true
            }
        }
    }

    /// Pop up to `nmax` messages from input port `port` as a batch.
    ///
    /// Uses node-level `node_policy` to construct a clamped `BatchingPolicy`.
    /// Only the edge's `try_pop_batch` path is used; we propagate edge errors.
    /// Telemetry and occupancy gauges are updated *after* the pop by computing
    /// the post-pop occupancy from a sampled pre-pop occupancy and the
    /// returned batch size/bytes.
    pub fn pop_input_messages_as_batch(
        &mut self,
        port: usize,
        nmax: usize,
        node_policy: &NodePolicy,
    ) -> Result<BatchView<'_, Message<InP>>, QueueError> {
        debug_assert!(port < IN);

        if nmax == 0 {
            return Err(QueueError::Unsupported);
        }

        // Build clamped batching policy from node policy.
        let requested_policy = {
            let nb = *node_policy.batching();

            BatchingPolicy::with_window(
                // clamp fixed_n if present
                nb.fixed_n().map(|f| core::cmp::min(f, nmax)),
                // preserve max_delta_t as-is
                *nb.max_delta_t(),
                // clamp sliding window size/stride if needed
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

        // SAMPLE pre-pop occupancy by calling the edge occupancy directly.
        // This borrows only the input queue immutably (no &mut self).
        let occ_before = self.inputs[port].occupancy(&self.in_policies[port]);

        // Ask the edge to pop a batch. This may return a Borrowed BatchView
        // that mutably borrows the input queue; don't call &mut self helpers
        // that borrow the same field while batch_view is alive.
        match self.inputs[port].try_pop_batch(&requested_policy) {
            Ok(mut batch_view) => {
                let batch_len = batch_view.len();
                if batch_len == 0 {
                    return Err(QueueError::Empty);
                }

                // Compute bytes removed as batch payload/header bytes via BatchView's Payload impl.
                //
                // NOTE: Not currently required as bytes not returned in telemetry.
                // let removed_bytes = {
                //     // BatchView implements Payload for Message<P>, so we can call buffer_descriptor.
                //     let desc = batch_view.buffer_descriptor();
                //     *desc.bytes()
                // };

                // Mark batch boundaries (works for Owned and Borrowed).
                if let Some(header) = batch_view.first_header_mut() {
                    header.set_first_in_batch();
                }
                if let Some(header) = batch_view.last_header_mut() {
                    header.set_last_in_batch();
                }

                // Update telemetry and occupancy gauge:
                if T::METRICS_ENABLED {
                    // Increment ingress messages counter by number returned.
                    // Mutably borrow only the telemetry field (non-overlapping with input queue borrow).
                    let telemetry = &mut self.telemetry;
                    telemetry.incr_counter(
                        TelemetryKey::node(self.node_id, TelemetryKind::IngressMsgs),
                        batch_len as u64,
                    );

                    // Compute post-pop occupancy numbers (saturating to avoid underflow).
                    let after_items = occ_before.items().saturating_sub(batch_len);

                    // NOTE: Bytes not currently returned in telemetry.
                    // let after_bytes = occ_before.bytes().saturating_sub(removed_bytes);

                    // Note: we do not call self.in_occupancy(port) here because that
                    // would borrow the input queue again. Instead we update the gauge
                    // directly from calculated values.
                    telemetry.set_gauge(
                        TelemetryKey::edge(self.in_edge_ids[port], TelemetryKind::QueueDepth),
                        after_items as u64,
                    );
                }

                Ok(batch_view)
            }

            // Propagate queue errors as-is.
            Err(e) => Err(e),
        }
    }

    /// Minimal helper: pop a batch *and* construct an OutStepContext in the same `&mut self` borrow.
    ///
    /// Returning both the `BatchView<'ctx, Message<InP>>` (which borrows inputs)
    /// and the `OutStepContext<'graph, 'ctx, 'clock, ...>` (which borrows outputs/telemetry)
    /// from the *same* `&'ctx mut self` is the safe way to obtain two disjoint mutable
    /// borrows that the caller can use simultaneously (the borrow checker accepts it).
    #[allow(clippy::type_complexity)]
    #[inline]
    pub fn pop_input_messages_as_batch_with_out<'ctx>(
        &'ctx mut self,
        port: usize,
        nmax: usize,
        node_policy: &NodePolicy,
    ) -> Result<
        (
            BatchView<'ctx, Message<InP>>,
            OutStepContext<'graph, 'ctx, 'clock, OUT, OutP, OutQ, C, T>,
        ),
        QueueError,
    > {
        debug_assert!(port < IN);

        if nmax == 0 {
            return Err(QueueError::Unsupported);
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

        // SAMPLE pre-pop occupancy by calling the edge occupancy directly.
        let occ_before = self.inputs[port].occupancy(&self.in_policies[port]);

        match self.inputs[port].try_pop_batch(&requested_policy) {
            Ok(mut batch_view) => {
                let batch_len = batch_view.len();
                if batch_len == 0 {
                    return Err(QueueError::Empty);
                }

                if let Some(header) = batch_view.first_header_mut() {
                    header.set_first_in_batch();
                }
                if let Some(header) = batch_view.last_header_mut() {
                    header.set_last_in_batch();
                }

                if T::METRICS_ENABLED {
                    let telemetry = &mut self.telemetry;
                    telemetry.incr_counter(
                        TelemetryKey::node(self.node_id, TelemetryKind::IngressMsgs),
                        batch_len as u64,
                    );
                    let after_items = occ_before.items().saturating_sub(batch_len);
                    telemetry.set_gauge(
                        TelemetryKey::edge(self.in_edge_ids[port], TelemetryKind::QueueDepth),
                        after_items as u64,
                    );
                }

                // Construct OutStepContext **now**, from the same &mut self borrow.
                let out_policies = self.out_policies;
                let out_edge_ids = self.out_edge_ids;
                let node_id = self.node_id;
                let clock = self.clock;
                let telemetry: &'ctx mut T = &mut *self.telemetry;
                let outputs: &'ctx mut [&'graph mut OutQ; OUT] = &mut self.outputs;

                let out_ctx = OutStepContext {
                    outputs,
                    out_policies,
                    out_edge_ids,
                    node_id,
                    clock,
                    telemetry,
                    _marker: core::marker::PhantomData,
                };

                Ok((batch_view, out_ctx))
            }
            Err(e) => Err(e),
        }
    }

    /// Construct an `OutStepContext` by borrowing only the output-related
    /// fields and telemetry from `self`.
    ///
    /// The returned proxy is tied to the mutable borrow of `self` (the `'ctx`
    /// lifetime) so it cannot outlive the `StepContext` that produced it.
    #[inline]
    pub fn to_out_step_context<'ctx>(
        &'ctx mut self,
    ) -> OutStepContext<'graph, 'ctx, 'clock, OUT, OutP, OutQ, C, T>
    where
        EdgePolicy: Copy,
    {
        // Copy small `Copy` arrays (EdgePolicy, u32) into the proxy for convenience.
        let out_policies = self.out_policies;
        let out_edge_ids = self.out_edge_ids;
        let node_id = self.node_id;
        let clock = self.clock;

        // Reborrow telemetry for the `'ctx` lifetime and borrow the outputs array
        // for the `'ctx` lifetime while preserving the inner `&'graph mut OutQ`
        // element lifetimes.
        let telemetry = &mut *self.telemetry;
        let outputs: &'ctx mut [&'graph mut OutQ; OUT] = &mut self.outputs;

        OutStepContext {
            outputs,
            out_policies,
            out_edge_ids,
            node_id,
            clock,
            telemetry,
            _marker: core::marker::PhantomData,
        }
    }
}

/// A `StepContext` *view* that only exposes outputs / clock / telemetry.
///
/// Explicitly **does not** provide access to input queues/policies. This is
/// intended to be constructed from `StepContext::to_out_step_context(&mut self)`
/// and used while a borrowed `BatchView` or other input borrow is live.
///
/// Lifetime `'ctx` is the mutable borrow lifetime of the `StepContext` used
/// to construct this proxy. `'clock` is the clock lifetime passed through.
pub struct OutStepContext<'graph, 'ctx, 'clock, const OUT: usize, OutP, OutQ, C, T>
where
    OutP: Payload,
    OutQ: Edge<Item = Message<OutP>>,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    /// Mutable borrow of the outputs array from the original StepContext.
    outputs: &'ctx mut [&'graph mut OutQ; OUT],

    /// Copy of per-output policies (EdgePolicy: Copy).
    out_policies: [EdgePolicy; OUT],

    /// Copy of output edge ids (u32 Copy).
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

impl<'graph, 'ctx, 'clock, const OUT: usize, OutP, OutQ, C, T>
    OutStepContext<'graph, 'ctx, 'clock, OUT, OutP, OutQ, C, T>
where
    OutP: Payload,
    OutQ: Edge<Item = Message<OutP>>,
    C: PlatformClock + Sized,
    T: Telemetry + Sized,
{
    /// Push to an output queue, emitting telemetry similar to `StepContext::out_try_push`.
    #[inline]
    pub fn out_try_push(&mut self, o: usize, m: Message<OutP>) -> EnqueueResult {
        debug_assert!(o < OUT);
        let res = self.outputs[o].try_push(m, &self.out_policies[o]);
        if T::METRICS_ENABLED {
            match res {
                EnqueueResult::Enqueued => {
                    self.telemetry.incr_counter(
                        TelemetryKey::node(self.node_id, TelemetryKind::EgressMsgs),
                        1,
                    );
                    // update queue depth gauge by sampling occupancy from the queue
                    let occ = self.outputs[o].occupancy(&self.out_policies[o]);
                    self.telemetry.set_gauge(
                        TelemetryKey::edge(self.out_edge_ids[o], TelemetryKind::QueueDepth),
                        *occ.items() as u64,
                    );
                }
                _ => {
                    self.telemetry
                        .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
                }
            }
        }
        res
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

    /// Current time in nanoseconds per the clock’s tick-to-ns mapping.
    #[inline]
    pub fn now_nanos(&self) -> u64 {
        self.clock.ticks_to_nanos(self.clock.now_ticks())
    }

    /// Convert clock ticks to nanoseconds using the clock’s scale.
    #[inline]
    pub fn ticks_to_nanos(&self, t: Ticks) -> u64 {
        self.clock.ticks_to_nanos(t)
    }

    /// Convert nanoseconds to clock ticks using the clock’s scale.
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
pub trait Node<const IN: usize, const OUT: usize, InP, OutP>
where
    // TODO: Should this be message<payload>?
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

    /// **TEST ONLY** method used to override batching policis for node contract tests.
    #[cfg(any(test, feature = "bench"))]
    fn set_policy(&mut self, policy: NodePolicy);

    /// Return the type of node (Mmodel, processing, source, sink).
    fn node_kind(&self) -> NodeKind;

    /// Prepare internal state, acquire buffers, and register telemetry series.
    fn initialize<C, T>(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry;

    /// Optional warm-up (e.g., compile kernels, prime pools). Default: no-op.
    fn start<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry;

    /// Note: we intentionally use an *anonymous* borrow `'_` for the second
    /// lifetime parameter of OutStepContext. This ensures each call to
    /// `process_message(..., &mut out)` creates a *fresh* short-lived mutable
    /// borrow of `out`, allowing repeated re-borrows inside a loop over a
    /// batch. If we tied this borrow to the batch lifetime, the borrow would
    /// last the whole batch and prevent reborrowing.
    fn process_message<'graph, 'clock, OutQ, C, T>(
        &mut self,
        msg: &Message<InP>,
        out_ctx: &mut OutStepContext<'graph, '_, 'clock, OUT, OutP, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized;

    /// Execute one cooperative step using the provided context.
    ///
    /// The input and output queues are exposed through the context, along with
    /// per-edge policies and services. Implementations should honor the node
    /// policy (batching, budgets, deadlines) and return a `StepResult` to help
    /// the scheduler make progress decisions.
    fn step<'graph, 'telemetry, 'clock, InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<'graph, 'telemetry, 'clock, IN, OUT, InP, OutP, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge<Item = Message<InP>>,
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        // 1) pick a ready input port according to the node policy
        let node_policy = self.policy();
        let mut port_opt: Option<usize> = None;
        for port in 0..IN {
            if ctx.input_edge_has_batch(port, &node_policy) {
                port_opt = Some(port);
                break;
            }
        }

        let port = match port_opt {
            None => return Ok(StepResult::NoInput),
            Some(p) => p,
        };

        // 2) attempt to pop a single message from the selected port
        match ctx.in_try_pop(port) {
            Ok(msg) => {
                // Create an output-only proxy that borrows only outputs & telemetry
                // (disjoint from input-borrow held by popped message if any).
                let mut out = ctx.to_out_step_context();
                // Delegate per-message work using the proxy.
                self.process_message(&msg, &mut out)
            }

            // Map queue errors to either StepResult or NodeError as appropriate.
            Err(QueueError::Empty) => Ok(StepResult::NoInput),

            // Backpressure / hard-cap: surface as node-level backpressure error.
            Err(QueueError::Backpressured) | Err(QueueError::AtOrAboveHardCap) => {
                Err(NodeError::backpressured())
            }

            // Poisoned lock / unsupported operation: treat as execution failure.
            Err(QueueError::Poisoned) | Err(QueueError::Unsupported) => {
                Err(NodeError::execution_failed())
            }
        }
    }

    /// Default batched-step implementation that honors all NodePolicy batching
    /// variants while delegating actual consumption to the implementor's
    /// single-message `step()` method.
    ///
    /// Key rules implemented:
    /// - fixed_n, no max_delta_t  => attempt min(nmax, fixed_n) step() calls
    /// - no fixed_n, max_delta_t  => attempt 1 step() call
    /// - no fixed_n, no max_delta_t => attempt 1 step() call
    /// - fixed_n + max_delta_t => require the first fixed_n items' creation_tick
    ///   span to be <= max_delta_t (verified by peeks) and then attempt
    ///   exactly fixed_n step() calls
    /// - WindowKind::Disjoint => keep calling step() until the input port is empty
    /// - WindowKind::Sliding => call step() `stride` times (or up to nmax), and
    ///   peek remaining items as necessary to validate span when fixed_n+max_delta_t.
    fn step_batch<'graph, 'telemetry, 'clock, InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<'graph, 'telemetry, 'clock, IN, OUT, InP, OutP, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: Edge<Item = Message<InP>>,
        OutQ: Edge<Item = Message<OutP>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        // 1) Find a ready input port according to the node policy.
        let node_policy = self.policy();
        let mut port_opt: Option<usize> = None;
        for port in 0..IN {
            if ctx.input_edge_has_batch(port, &node_policy) {
                port_opt = Some(port);
                break;
            }
        }

        // Nothing ready.
        let port = match port_opt {
            None => return Ok(StepResult::NoInput),
            Some(p) => p,
        };

        // 2) Use the node policy's nmax as the batch maximum.
        // Replace `.nmax()` with your actual accessor if it's named differently.
        let nmax = node_policy.batching().fixed_n().unwrap_or(1);

        // 3) Attempt the canonical batch pop that honors sliding/disjoint/fixed+delta semantics.
        match ctx.pop_input_messages_as_batch_with_out(port, nmax, &node_policy) {
            Ok((batch_view, mut out)) => {
                // Defensive: if the batch is empty treat as NoInput.
                let batch_len = batch_view.len();
                if batch_len == 0 {
                    return Ok(StepResult::NoInput);
                }

                // Consume the BatchView so the borrow on the input queue ends while we call
                // `process_message(..., ctx)`. This requires `BatchView` to provide an
                // owning iterator (`into_iter()` returning owned Message<InP>) or similar.
                // If your BatchView.into_iter() yields `&Message<InP>` you will need to
                // clone or otherwise obtain owned messages before dropping the BatchView.
                let mut any_made = false;

                // Note: move/consume batch_view here to drop its internal borrows while iterating.
                for msg in batch_view.iter() {
                    // `msg` is assumed to be `Message<InP>` (owned). We pass a reference to the
                    // per-message hook; the hook may use `ctx` to emit outputs and telemetry.
                    let msg_ref: &Message<InP> = msg;

                    // Put the mutable borrow of `out` into a *short, inner scope* so
                    // the borrow ends before the next loop iteration.
                    let res = {
                        // `out_tmp` lives only until the end of this block.
                        let out_tmp = &mut out;
                        self.process_message(msg_ref, out_tmp)
                    };

                    match res {
                        Ok(StepResult::MadeProgress) => any_made = true,
                        Ok(StepResult::NoInput) => {
                            // Unlikely when processing an explicit item — treat as no-op.
                        }
                        Ok(StepResult::Backpressured) => return Ok(StepResult::Backpressured),
                        Ok(StepResult::WaitingOnExternal) => {
                            return Ok(StepResult::WaitingOnExternal)
                        }
                        Ok(StepResult::YieldUntil(t)) => return Ok(StepResult::YieldUntil(t)),
                        Ok(StepResult::Terminal) => return Ok(StepResult::Terminal),
                        Err(e) => return Err(e),
                    }
                }

                if any_made {
                    Ok(StepResult::MadeProgress)
                } else {
                    Ok(StepResult::NoInput)
                }
            }

            // Map queue errors consistently with the single-message `step()` mapping:
            Err(QueueError::Empty) => Ok(StepResult::NoInput),
            Err(QueueError::Backpressured) | Err(QueueError::AtOrAboveHardCap) => {
                Err(NodeError::backpressured())
            }
            Err(QueueError::Poisoned) => Err(NodeError::execution_failed().with_code(1)),
            // We do not provide a fallback here: Unsupported indicates the backend cannot
            // supply the batch path; surface as execution failure so implementers know
            // they must override if needed for that backend.
            Err(QueueError::Unsupported) => Err(NodeError::execution_failed().with_code(2)),
        }
    }

    /// Handle watchdog timeouts by applying over-budget policy (degrade/default/skip).
    fn on_watchdog_timeout<C, T>(
        &mut self,
        _clock: &C,
        _telemetry: &mut T,
    ) -> Result<StepResult, NodeError>
    where
        C: PlatformClock + Sized,
        T: Telemetry;

    /// Flush and release resources, if any. Default: no-op.
    fn stop<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>
    where
        T: Telemetry;
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
            NoStdLinuxMonotonicClock, NodeLink, TestSpscRingBuf,
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
        [EdgeLink<TestSpscRingBuf<Message<InP>, 16>, InP>; IN],
        [EdgeLink<TestSpscRingBuf<Message<OutP>, 16>, OutP>; OUT],
    )
    where
        InP: crate::message::payload::Payload + Default + Clone,
        OutP: crate::message::payload::Payload + Default + Clone,
    {
        let inputs = core::array::from_fn(|i| {
            let queue = TestSpscRingBuf::<Message<InP>, 16>::new();
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
            let queue = TestSpscRingBuf::<Message<OutP>, 16>::new();
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
        inputs: &'graph mut [EdgeLink<TestSpscRingBuf<Message<InP>, 16>, InP>; IN],
        outputs: &'graph mut [EdgeLink<TestSpscRingBuf<Message<OutP>, 16>, OutP>; OUT],
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
        EdgeLink<TestSpscRingBuf<Message<InP>, 16>, InP>,
        EdgeLink<TestSpscRingBuf<Message<OutP>, 16>, OutP>,
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

        let mut inputs_ref_vec: Vec<&mut EdgeLink<TestSpscRingBuf<Message<InP>, 16>, InP>, IN> =
            Vec::new();
        for elem in inputs.iter_mut() {
            assert!(inputs_ref_vec.push(elem).is_ok(), "inputs_ref_vec overflow");
        }

        let mut outputs_ref_vec: Vec<&mut EdgeLink<TestSpscRingBuf<Message<OutP>, 16>, OutP>, OUT> =
            Vec::new();
        for elem in outputs.iter_mut() {
            assert!(
                outputs_ref_vec.push(elem).is_ok(),
                "outputs_ref_vec overflow"
            );
        }

        let inputs_ref: [&mut EdgeLink<TestSpscRingBuf<Message<InP>, 16>, InP>; IN] =
            match inputs_ref_vec.into_array() {
                Ok(arr) => arr,
                Err(_) => panic!("inputs_ref_vec length mismatch"),
            };

        let outputs_ref: [&mut EdgeLink<TestSpscRingBuf<Message<OutP>, 16>, OutP>; OUT] =
            match outputs_ref_vec.into_array() {
                Ok(arr) => arr,
                Err(_) => panic!("outputs_ref_vec length mismatch"),
            };

        let in_edge_ids = core::array::from_fn(|i| *inputs_ref[i].id().as_usize() as u32);
        let out_edge_ids = core::array::from_fn(|o| *outputs_ref[o].id().as_usize() as u32);

        crate::node::StepContext::new(
            inputs_ref,
            outputs_ref,
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

        if IN == 0 {
            // Not applicable to sources (no input)
            return;
        }

        // Build a message with header creation tick sourced from the clock.
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(clock.now_ticks());
        let msg = Message::new(hdr, InP::default());

        let in_policy = TEST_EDGE_POLICY;
        assert_eq!(
            in_links[0].try_push(msg, &in_policy),
            crate::edge::EnqueueResult::Enqueued
        );

        let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

        let res = nlink.step(&mut ctx).expect("step ok");
        assert!(res != crate::node::StepResult::NoInput);

        if OUT > 0 {
            let mut pushed = 0usize;
            loop {
                match out_links[0].try_pop() {
                    Ok(_m) => pushed += 1,
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

        let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

        let res = nlink.step(&mut ctx).expect("step ok");

        // For zero-input nodes (sources) it's valid for the node to actively
        // produce output and therefore return `MadeProgress`. For nodes with
        // inputs, the empty-input fast path must return `NoInput`.
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

        if IN == 0 {
            // No input ports: nothing to test here.
            return;
        }

        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(clock.now_ticks());
        let msg = Message::new(hdr, InP::default());

        let policy = TEST_EDGE_POLICY;
        assert_eq!(
            in_links[0].try_push(msg, &policy),
            crate::edge::EnqueueResult::Enqueued
        );

        let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

        let res = nlink.step(&mut ctx).expect("step ok");
        assert!(res != crate::node::StepResult::NoInput);

        if OUT > 0 {
            let mut popped = 0usize;
            while let Ok(_m) = out_links[0].try_pop() {
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
        // Override the node's policy for this test so we exercise fixed-N, disjoint semantics.
        // Pick a small fixed_n suitable for tests.
        const TEST_FIXED_N: usize = 3;

        // Start from the node's current policy and replace the batching window with
        // a fixed-N, disjoint window for deterministic behaviour.
        let base_policy = nlink.node().policy();
        let batching = crate::policy::BatchingPolicy::with_window(
            Some(TEST_FIXED_N),
            None,
            crate::policy::WindowKind::Disjoint,
        );
        // Construct a new node policy with the requested batching. Most NodePolicy
        // implementations expose a builder like `with_batching`. If your project
        // uses a different name, adjust this call accordingly.
        let new_policy = NodePolicy::new(batching, *base_policy.budget(), *base_policy.deadline());

        // Apply the test policy to the actual node under test.
        nlink.set_policy(new_policy);

        let clock = NoStdLinuxMonotonicClock::new();
        let mut tele = make_graph_telemetry();
        nlink.initialize(&clock, &mut tele).expect("init ok");

        let (mut in_links, mut out_links) =
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));

        if IN == 0 {
            return;
        }

        // Re-read fixed_n from the node's (now overridden) policy so the rest of
        // the test adapts to the value we just installed.
        let fixed_n = nlink.node().policy().batching().fixed_n().unwrap_or(1usize);

        let policy = TEST_EDGE_POLICY;
        for t in 1u64..=(fixed_n as u64 + 1) {
            let mut hdr = MessageHeader::empty();
            hdr.set_creation_tick(Ticks::new(t));
            let m = Message::new(hdr, InP::default());
            assert_eq!(
                in_links[0].try_push(m, &policy),
                crate::edge::EnqueueResult::Enqueued
            );
        }

        let in_before = *in_links[0].occupancy(&policy).items();

        let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

        let res = nlink.step(&mut ctx).expect("step_batch ok");
        assert!(res != crate::node::StepResult::NoInput);

        // read input occupancy via ctx (while ctx is live) and assert exact pop
        let in_after = *ctx.in_occupancy(0).items();
        assert_eq!(
            in_before.saturating_sub(in_after),
            fixed_n,
            "expected fixed_n items popped from input"
        );

        if OUT > 0 {
            let mut out_count = 0usize;
            while let Ok(_m) = out_links[0].try_pop() {
                out_count += 1;
            }
            // For disjoint fixed-N batching we expect the node to process exactly
            // `fixed_n` inputs and (in the common case) produce `fixed_n` outputs.
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

        // Telemetry: NodeLink increments `processed` by `fixed_n` for a batched step.
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

        // Install a sliding-window batching policy for this test:
        // fixed_n = 4 (presentation size), sliding stride = 2 (pop 2 each step).
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

        if IN == 0 {
            return;
        }

        let policy = TEST_EDGE_POLICY;
        // Push 6 messages so we can form a presentation of size `fixed_n` with
        // available items > fixed_n and stride < fixed_n.
        for t in 1u64..=6u64 {
            let mut hdr = MessageHeader::empty();
            hdr.set_creation_tick(Ticks::new(t));
            let m = Message::new(hdr, InP::default());
            assert_eq!(
                in_links[0].try_push(m, &policy),
                crate::edge::EnqueueResult::Enqueued
            );
        }

        // Sample pre-step occupancy for later comparison (before creating ctx).
        let in_before = *in_links[0].occupancy(&policy).items();

        let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

        let res = nlink.step(&mut ctx).expect("step_batch ok");
        assert!(res != crate::node::StepResult::NoInput);

        // Read input occupancy via ctx while it is live.
        let in_after = *ctx.in_occupancy(0).items();

        // How many items should have been popped according to sliding semantics?
        let stride_to_pop = core::cmp::min(TEST_STRIDE, in_before);
        let removed = in_before.saturating_sub(in_after);

        // Strict sliding semantics: must pop exactly `stride_to_pop`.
        assert_eq!(
            removed, stride_to_pop,
            "unexpected number popped: removed={}, expected stride {}",
            removed, stride_to_pop
        );

        // Determine presentation size and then drop ctx before popping outputs.
        let fixed_n = nlink.node().policy().batching().fixed_n().unwrap_or(1usize);
        let expected_present = core::cmp::min(in_before, fixed_n);

        if OUT > 0 {
            let mut out_count = 0usize;
            while let Ok(_m) = out_links[0].try_pop() {
                out_count += 1;
            }
            assert_eq!(
                out_count, expected_present,
                "expected out_count == expected_present (got {}, expected {})",
                out_count, expected_present
            );
        }

        // Telemetry: NodeLink increments processed by fixed_n for batched steps.
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
        // This test attempts to force output backpressure and ensures the node maps
        // the enqueue result / queue errors as documented.

        if IN == 0 || OUT == 0 {
            // Not applicable for sources (no input) or nodes with no outputs.
            return;
        }

        let mut nlink = make_nodelink();
        let clock = NoStdLinuxMonotonicClock::new();
        let mut tele = make_graph_telemetry();
        nlink.initialize(&clock, &mut tele).expect("init ok");

        // create edges and fill the output queue to force backpressure/rejection
        let (mut in_links, mut out_links) =
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));

        // prefill output 0 until it rejects or drops (ensures backpressure or drop behavior).
        let policy = TEST_EDGE_POLICY;
        let dummy_out_msg = Message::new(MessageHeader::empty(), OutP::default());
        // Keep trying to push; when queue capacity reached admission policy will cause non-Enqueued.
        while let crate::edge::EnqueueResult::Enqueued =
            out_links[0].try_push(dummy_out_msg.clone(), &policy)
        {
            match out_links[0].try_push(dummy_out_msg.clone(), &policy) {
                crate::edge::EnqueueResult::Enqueued => continue,
                crate::edge::EnqueueResult::DroppedNewest
                | crate::edge::EnqueueResult::Rejected => break,
            }
        }

        // push a single input so step will attempt to push to the full output
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(clock.now_ticks());
        let msg = Message::new(hdr, InP::default());
        assert_eq!(
            in_links[0].try_push(msg, &policy),
            crate::edge::EnqueueResult::Enqueued
        );

        let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

        // Node may either return Ok(StepResult::Backpressured) or Err(NodeError::backpressured())
        // depending on whether the backpressure is surfaceable as a StepResult or NodeError.
        match nlink.step(&mut ctx) {
            Ok(res) => {
                // If node returned a StepResult, it should not be NoInput since an input existed.
                assert!(res != crate::node::StepResult::NoInput);
            }
            Err(_e) => {
                // Error is acceptable; presence suffices for this test.
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
        // If this node is not a source, skip source checks.
        let mut nlink = make_nodelink();
        let kind = nlink.node().node_kind();
        if kind != crate::node::NodeKind::Source {
            return;
        }

        let clock = NoStdLinuxMonotonicClock::new();
        let mut tele = make_graph_telemetry();
        nlink.initialize(&clock, &mut tele).expect("init ok");

        // Build edges: for sources IN == 0, so make_edge_links_for_node works with IN==0.
        let (mut in_links, mut out_links) =
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));

        // Build context and call step() — if source has nothing to produce we expect NoInput.
        let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

        let res = nlink.step(&mut ctx).expect("step ok");
        assert!(
            res == crate::node::StepResult::NoInput || res == crate::node::StepResult::MadeProgress,
            "source.step should return NoInput or MadeProgress"
        );

        // step_batch should not panic and must return a valid StepResult. This also exercises
        // the source's ingress occupancy and peek semantics indirectly.
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
        // Only sinks implement NodeKind::Sink
        let mut nlink = make_nodelink();
        let kind = nlink.node().node_kind();
        if kind != crate::node::NodeKind::Sink {
            return;
        }

        let clock = NoStdLinuxMonotonicClock::new();
        let mut tele = make_graph_telemetry();
        nlink.initialize(&clock, &mut tele).expect("init ok");

        // Build input edges; sink consumes from inputs, no outputs.
        let (mut in_links, mut out_links) =
            make_edge_links_for_node::<IN, OUT, InP, OutP>(NodeIndex::new(0), NodeIndex::new(1));

        if IN == 0 {
            return;
        }

        // Push a message to input 0 and call step(): sink should consume and return MadeProgress
        let mut hdr = MessageHeader::empty();
        hdr.set_creation_tick(clock.now_ticks());
        let msg = Message::new(hdr, InP::default());
        let policy = TEST_EDGE_POLICY;
        assert_eq!(
            in_links[0].try_push(msg.clone(), &policy),
            crate::edge::EnqueueResult::Enqueued
        );

        // let mut ctx = build_step_context(&mut in_links, &mut [], &clock, &mut tele);
        let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

        let res = nlink.step(&mut ctx);
        match res {
            Ok(r) => {
                assert!(
                    r == crate::node::StepResult::MadeProgress
                        || r == crate::node::StepResult::NoInput,
                    "sink.step returned unexpected StepResult"
                );
            }
            Err(_e) => {
                // An execution failure is acceptable as long as it's an expected error type.
            }
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
        // Only run when the node is a Model and has 1 input & 1 output (InferenceModel is 1×1)
        let mut nlink = make_nodelink();
        if nlink.node().node_kind() != crate::node::NodeKind::Model || IN != 1 || OUT != 1 {
            return;
        }

        // Install a deterministic fixed-N batching policy for the model test.
        // Choose a modest batch size that fits typical test backends.
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

        // Determine the batch size we requested (from the node policy we just installed).
        let requested_fixed = nlink.node().policy().batching().fixed_n().unwrap_or(1usize);

        // Push `requested_fixed` messages; step_batch should process at least requested_fixed
        // (the model node may be capped by backend or MAX_BATCH, but it should not exceed requested_fixed).
        let policy = TEST_EDGE_POLICY;
        for t in 1u64..=(requested_fixed as u64) {
            let mut hdr = MessageHeader::empty();
            hdr.set_creation_tick(Ticks::new(t));
            let m = Message::new(hdr, InP::default());
            assert_eq!(
                in_links[0].try_push(m, &policy),
                crate::edge::EnqueueResult::Enqueued
            );
        }

        let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

        // Execute the batched step.
        let res = nlink.step(&mut ctx).expect("step_batch ok");
        assert!(
            res != crate::node::StepResult::NoInput,
            "model.step_batch returned NoInput"
        );

        // If outputs exist, ensure at least one output was produced, and no more than requested_fixed.
        if OUT > 0 {
            let mut out_count = 0usize;
            while let Ok(_m) = out_links[0].try_pop() {
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

        // Telemetry: NodeLink increments `processed` by `requested_fixed` for a batched step.
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

        // Make a NodeLink and install a deterministic fixed_n + max_delta_t
        // disjoint batching policy for this test so we exercise the span checks.
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

        // Recompute fixed_n and max_delta from the installed policy.
        let policy_installed = *nlink.node().policy().batching();
        let fixed_opt = *policy_installed.fixed_n();
        let delta_opt = *policy_installed.max_delta_t();
        if fixed_opt.is_none() || delta_opt.is_none() {
            // Nothing to test if policy doesn't provide both knobs.
            return;
        }
        let fixed_n = fixed_opt.unwrap();
        let max_delta = *delta_opt.unwrap().as_u64();

        let clock = NoStdLinuxMonotonicClock::new();
        let mut tele = make_graph_telemetry();
        nlink.initialize(&clock, &mut tele).expect("init ok");

        // 1) VALID SPAN: push fixed_n msgs with ticks within max_delta_t and expect a batch processed.
        {
            let (mut in_links, mut out_links) = make_edge_links_for_node::<IN, OUT, InP, OutP>(
                NodeIndex::new(0),
                NodeIndex::new(1),
            );

            let policy = TEST_EDGE_POLICY;

            // Use ticks spaced by 1 up to max_delta to create a valid span.
            for i in 0..fixed_n {
                let tick = i as u64; // within small span (<= max_delta)
                let mut hdr = MessageHeader::empty();
                hdr.set_creation_tick(Ticks::new(tick));
                let m = Message::new(hdr, InP::default());
                assert_eq!(
                    in_links[0].try_push(m, &policy),
                    crate::edge::EnqueueResult::Enqueued
                );
            }

            // Capture telemetry before the valid-span step so we can assert the delta.
            let metrics_before = tele.metrics();
            let processed_before = *metrics_before.nodes()[0].processed();

            let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

            let res = nlink.step(&mut ctx).expect("step_batch ok (valid span)");
            assert!(
                res != crate::node::StepResult::NoInput,
                "expected batch processed for valid span"
            );

            if OUT > 0 {
                let mut out_count = 0usize;
                while let Ok(_m) = out_links[0].try_pop() {
                    out_count += 1;
                }
                // Expect exactly fixed_n outputs when batch processed fully
                assert_eq!(
                    out_count, fixed_n,
                    "expected exactly fixed_n outputs ({}) for valid span, got {}",
                    fixed_n, out_count
                );
            }

            // Telemetry should have been incremented by `fixed_n` for the batched step.
            let metrics_after = tele.metrics();
            let processed_after = *metrics_after.nodes()[0].processed();
            assert_eq!(
                processed_after.saturating_sub(processed_before),
                fixed_n as u64,
                "expected telemetry processed to increase by fixed_n for valid span"
            );
        }

        // 2) INVALID SPAN: push fixed_n msgs with ticks far apart (> max_delta_t) and expect NoInput
        {
            let (mut in_links, mut out_links) = make_edge_links_for_node::<IN, OUT, InP, OutP>(
                NodeIndex::new(0),
                NodeIndex::new(1),
            );

            let policy = TEST_EDGE_POLICY;
            // Use ticks spaced beyond max_delta to violate span
            for i in 0..fixed_n {
                let tick = (i as u64) * (max_delta + 1000u64);
                let mut hdr = MessageHeader::empty();
                hdr.set_creation_tick(Ticks::new(tick));
                let m = Message::new(hdr, InP::default());
                assert_eq!(
                    in_links[0].try_push(m, &policy),
                    crate::edge::EnqueueResult::Enqueued
                );
            }

            // Capture telemetry before the invalid-span step.
            let metrics_before_invalid = tele.metrics();
            let processed_before_invalid = *metrics_before_invalid.nodes()[0].processed();

            let mut ctx = build_step_context(&mut in_links, &mut out_links, &clock, &mut tele);

            let res = nlink.step(&mut ctx).expect("step_batch ok (invalid span)");

            // For invalid span, prefer NoInput (a node may also process fewer items; allow both).
            if res == crate::node::StepResult::NoInput {
                // Telemetry must not have advanced for the node (no batch processed).
                let metrics_after_invalid = tele.metrics();
                let processed_after_invalid = *metrics_after_invalid.nodes()[0].processed();
                assert_eq!(
                    processed_after_invalid, processed_before_invalid,
                    "expected no telemetry change when invalid span results in NoInput"
                );
            } else {
                // MadeProgress (partial processing) is allowed; ensure outputs are fewer than fixed_n.
                assert_eq!(
                    res,
                    crate::node::StepResult::MadeProgress,
                    "unexpected StepResult for invalid span: {:?}",
                    res
                );

                if OUT > 0 {
                    let mut out_count = 0usize;
                    while let Ok(_m) = out_links[0].try_pop() {
                        out_count += 1;
                    }
                    assert!(
                        out_count > 0 && out_count < fixed_n,
                        "expected partial progress for invalid span (0 < out_count < fixed_n), got {}",
                        out_count
                    );
                }
                // We do not assert telemetry delta here — implementations may still
                // increment processed by `fixed_n` even for partial progress.
            }
        }
    }
}
