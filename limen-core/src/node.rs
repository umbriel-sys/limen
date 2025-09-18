//! Uniform Node contract and lifecycle.
//!
//! Nodes are monomorphized by generics and const generics. There is **no dynamic
//! dispatch** in the hot path. Port schemas and policies are encoded on the Node.

pub mod descriptor;
pub mod routing;

use crate::errors::{NodeError, QueueError};
use crate::memory::{MemoryClass, PlacementAcceptance};
use crate::message::{payload::Payload, Message};
use crate::message::{MessageFlags, MessageHeader};
use crate::policy::{BatchingPolicy, BudgetPolicy, DeadlinePolicy, EdgePolicy};
use crate::queue::{EnqueueResult, QueueOccupancy, SpscQueue};
use crate::telemetry::Telemetry;
use crate::types::{DeadlineNs, QoSClass, SequenceNumber, Ticks, TraceId};

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

/// Policy bundle attached to a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

// TEMPORARY TEST CODE!

use core::fmt::Write;

// -----------------------------------------------------------------------------
// TestSourceNodeU32: 0 inputs, 1 output (u32 payload), emits incrementing values
// -----------------------------------------------------------------------------

/// test source
pub struct TestSourceNodeU32 {
    next_value_to_emit: u32,

    // Header template fields:
    trace_id: TraceId,
    next_sequence: SequenceNumber,
    next_creation_tick: Ticks,
    deadline_ns: Option<DeadlineNs>,
    qos: QoSClass,
    flags: MessageFlags,

    // Static node properties:
    node_capabilities: NodeCapabilities,
    node_policy: NodePolicy,
    output_placement_acceptance: [PlacementAcceptance; 1],
}

impl TestSourceNodeU32 {
    /// new
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        starting_value_inclusive: u32,
        trace_id: TraceId,
        starting_sequence: SequenceNumber,
        starting_tick: Ticks,
        deadline_ns: Option<DeadlineNs>,
        qos: QoSClass,
        flags: MessageFlags,
        node_capabilities: NodeCapabilities,
        node_policy: NodePolicy,
        output_placement_acceptance: [PlacementAcceptance; 1],
    ) -> Self {
        Self {
            next_value_to_emit: starting_value_inclusive,
            trace_id,
            next_sequence: starting_sequence,
            next_creation_tick: starting_tick,
            deadline_ns,
            qos,
            flags,
            node_capabilities,
            node_policy,
            output_placement_acceptance,
        }
    }

    #[inline]
    fn make_header(&self) -> MessageHeader {
        // `payload_size_bytes` and `memory_class` are fixed by `Message::new`.
        MessageHeader::new(
            self.trace_id,
            self.next_sequence,
            self.next_creation_tick,
            self.deadline_ns,
            self.qos,
            0,
            self.flags,
            MemoryClass::Host,
        )
    }
}

impl Node<0, 1, (), u32> for TestSourceNodeU32 {
    fn describe_capabilities(&self) -> NodeCapabilities {
        self.node_capabilities
    }

    fn input_acceptance(&self) -> [PlacementAcceptance; 0] {
        []
    }

    fn output_acceptance(&self) -> [PlacementAcceptance; 1] {
        self.output_placement_acceptance
    }

    fn policy(&self) -> NodePolicy {
        self.node_policy
    }

    fn node_kind(&self) -> NodeKind {
        NodeKind::Source
    }

    fn initialize<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> {
        Ok(())
    }

    fn start<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> {
        Ok(())
    }

    fn step<InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<0, 1, (), u32, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<()>>,
        OutQ: SpscQueue<Item = Message<u32>>,
    {
        let header = self.make_header();
        let message = Message::new(header, self.next_value_to_emit);

        let _ = ctx.out_try_push(0, message);

        // Advance counters (wrapping is fine for simple test behavior).
        self.next_value_to_emit = self.next_value_to_emit.wrapping_add(1);
        self.next_sequence = SequenceNumber((self.next_sequence).0.wrapping_add(1));
        self.next_creation_tick = Ticks((self.next_creation_tick).0.wrapping_add(1));

        Ok(StepResult::MadeProgress)
    }

    fn on_watchdog_timeout<C, T>(
        &mut self,
        _clock: &C,
        _telemetry: &mut T,
    ) -> Result<StepResult, NodeError> {
        Ok(StepResult::WaitingOnExternal)
    }

    fn stop<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> {
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// TestIdentityModelNodeU32: 1 input, 1 output (identity pass-through)
// -----------------------------------------------------------------------------

/// test model
pub struct TestIdentityModelNodeU32 {
    node_capabilities: NodeCapabilities,
    node_policy: NodePolicy,
    input_placement_acceptance: [PlacementAcceptance; 1],
    output_placement_acceptance: [PlacementAcceptance; 1],
}

impl TestIdentityModelNodeU32 {
    /// new
    pub const fn new(
        node_capabilities: NodeCapabilities,
        node_policy: NodePolicy,
        input_placement_acceptance: [PlacementAcceptance; 1],
        output_placement_acceptance: [PlacementAcceptance; 1],
    ) -> Self {
        Self {
            node_capabilities,
            node_policy,
            input_placement_acceptance,
            output_placement_acceptance,
        }
    }
}

impl Node<1, 1, u32, u32> for TestIdentityModelNodeU32 {
    fn describe_capabilities(&self) -> NodeCapabilities {
        self.node_capabilities
    }

    fn input_acceptance(&self) -> [PlacementAcceptance; 1] {
        self.input_placement_acceptance
    }

    fn output_acceptance(&self) -> [PlacementAcceptance; 1] {
        self.output_placement_acceptance
    }

    fn policy(&self) -> NodePolicy {
        self.node_policy
    }

    fn node_kind(&self) -> NodeKind {
        NodeKind::Model
    }

    fn initialize<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> {
        Ok(())
    }

    fn start<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> {
        Ok(())
    }

    fn step<InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<1, 1, u32, u32, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<u32>>,
        OutQ: SpscQueue<Item = Message<u32>>,
    {
        let Ok(message) = ctx.in_try_pop(0) else {
            return Ok(StepResult::NoInput);
        };

        let _ = ctx.out_try_push(0, message);

        Ok(StepResult::MadeProgress)
    }

    fn on_watchdog_timeout<C, T>(
        &mut self,
        _clock: &C,
        _telemetry: &mut T,
    ) -> Result<StepResult, NodeError> {
        Ok(StepResult::WaitingOnExternal)
    }

    fn stop<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> {
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// TestSinkNodeU32: 1 input, 0 outputs, logs full Message<u32>
// -----------------------------------------------------------------------------

/// test source
pub struct TestSinkNodeU32 {
    node_capabilities: NodeCapabilities,
    node_policy: NodePolicy,
    input_placement_acceptance: [PlacementAcceptance; 1],
    printer: fn(&str),
}

impl TestSinkNodeU32 {
    /// new
    pub const fn new(
        node_capabilities: NodeCapabilities,
        node_policy: NodePolicy,
        input_placement_acceptance: [PlacementAcceptance; 1],
        printer: fn(&str),
    ) -> Self {
        Self {
            node_capabilities,
            node_policy,
            input_placement_acceptance,
            printer,
        }
    }
}

// simple fixed-size stack buffer implementing core::fmt::Write
struct FixedBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> FixedBuf<N> {
    #[inline]
    const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }
    #[inline]
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or_default()
    }
}

impl<const N: usize> core::fmt::Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let remaining = N.saturating_sub(self.len);
        if bytes.len() > remaining {
            return Err(core::fmt::Error);
        }
        let dst = &mut self.buf[self.len..self.len + bytes.len()];
        for (d, b) in dst.iter_mut().zip(bytes.iter().copied()) {
            *d = b;
        }
        self.len += bytes.len();
        Ok(())
    }
}

impl Node<1, 0, u32, ()> for TestSinkNodeU32 {
    fn describe_capabilities(&self) -> NodeCapabilities {
        self.node_capabilities
    }

    fn input_acceptance(&self) -> [PlacementAcceptance; 1] {
        self.input_placement_acceptance
    }

    fn output_acceptance(&self) -> [PlacementAcceptance; 0] {
        []
    }

    fn policy(&self) -> NodePolicy {
        self.node_policy
    }

    fn node_kind(&self) -> NodeKind {
        NodeKind::Sink
    }

    fn initialize<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> {
        Ok(())
    }

    fn start<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> {
        Ok(())
    }

    fn step<InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<1, 0, u32, (), InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<u32>>,
        OutQ: SpscQueue<Item = Message<()>>,
    {
        let Ok(message) = ctx.in_try_pop(0) else {
            return Ok(StepResult::NoInput);
        };

        let mut buf: FixedBuf<256> = FixedBuf::new();
        let _ = core::write!(&mut buf, "{:?}", message);
        (self.printer)(buf.as_str());

        Ok(StepResult::MadeProgress)
    }

    fn on_watchdog_timeout<C, T>(
        &mut self,
        _clock: &C,
        _telemetry: &mut T,
    ) -> Result<StepResult, NodeError> {
        Ok(StepResult::WaitingOnExternal)
    }

    fn stop<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> {
        Ok(())
    }
}
