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
use crate::types::{DeadlineNs, QoSClass, SequenceNumber, TraceId};

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
pub struct TestCounterSourceU32_2<Clock, const BACKLOG_CAP: usize>
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
    ingress_policy: EdgePolicy,

    // ---- Upstream pressure modelling ----
    // Layout: circular buffer with head index (oldest) and len (number of items).
    // Capacity is small and fixed for tests.
    backlog: [Option<Message<u32>>; BACKLOG_CAP],
    backlog_head: usize,
    backlog_len: usize,
    backlog_bytes: usize,

    // std optional shared probe (if present, it is authoritative).
    #[cfg(feature = "std")]
    ingress_probe: Option<SourceIngressProbe>,
    #[cfg(feature = "std")]
    ingress_updater: Option<SourceIngressUpdater>,
}

impl<Clock, const BACKLOG_CAP: usize> TestCounterSourceU32_2<Clock, BACKLOG_CAP>
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
        ingress_policy: EdgePolicy,
    ) -> Self {
        // Hard check: backlog capacity must be at least the ingress edge max_items.
        // Use a plain `panic!` with a string literal so this is `const`-friendly.
        if BACKLOG_CAP < ingress_policy.caps.max_items {
            panic!(
                "TestCounterSourceU32_2: backlog capacity must be >= ingress_policy.caps.max_items"
            );
        }

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
            ingress_policy,
            backlog: [None; BACKLOG_CAP],
            backlog_head: 0usize,
            backlog_len: 0usize,
            backlog_bytes: 0usize,
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

    #[inline]
    fn make_message(&self) -> Message<u32> {
        Message::new(
            MessageHeader::new(
                self.trace_id,
                self.next_sequence,
                self.clock.now_ticks(),
                self.deadline_ns,
                self.qos,
                core::mem::size_of::<u32>(),
                self.flags,
                MemoryClass::Host,
            ),
            self.next_value_to_emit,
        )
    }

    /// Set a synthetic upstream backlog (items).
    #[inline]
    pub fn produce_n_items_in_backlog(&mut self, n: usize) {
        // Append up to `n` synthetic items into the ring backlog without
        // clearing existing items. If capacity reached, stop appending.
        let mut to_add = n;
        while to_add > 0 && self.backlog_len < BACKLOG_CAP {
            let tail = (self.backlog_head + self.backlog_len) % BACKLOG_CAP;
            self.backlog[tail] = Some(self.make_message());

            self.backlog_len += 1;
            to_add = to_add.saturating_sub(1);
            self.backlog_bytes = self.backlog_len * core::mem::size_of::<u32>();

            self.next_value_to_emit = self.next_value_to_emit.wrapping_add(1);
            self.next_sequence = SequenceNumber::new(self.next_sequence.as_u64().wrapping_add(1));
        }
    }

    /// Pop the oldest message from the software backlog (ring), if any.
    ///
    /// Returns `None` if the backlog is empty. This is destructive: it removes the
    /// oldest item and advances the ring head.
    #[inline]
    fn try_pop_from_backlog(&mut self) -> Option<Message<u32>> {
        if self.backlog_len == 0 {
            return None;
        }

        let head_index = self.backlog_head;

        // Remove the oldest entry.
        let message = self.backlog[head_index].take();

        // Advance head and shrink length.
        self.backlog_head = (self.backlog_head + 1) % BACKLOG_CAP;
        self.backlog_len = self.backlog_len.saturating_sub(1);

        // Keep bytes consistent with counters.
        self.backlog_bytes = self.backlog_len * core::mem::size_of::<u32>();

        message
    }

    #[inline]
    fn random_backlog_add_count(&self) -> usize {
        // Use the platform clock tick parity as a cheap jitter source.
        // Even -> add 1, odd -> add 2.
        let now_ticks_u64 = *self.clock.now_ticks().as_u64();
        if (now_ticks_u64 & 1) == 0 {
            1
        } else {
            2
        }
    }
}

impl<Clock, const BACKLOG_CAP: usize> Source<u32, 1> for TestCounterSourceU32_2<Clock, BACKLOG_CAP>
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
        // Random test delay.
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

        // Random ingress pressure update.
        self.produce_n_items_in_backlog(self.random_backlog_add_count());

        // Pop and send message.
        self.try_pop_from_backlog().map(|message| (0, message))
    }

    #[inline]
    fn ingress_occupancy(&self) -> EdgeOccupancy {
        #[cfg(feature = "std")]
        if let Some(probe) = &self.ingress_probe {
            return probe.occupancy(&self.ingress_policy());
        }

        // Fallback to software backlog counters.
        let items = self.backlog_len;
        let bytes = self.backlog_bytes;
        EdgeOccupancy::new(items, bytes, self.ingress_policy.watermark(items, bytes))
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

    fn ingress_policy(&self) -> EdgePolicy {
        self.ingress_policy
    }

    /// Peek the creation tick of the `item_index`'th ingress item (0 = oldest).
    /// Non-blocking and non-destructive. Returns `None` if metadata is not
    /// available (no backlog) or `item_index` is out of range.
    #[inline]
    fn peek_ingress_creation_tick(&self, item_index: usize) -> Option<u64> {
        // If no backlog, nothing to peek.
        if (self.backlog_len == 0) || (item_index >= self.backlog_len) {
            return None;
        }

        Some(
            *self.backlog[item_index]
                .unwrap()
                .header()
                .creation_tick()
                .as_u64(),
        )
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
        ModelMetadata::new(MemoryClass::Host, MemoryClass::Host, None, None)
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
        BackendCapabilities::new(false, Some(usize::MAX), 0)
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
    fn consume(&mut self, msg: &Message<u32>) -> Result<(), Self::Error> {
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
