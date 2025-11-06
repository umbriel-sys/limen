//! Uniform Node contract and lifecycle.
//!
//! Nodes are monomorphized by generics and const generics. There is **no dynamic
//! dispatch** in the hot path. Port schemas and policies are encoded on the Node.

pub mod bench;
pub mod link;
pub mod model;
pub mod sink;
pub mod source;

use crate::edge::{Edge, EdgeOccupancy};
use crate::errors::{NodeError, QueueError};
use crate::memory::PlacementAcceptance;
use crate::message::{payload::Payload, Message};
use crate::policy::{EdgePolicy, NodePolicy};
use crate::prelude::{PlatformClock, TelemetryKey, TelemetryKind};
use crate::telemetry::Telemetry;
use crate::types::Ticks;

#[cfg(feature = "std")]
use crate::edge::EnqueueResult;

/// Categories of nodes used in graph descriptors and builders.
///
/// These capture the high-level role of a node in the dataflow graph.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NodeCapabilities {
    /// Whether the node can execute on device streams (P2).
    pub device_streams: bool,
    /// Whether mixed-precision or degrade tiers are available.
    pub degrade_tiers: bool,
}

/// Result of a `step` call indicating progress and scheduling hints.
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
                // Node ingress + input-edge processed counter.
                self.telemetry.incr_counter(
                    TelemetryKey::node(self.node_id, TelemetryKind::IngressMsgs),
                    1,
                );
                self.telemetry.incr_counter(
                    TelemetryKey::edge(self.in_edge_ids[i], TelemetryKind::Processed),
                    1,
                );
                Ok(msg)
            }
            Err(e) => Err(e),
        }
    }

    /// Attempt to peek at the front item without removing it.
    #[inline]
    pub fn in_try_peek(&self, i: usize) -> Result<&Message<InP>, QueueError> {
        debug_assert!(i < IN);
        self.inputs[i].try_peek()
    }

    /// Return a snapshot of occupancy of the specified input queue.
    #[inline]
    pub fn in_occupancy(&mut self, i: usize) -> EdgeOccupancy {
        debug_assert!(i < IN);
        let occ = self.inputs[i].occupancy(&self.in_policies[i]);
        // Update an edge QueueDepth gauge (optional but useful).
        self.telemetry.set_gauge(
            TelemetryKey::edge(self.in_edge_ids[i], TelemetryKind::QueueDepth),
            occ.items as u64,
        );
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
        match res {
            crate::edge::EnqueueResult::Rejected => {
                // Admission failed: count drops on both node and edge.
                self.telemetry.incr_counter(
                    TelemetryKey::edge(self.out_edge_ids[o], TelemetryKind::Dropped),
                    1,
                );
                self.telemetry
                    .incr_counter(TelemetryKey::node(self.node_id, TelemetryKind::Dropped), 1);
            }
            _ => {
                // Successful egress.
                self.telemetry.incr_counter(
                    TelemetryKey::node(self.node_id, TelemetryKind::EgressMsgs),
                    1,
                );
                self.telemetry.incr_counter(
                    TelemetryKey::edge(self.out_edge_ids[o], TelemetryKind::Processed),
                    1,
                );
            }
        }
        res
    }

    /// Return a snapshot of occupancy of the specified output queue.
    #[inline]
    pub fn out_occupancy(&mut self, o: usize) -> EdgeOccupancy {
        debug_assert!(o < OUT);
        let occ = self.outputs[o].occupancy(&self.out_policies[o]);
        self.telemetry.set_gauge(
            TelemetryKey::edge(self.out_edge_ids[o], TelemetryKind::QueueDepth),
            occ.items as u64,
        );
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
        T: Telemetry + Sized;

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
