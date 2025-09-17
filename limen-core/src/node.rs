//! Uniform Node contract and lifecycle.
//!
//! Nodes are monomorphized by generics and const generics. There is **no dynamic
//! dispatch** in the hot path. Port schemas and policies are encoded on the Node.

pub mod descriptor;
pub mod routing;

use crate::errors::{NodeError, QueueError};
use crate::memory::PlacementAcceptance;
use crate::message::{payload::Payload, Message};
use crate::policy::{BatchingPolicy, BudgetPolicy, DeadlinePolicy, EdgePolicy};
use crate::queue::{EnqueueResult, QueueOccupancy, SpscQueue};
use crate::telemetry::Telemetry;
use crate::types::Ticks;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeCapabilities {
    /// Whether the node can execute on device streams (P2).
    pub device_streams: bool,
    /// Whether mixed-precision or degrade tiers are available.
    pub degrade_tiers: bool,
}

/// Policy bundle attached to a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodePolicy {
    /// Batch formation policy.
    pub batching: BatchingPolicy,
    /// Budget policy for execution steps.
    pub budget: BudgetPolicy,
    /// Deadline policy for inputs/outputs.
    pub deadline: DeadlinePolicy,
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
pub struct StepContext<'a, const IN: usize, const OUT: usize, InP, OutP, InQ, OutQ, C, T>
where
    InP: Payload,
    OutP: Payload,
{
    /// Arrays of inbound queues by input port index.
    pub inputs: [&'a mut InQ; IN],
    /// Arrays of outbound queues by output port index.
    pub outputs: [&'a mut OutQ; OUT],
    /// Edge policies for each input.
    pub in_policies: [EdgePolicy; IN],
    /// Edge policies for each output.
    pub out_policies: [EdgePolicy; OUT],
    /// Platform clock or timer services.
    pub clock: &'a C,
    /// Telemetry sink for counters/histograms.
    pub telemetry: &'a mut T,
    /// Phantom type markers to keep payload types visible to the compiler.
    _marker: core::marker::PhantomData<(InP, OutP)>,
}

impl<'a, const IN: usize, const OUT: usize, InP, OutP, InQ, OutQ, C, T>
    StepContext<'a, IN, OUT, InP, OutP, InQ, OutQ, C, T>
where
    InP: Payload,
    OutP: Payload,
{
    /// Create a new step context from queues, policies, and services.
    pub fn new(
        inputs: [&'a mut InQ; IN],
        outputs: [&'a mut OutQ; OUT],
        in_policies: [EdgePolicy; IN],
        out_policies: [EdgePolicy; OUT],
        clock: &'a C,
        telemetry: &'a mut T,
    ) -> Self {
        Self {
            inputs,
            outputs,
            in_policies,
            out_policies,
            clock,
            telemetry,
            _marker: core::marker::PhantomData,
        }
    }
}

impl<'a, const IN: usize, const OUT: usize, InP, OutP, InQ, OutQ, C, T>
    StepContext<'a, IN, OUT, InP, OutP, InQ, OutQ, C, T>
where
    InP: Payload,
    OutP: Payload,
    InQ: SpscQueue<Item = Message<InP>>,
    OutQ: SpscQueue<Item = Message<OutP>>,
{
    /// Attempt to pop an item from the specified input queue.
    #[inline]
    pub fn in_try_pop(&mut self, i: usize) -> Result<Message<InP>, QueueError> {
        debug_assert!(i < IN);
        self.inputs[i].try_pop()
    }

    /// Attempt to peek at the front item in the specifiend input queue without removing it.
    #[inline]
    pub fn in_try_peek(&self, i: usize) -> Result<&Message<InP>, QueueError> {
        debug_assert!(i < IN);
        self.inputs[i].try_peek()
    }

    /// Return a snapshot of occupancy of the specified input queue, used for telemetry and admission.
    #[inline]
    pub fn in_occupancy(&self, i: usize) -> QueueOccupancy {
        debug_assert!(i < IN);
        self.inputs[i].occupancy(&self.in_policies[i])
    }

    /// Attempt to ush an item to the specified output queue.
    #[inline]
    pub fn out_try_push(&mut self, o: usize, m: Message<OutP>) -> EnqueueResult {
        debug_assert!(o < OUT);
        self.outputs[o].try_push(m, &self.out_policies[o])
    }

    /// Return a snapshot of occupancy of the specified output queue, used for telemetry and admission.
    #[inline]
    pub fn out_occupancy(&self, o: usize) -> QueueOccupancy {
        debug_assert!(o < OUT);
        self.outputs[o].occupancy(&self.out_policies[o])
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
    fn initialize<C, T>(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError>;

    /// Optional warm-up (e.g., compile kernels, prime pools). Default: no-op.
    fn start<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>;

    /// Execute one cooperative step using the provided context.
    ///
    /// The input and output queues are exposed through the context, along with
    /// per-edge policies and services. Implementations should honor the node
    /// policy (batching, budgets, deadlines) and return a `StepResult` to help
    /// the scheduler make progress decisions.
    fn step<InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<IN, OUT, InP, OutP, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<InP>>,
        OutQ: SpscQueue<Item = Message<OutP>>;

    /// Handle watchdog timeouts by applying over-budget policy (degrade/default/skip).
    fn on_watchdog_timeout<C, T>(
        &mut self,
        _clock: &C,
        _telemetry: &mut T,
    ) -> Result<StepResult, NodeError>;

    /// Flush and release resources, if any. Default: no-op.
    fn stop<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError>;
}
