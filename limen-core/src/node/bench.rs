//! (Work)bench [test] Node implementations.

use super::*;

use crate::errors::NodeError;
use crate::memory::{MemoryClass, PlacementAcceptance};
use crate::message::Message;
use crate::message::{MessageFlags, MessageHeader};
use crate::queue::SpscQueue;
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
        InQ: SpscQueue<Item = Message<()>>,
        OutQ: SpscQueue<Item = Message<u32>>,
    {
        let header = self.make_header();
        let message = Message::new(header, self.next_value_to_emit);

        #[cfg(feature = "std")]
        {
            println!("--- [src::step] --- pushing message: {:?}", message);
        }

        let enqueue_result = ctx.out_try_push(0, message);

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

        let enqueue_result = ctx.out_try_push(0, message);

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
