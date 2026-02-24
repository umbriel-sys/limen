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
                        let size = core::cmp::min(*sw.size(), nmax);
                        let stride = core::cmp::min(*sw.stride(), size);
                        WindowKind::Sliding(SlidingWindow::new(size, stride))
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
                        let size = core::cmp::min(*sw.size(), nmax);
                        let stride = core::cmp::min(*sw.stride(), size);
                        WindowKind::Sliding(SlidingWindow::new(size, stride))
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
    fn process_message<'graph, 'telemetry, 'clock, OutQ, C, T>(
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
                    let msg_ref: &Message<InP> = &msg;

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
