//! (Work)bench [test] Node implementations.

use super::*;

use crate::compute::{BackendCapabilities, ComputeBackend, ComputeModel, ModelMetadata};
use crate::edge::Edge;
use crate::errors::{InferenceError, InferenceErrorKind, NodeError};
use crate::memory::{MemoryClass, PlacementAcceptance};
use crate::message::Message;
use crate::message::{MessageFlags, MessageHeader};
use crate::node::model::InferenceModel;
#[cfg(feature = "std")]
use crate::node::source::probe::{SourceIngressProbe, SourceIngressUpdater};
use crate::node::source::Source;
use crate::types::{DeadlineNs, QoSClass, SequenceNumber, Ticks, TraceId};

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
        InQ: Edge<Item = Message<()>>,
        OutQ: Edge<Item = Message<u32>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let header = self.make_header();
        let message = Message::new(header, self.next_value_to_emit);

        #[cfg(feature = "std")]
        {
            println!("--- [src::step] --- pushing message: {:?}", message);
        }

        #[cfg(feature = "std")]
        let enqueue_result = ctx.out_try_push(0, message);

        #[cfg(not(feature = "std"))]
        let _ = ctx.out_try_push(0, message);

        #[cfg(feature = "std")]
        {
            match enqueue_result {
                EnqueueResult::Enqueued => println!("--- [src::step] --- Enqueue succeded."),
                EnqueueResult::DroppedNewest => {
                    println!("--- [src::step] --- Enqueue succeded, newest dropped.")
                }
                EnqueueResult::Rejected => print!("--- [src::step] --- Enqueue Rejected."),
            }
        }

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

/// A test source that:
/// - Produces an incrementing `u32` on each `try_produce()`.
/// - Exposes *ingress* pressure via either an internal backlog or a std probe.
pub struct TestCounterSourceU32_2 {
    // Next value to emit.
    next_value_to_emit: u32,

    // Header template fields:
    trace_id: TraceId,
    next_sequence: SequenceNumber,
    next_creation_tick: Ticks,
    deadline_ns: Option<DeadlineNs>,
    qos: QoSClass,
    flags: MessageFlags,

    // Static properties:
    node_capabilities: NodeCapabilities,
    node_policy: NodePolicy,
    output_placement_acceptance: [PlacementAcceptance; 1],

    // ---- Upstream pressure modelling ----
    // no_std/internal software backlog (items/bytes before the source).
    backlog_items: usize,
    backlog_bytes: usize,

    // std optional shared probe (if present, it is authoritative).
    #[cfg(feature = "std")]
    ingress_probe: Option<SourceIngressProbe>,
    #[cfg(feature = "std")]
    ingress_updater: Option<SourceIngressUpdater>,
}

impl TestCounterSourceU32_2 {
    /// Create a new TestCounterSourceU32_2.
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
            backlog_items: 0,
            backlog_bytes: 0,
            #[cfg(feature = "std")]
            ingress_probe: None,
            #[cfg(feature = "std")]
            ingress_updater: None,
        }
    }

    /// Attach a std ingress probe + updater (authoritative for occupancy when present).
    #[cfg(feature = "std")]
    pub fn with_probe(mut self, probe: SourceIngressProbe, updater: SourceIngressUpdater) -> Self {
        self.ingress_probe = Some(probe);
        self.ingress_updater = Some(updater);
        self
    }

    /// Set a synthetic upstream backlog (items).
    #[inline]
    pub fn set_upstream_backlog_items(&mut self, n: usize) {
        self.backlog_items = n;
    }

    /// Set a synthetic upstream backlog (bytes).
    #[inline]
    pub fn set_upstream_backlog_bytes(&mut self, b: usize) {
        self.backlog_bytes = b;
    }

    #[inline]
    fn make_header(&self) -> MessageHeader {
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

    #[inline]
    fn advance_counters(&mut self) {
        // Wrapping increments are fine for a test source.
        self.next_value_to_emit = self.next_value_to_emit.wrapping_add(1);
        self.next_sequence = SequenceNumber((self.next_sequence).0.wrapping_add(1));
        self.next_creation_tick = Ticks((self.next_creation_tick).0.wrapping_add(1));
    }

    /// Consume one unit from the software backlog when we successfully produce.
    #[inline]
    fn consume_software_backlog_one(&mut self) {
        if self.backlog_items > 0 {
            self.backlog_items -= 1;
        }
        let sz = core::mem::size_of::<u32>();
        if self.backlog_bytes >= sz {
            self.backlog_bytes -= sz;
        } else {
            self.backlog_bytes = 0;
        }
    }
}

impl Source<u32, 1> for TestCounterSourceU32_2 {
    type Error = core::convert::Infallible;

    #[inline]
    fn open(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    #[inline]
    fn try_produce(&mut self) -> Option<(usize, Message<u32>)> {
        // Produce one message on port 0.
        let header = self.make_header();
        let msg = Message::new(header, self.next_value_to_emit);

        // Advance header counters and consume one item from the *software* backlog.
        // (If a std probe is attached, the external thread should decrement that.)
        self.advance_counters();
        self.consume_software_backlog_one();

        Some((0, msg))
    }

    #[inline]
    fn ingress_occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        #[cfg(feature = "std")]
        if let Some(probe) = &self.ingress_probe {
            return probe.occupancy(policy);
        }

        // Fallback to software backlog counters.
        let items = self.backlog_items;
        let bytes = self.backlog_bytes;
        EdgeOccupancy {
            items,
            bytes,
            watermark: policy.watermark(items, bytes),
        }
    }

    #[inline]
    fn output_acceptance(&self) -> [PlacementAcceptance; 1] {
        self.output_placement_acceptance
    }

    #[inline]
    fn capabilities(&self) -> NodeCapabilities {
        self.node_capabilities
    }

    #[inline]
    fn policy(&self) -> NodePolicy {
        self.node_policy
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
        InQ: Edge<Item = Message<u32>>,
        OutQ: Edge<Item = Message<u32>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let Ok(message) = ctx.in_try_pop(0) else {
            #[cfg(feature = "std")]
            {
                println!("--- [map::step] --- no input on in0");
            }
            return Ok(StepResult::NoInput);
        };

        #[cfg(feature = "std")]
        {
            println!("--- [map::step] --- received on in0: {:?}", message);
        }

        #[cfg(feature = "std")]
        let enqueue_result = ctx.out_try_push(0, message);

        #[cfg(not(feature = "std"))]
        let _ = ctx.out_try_push(0, message);

        #[cfg(feature = "std")]
        {
            match enqueue_result {
                EnqueueResult::Enqueued => println!("--- [map::step] --- out0 enqueue succeeded."),
                EnqueueResult::DroppedNewest => {
                    println!("--- [map::step] --- out0 enqueue succeeded, newest dropped.")
                }
                EnqueueResult::Rejected => println!("--- [map::step] --- out0 enqueue rejected."),
            }
        }

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
// Test backend + model for u32 → u32 identity, no alloc, no dyn, no unsafe.
// -----------------------------------------------------------------------------

/// ---------- Test model ----------
pub struct TestU32Model;

impl ComputeModel<u32, u32> for TestU32Model {
    #[inline]
    fn init(&mut self) -> Result<(), InferenceError> {
        Ok(())
    }

    #[inline]
    fn infer_one(&mut self, inp: &u32, out: &mut u32) -> Result<(), InferenceError> {
        *out = *inp;
        Ok(())
    }

    #[inline]
    fn infer_batch(&mut self, inputs: &[u32], outputs: &mut [u32]) -> Result<(), InferenceError> {
        if outputs.len() < inputs.len() {
            return Err(InferenceError::new(InferenceErrorKind::ExecutionFailed, 0));
        }
        // copy inputs → outputs (identity)
        for (o, i) in outputs.iter_mut().zip(inputs.iter()) {
            *o = *i;
        }
        Ok(())
    }

    #[inline]
    fn drain(&mut self) -> Result<(), InferenceError> {
        Ok(())
    }

    #[inline]
    fn reset(&mut self) -> Result<(), InferenceError> {
        Ok(())
    }

    #[inline]
    fn metadata(&self) -> ModelMetadata {
        ModelMetadata {
            preferred_input: MemoryClass::Host,
            preferred_output: MemoryClass::Host,
            max_input_bytes: None,
            max_output_bytes: None,
        }
    }
}

/// ---------- Test backend ----------
#[derive(Clone, Copy, Debug, Default)]
pub struct TestU32Backend;

impl ComputeBackend<u32, u32> for TestU32Backend {
    type Model = TestU32Model;
    type Error = InferenceError;

    // Unit descriptor: no artifact needed for this test model.
    type ModelDescriptor<'d> = ();

    #[inline]
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            device_streams: false,
            max_batch: Some(usize::MAX),
            dtype_mask: 0,
        }
    }

    #[inline]
    fn load_model<'d>(&self, _desc: Self::ModelDescriptor<'d>) -> Result<Self::Model, Self::Error> {
        Ok(TestU32Model)
    }
}

/// Alias preserving the old test node name.
pub type TestIdentityModelNodeU32_2<const MAX_BATCH: usize> =
    InferenceModel<TestU32Backend, u32, u32, MAX_BATCH>;

impl<const MAX_BATCH: usize> TestIdentityModelNodeU32_2<MAX_BATCH> {
    /// Construct the identity test node with your policy/capability params.
    #[inline]
    pub fn new_identity(
        node_capabilities: NodeCapabilities,
        node_policy: NodePolicy,
        input_placement_acceptance: [PlacementAcceptance; 1],
        output_placement_acceptance: [PlacementAcceptance; 1],
    ) -> Result<Self, InferenceError> {
        let backend = TestU32Backend;
        InferenceModel::new(
            backend,
            (),
            node_policy,
            node_capabilities,
            input_placement_acceptance,
            output_placement_acceptance,
        )
    }

    /// Handy constant for tests.
    #[inline]
    pub fn kind() -> NodeKind {
        NodeKind::Model
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
        InQ: Edge<Item = Message<u32>>,
        OutQ: Edge<Item = Message<()>>,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let Ok(message) = ctx.in_try_pop(0) else {
            #[cfg(feature = "std")]
            {
                println!("--- [snk::step] --- no input on in0");
            }
            return Ok(StepResult::NoInput);
        };

        #[cfg(feature = "std")]
        {
            println!("--- [snk::step] --- received on in0: {:?}", message);
        }

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
