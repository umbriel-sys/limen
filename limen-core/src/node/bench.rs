//! (Work)bench [test] Node implementations.

use super::*;

use crate::compute::{BackendCapabilities, ComputeBackend, ComputeModel, ModelMetadata};
use crate::errors::{InferenceError, InferenceErrorKind};
use crate::memory::{MemoryClass, PlacementAcceptance};
use crate::message::Message;
use crate::message::{MessageFlags, MessageHeader};
use crate::node::model::InferenceModel;
use crate::node::sink::Sink;
use crate::node::source::Source;
use crate::types::{DeadlineNs, QoSClass, SequenceNumber, Ticks, TraceId};

#[cfg(feature = "std")]
use crate::node::source::probe::{SourceIngressProbe, SourceIngressUpdater};

use core::fmt::Write;

/// Busy-waits for a pseudo random duration up to `max_delay_microseconds` microseconds.
///
/// # Input
/// * `random_state`: mutable pseudo random number generator state; any `u32` value is accepted.
///   If it is zero, it will be internally changed to one to avoid the all-zero XorShift32 state.
/// * `max_delay_microseconds`: maximum delay in microseconds; if zero, this function returns immediately.
fn random_test_node_delay(random_state: &mut u32, max_delay_microseconds: u32) {
    // No delay requested.
    if max_delay_microseconds == 0 {
        return;
    }

    // XorShift32 step (simple pseudo random number generator)
    if *random_state == 0 {
        *random_state = 1;
    }
    let mut current_state = *random_state;
    current_state ^= current_state << 13;
    current_state ^= current_state >> 17;
    current_state ^= current_state << 5;
    *random_state = current_state;

    // Random delay in "microseconds": 1..=max_delay_microseconds
    let delay_microseconds = (current_state % max_delay_microseconds) + 1;

    // Rough timing model for a laptop-class central processing unit.
    let assumed_cpu_frequency_hertz: u32 = 2_000_000_000; // two gigahertz
    let estimated_cpu_cycles_per_loop_iteration: u32 = 8;

    // At two gigahertz there are two thousand cycles per microsecond.
    let cycles_per_microsecond = assumed_cpu_frequency_hertz / 1_000_000;

    // Convert cycles per microsecond into loop iterations per microsecond.
    let mut iterations_per_microsecond =
        cycles_per_microsecond / estimated_cpu_cycles_per_loop_iteration;
    if iterations_per_microsecond == 0 {
        iterations_per_microsecond = 1;
    }

    let total_iterations = delay_microseconds.saturating_mul(iterations_per_microsecond);

    for _iteration in 0..total_iterations {
        core::hint::spin_loop();
    }
}

// -----------------------------------------------------------------------------
// TestSourceNodeU32: 0 inputs, 1 output (u32 payload), emits incrementing values
// -----------------------------------------------------------------------------

/// A test source that:
/// - Produces an incrementing `u32` on each `try_produce()`.
/// - Exposes *ingress* pressure via either an internal backlog or a std probe.
pub struct TestCounterSourceU32_2<Clock>
where
    Clock: PlatformClock,
{
    /// Monotonic platform clock used to stamp creation ticks.
    clock: Clock,

    // Next value to emit.
    next_value_to_emit: u32,

    // Header template fields:
    trace_id: TraceId,
    next_sequence: SequenceNumber,
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

impl<Clock> TestCounterSourceU32_2<Clock>
where
    Clock: PlatformClock,
{
    /// Create a new TestCounterSourceU32_2.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        clock: Clock,
        starting_value_inclusive: u32,
        trace_id: TraceId,
        starting_sequence: SequenceNumber,
        deadline_ns: Option<DeadlineNs>,
        qos: QoSClass,
        flags: MessageFlags,
        node_capabilities: NodeCapabilities,
        node_policy: NodePolicy,
        output_placement_acceptance: [PlacementAcceptance; 1],
    ) -> Self {
        Self {
            clock,
            next_value_to_emit: starting_value_inclusive,
            trace_id,
            next_sequence: starting_sequence,
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
        let creation_tick: Ticks = self.clock.now_ticks();

        MessageHeader::new(
            self.trace_id,
            self.next_sequence,
            creation_tick,
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
        self.next_sequence = SequenceNumber::new(self.next_sequence.as_u64().wrapping_add(1));
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

impl<Clock> Source<u32, 1> for TestCounterSourceU32_2<Clock>
where
    Clock: PlatformClock,
{
    type Error = core::convert::Infallible;

    #[inline]
    fn open(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    #[inline]
    fn try_produce(&mut self) -> Option<(usize, Message<u32>)> {
        #[cfg(feature = "std")]
        let mut random_seed: u32 = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_else(|e| e.duration());
            (now.as_nanos() & 0xFFFF_FFFF) as u32
        };
        #[cfg(not(feature = "std"))]
        let mut random_seed = 1;
        random_test_node_delay(&mut random_seed, 250);

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
        #[cfg(feature = "std")]
        let mut random_seed: u32 = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_else(|e| e.duration());
            (now.as_nanos() & 0xFFFF_FFFF) as u32
        };
        #[cfg(not(feature = "std"))]
        let mut random_seed = 1;
        random_test_node_delay(&mut random_seed, 500);

        *out = *inp;
        Ok(())
    }

    #[inline]
    fn infer_batch(&mut self, inputs: &[u32], outputs: &mut [u32]) -> Result<(), InferenceError> {
        #[cfg(feature = "std")]
        let mut random_seed: u32 = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_else(|e| e.duration());
            (now.as_nanos() & 0xFFFF_FFFF) as u32
        };
        #[cfg(not(feature = "std"))]
        let mut random_seed = 1;
        random_test_node_delay(&mut random_seed, 1000);

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
// Implements `sink::Sink<u32, 1>` so it can be used via `SinkNode<_, u32, 1>`.
// -----------------------------------------------------------------------------

/// test sink
pub struct TestSinkNodeU32_2 {
    node_capabilities: NodeCapabilities,
    node_policy: NodePolicy,
    input_placement_acceptance: [PlacementAcceptance; 1],
    printer: fn(&str),
}

impl TestSinkNodeU32_2 {
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

impl Sink<u32, 1> for TestSinkNodeU32_2 {
    type Error = core::convert::Infallible;

    #[inline]
    fn open(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    #[inline]
    fn consume(&mut self, _port: usize, msg: Message<u32>) -> Result<(), Self::Error> {
        #[cfg(feature = "std")]
        let mut random_seed: u32 = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_else(|e| e.duration());
            (now.as_nanos() & 0xFFFF_FFFF) as u32
        };
        #[cfg(not(feature = "std"))]
        let mut random_seed = 1;
        random_test_node_delay(&mut random_seed, 100);

        let mut buf: FixedBuf<256> = FixedBuf::new();
        let _ = core::write!(&mut buf, "{:?}", msg);
        (self.printer)(buf.as_str());

        Ok(())
    }

    #[inline]
    fn input_acceptance(&self) -> [PlacementAcceptance; 1] {
        self.input_placement_acceptance
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
