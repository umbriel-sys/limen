//! Message header and payload contracts.
//!
//! The payload type is generic and can be any type that implements
//! [`Payload`]. For simple byte buffers, `&[u8]` or fixed-size arrays can
//! be used directly because `Payload` is implemented for them.

pub mod payload;
pub mod tensor;

use crate::memory::MemoryClass;
use crate::message::payload::Payload;
use crate::types::{DeadlineNs, QoSClass, SequenceNumber, Ticks, TraceId};

/// A compact bitfield of message flags.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageFlags(u32);

impl MessageFlags {
    /// Flag: this message is the first element in a batch.
    pub const FIRST_IN_BATCH: u32 = 1 << 0;
    /// Flag: this message is the last element in a batch.
    pub const LAST_IN_BATCH: u32 = 1 << 1;
    /// Flag: downstream may degrade this message (e.g., fast/low-precision path).
    pub const DEGRADE_ALLOWED: u32 = 1 << 2;

    /// Create an empty flag set.
    #[inline]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Construct from raw bits (advanced).
    #[inline]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Return the raw flag bits.
    #[inline]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Set a flag bit.
    #[inline]
    pub const fn with(self, bit: u32) -> Self {
        Self(self.0 | bit)
    }

    /// Clear a flag bit.
    #[inline]
    pub const fn without(self, bit: u32) -> Self {
        Self(self.0 & !bit)
    }

    /// Check whether a flag bit is set.
    #[inline]
    pub const fn contains(self, bit: u32) -> bool {
        (self.0 & bit) != 0
    }

    // Typed helpers (readable call sites, avoid repeating bit constants).

    /// Return a copy with `FIRST_IN_BATCH` set.
    #[inline]
    pub const fn first_in_batch(self) -> Self {
        self.with(Self::FIRST_IN_BATCH)
    }

    /// Return a copy with `LAST_IN_BATCH` set.
    #[inline]
    pub const fn last_in_batch(self) -> Self {
        self.with(Self::LAST_IN_BATCH)
    }

    /// Return a copy with `DEGRADE_ALLOWED` set.
    #[inline]
    pub const fn allow_degrade(self) -> Self {
        self.with(Self::DEGRADE_ALLOWED)
    }

    /// `true` if `FIRST_IN_BATCH` is set.
    #[inline]
    pub const fn is_first(self) -> bool {
        self.contains(Self::FIRST_IN_BATCH)
    }

    /// `true` if `LAST_IN_BATCH` is set.
    #[inline]
    pub const fn is_last(self) -> bool {
        self.contains(Self::LAST_IN_BATCH)
    }

    /// `true` if `DEGRADE_ALLOWED` is set.
    #[inline]
    pub const fn can_degrade(self) -> bool {
        self.contains(Self::DEGRADE_ALLOWED)
    }
}

/// Fixed header present on all messages that traverse the runtime.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHeader {
    /// Correlation identifier for tracing across nodes.
    trace_id: TraceId,
    /// Monotonic sequence number assigned by producers/routers.
    sequence: SequenceNumber,
    /// Creation tick (monotonic; platform-defined units).
    creation_tick: Ticks,
    /// Optional absolute deadline in nanoseconds since boot (P2).
    deadline_ns: Option<DeadlineNs>,
    /// QoS class used by admission/scheduling.
    qos: QoSClass,
    /// Reported payload size (bytes), used for byte-cap admission.
    payload_size_bytes: usize,
    /// Message flags (batch boundaries, degrade hints).
    flags: MessageFlags,
    /// Memory class where the payload currently resides.
    memory_class: MemoryClass,
}

impl MessageHeader {
    /// Construct a new header.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        trace_id: TraceId,
        sequence: SequenceNumber,
        creation_tick: Ticks,
        deadline_ns: Option<DeadlineNs>,
        qos: QoSClass,
        payload_size_bytes: usize,
        flags: MessageFlags,
        memory_class: MemoryClass,
    ) -> Self {
        Self {
            trace_id,
            sequence,
            creation_tick,
            deadline_ns,
            qos,
            payload_size_bytes,
            flags,
            memory_class,
        }
    }

    /// A zero/identity header (safe for scratch use).
    #[inline]
    pub const fn empty() -> Self {
        Self {
            trace_id: TraceId::new(0),
            sequence: SequenceNumber::new(0),
            creation_tick: Ticks::new(0),
            deadline_ns: None,
            qos: QoSClass::BestEffort,
            payload_size_bytes: 0,
            flags: MessageFlags::empty(),
            memory_class: MemoryClass::Host,
        }
    }

    /// Returns true is the message header is empty.
    #[inline]
    pub fn is_empty(self) -> bool {
        self == Self::empty()
    }

    /// Update `payload_size_bytes` and `memory_class` from a payload descriptor.
    #[inline]
    pub fn sync_from_payload<P: Payload>(&mut self, payload: &P) {
        let desc = payload.buffer_descriptor();
        self.payload_size_bytes = desc.bytes();
        self.memory_class = desc.class();
    }

    /// Return the trace id.
    #[inline]
    pub const fn trace_id(&self) -> TraceId {
        self.trace_id
    }

    /// Set the trace id.
    #[inline]
    pub fn set_trace_id(&mut self, trace_id: TraceId) {
        self.trace_id = trace_id;
    }

    /// Return the sequence number.
    #[inline]
    pub const fn sequence(&self) -> SequenceNumber {
        self.sequence
    }

    /// Set the sequence number.
    #[inline]
    pub fn set_sequence(&mut self, sequence: SequenceNumber) {
        self.sequence = sequence;
    }

    /// Return the creation tick.
    #[inline]
    pub const fn creation_tick(&self) -> Ticks {
        self.creation_tick
    }

    /// Set the creation tick.
    #[inline]
    pub fn set_creation_tick(&mut self, creation_tick: Ticks) {
        self.creation_tick = creation_tick;
    }

    /// Return the optional deadline.
    #[inline]
    pub const fn deadline_ns(&self) -> Option<DeadlineNs> {
        self.deadline_ns
    }

    /// Set the optional deadline in nanoseconds since boot.
    #[inline]
    pub fn set_deadline_ns(&mut self, deadline_ns: Option<DeadlineNs>) {
        self.deadline_ns = deadline_ns;
    }

    /// Return the QoS class.
    #[inline]
    pub const fn qos(&self) -> QoSClass {
        self.qos
    }

    /// Set the QoS class.
    #[inline]
    pub fn set_qos(&mut self, qos: QoSClass) {
        self.qos = qos;
    }

    /// Return the payload size in bytes.
    #[inline]
    pub const fn payload_size_bytes(&self) -> usize {
        self.payload_size_bytes
    }

    /// Set the payload size in bytes.
    #[inline]
    pub fn set_payload_size_bytes(&mut self, payload_size_bytes: usize) {
        self.payload_size_bytes = payload_size_bytes;
    }

    /// Return the message flags.
    #[inline]
    pub const fn flags(&self) -> MessageFlags {
        self.flags
    }

    /// Set the message flags.
    #[inline]
    pub fn set_flags(&mut self, flags: MessageFlags) {
        self.flags = flags;
    }

    /// Return the memory class.
    #[inline]
    pub const fn memory_class(&self) -> MemoryClass {
        self.memory_class
    }

    /// Set the memory class.
    #[inline]
    pub fn set_memory_class(&mut self, memory_class: MemoryClass) {
        self.memory_class = memory_class;
    }

    /// Mark this header as the first element in a batch by setting `FIRST_IN_BATCH`.
    #[inline]
    pub fn set_first_in_batch(&mut self) {
        self.flags = self.flags.first_in_batch();
    }

    /// Mark this header as the last element in a batch by setting `LAST_IN_BATCH`.
    #[inline]
    pub fn set_last_in_batch(&mut self) {
        self.flags = self.flags.last_in_batch();
    }
}

impl Default for MessageHeader {
    #[inline]
    fn default() -> Self {
        Self::empty()
    }
}

/// A message with a generic payload `P`.
#[derive(Debug, Clone)]
pub struct Message<P: Payload> {
    /// The header fields.
    header: MessageHeader,
    /// The payload object or view.
    payload: P,
}

// Copy only when the payload is Copy (e.g., TensorRef<'a>).
impl<P> Copy for Message<P> where P: Payload + Copy {}

impl<P: Payload> Message<P> {
    /// Construct a new message from a header and payload, fixing size and class.
    pub fn new(mut header: MessageHeader, payload: P) -> Self {
        let desc = payload.buffer_descriptor();
        header.payload_size_bytes = desc.bytes();
        header.memory_class = desc.class();
        Self { header, payload }
    }

    /// Swap payloads while recalculating header fields.
    #[inline]
    pub fn with_payload<Q: Payload>(self, payload: Q) -> Message<Q> {
        let mut header = self.header;
        let desc = payload.buffer_descriptor();
        header.payload_size_bytes = desc.bytes();
        header.memory_class = desc.class();
        Message { header, payload }
    }

    /// Transform payloads while preserving header metadata correctly.
    #[inline]
    pub fn map_payload<Q: Payload>(self, f: impl FnOnce(P) -> Q) -> Message<Q> {
        // Move out of `self` explicitly.
        let Message {
            mut header,
            payload,
        } = self;

        // Produce the new payload by consuming the old one.
        let new_payload = f(payload);

        // Recompute size and placement from the new payload.
        let desc = new_payload.buffer_descriptor();
        header.payload_size_bytes = desc.bytes();
        header.memory_class = desc.class();

        Message {
            header,
            payload: new_payload,
        }
    }

    /// Return the payload.
    #[inline]
    pub fn payload(self) -> P {
        self.payload
    }

    /// Borrow the payload.
    #[inline]
    pub fn payload_ref(&self) -> &P {
        &self.payload
    }

    /// Mutable borrow of the payload.
    #[inline]
    pub fn payload_mut(&mut self) -> &mut P {
        &mut self.payload
    }

    /// Return the header.
    #[inline]
    pub fn header(self) -> MessageHeader {
        self.header
    }

    /// Borrow the header.
    #[inline]
    pub fn header_ref(&self) -> &MessageHeader {
        &self.header
    }

    /// Mutable borrow of the header.
    #[inline]
    pub fn header_mut(&mut self) -> &mut MessageHeader {
        &mut self.header
    }

    /// Decompose into `(header, payload)`.
    #[inline]
    pub fn into_parts(self) -> (MessageHeader, P) {
        (self.header, self.payload)
    }
}

/// A thin batch view over a slice of messages.
///
/// Batch formation is runtime-specific; the core only provides
/// a convenient immutable view for policies and telemetry.
#[derive(Debug, Copy, Clone)]
pub struct Batch<'a, P: Payload> {
    /// The ordered messages in the batch.
    messages: &'a [Message<P>],
}

impl<'a, P: Payload> Batch<'a, P> {
    /// Construct a new batch view over a slice of messages.
    #[inline]
    pub const fn new(messages: &'a [Message<P>]) -> Self {
        Self { messages }
    }

    /// Return the underlying messages slice.
    #[inline]
    pub fn messages(&self) -> &'a [Message<P>] {
        self.messages
    }

    /// Return the number of messages in the batch.
    #[inline]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Return `true` if the batch is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Total byte size across message payloads.
    pub fn total_payload_bytes(&self) -> usize {
        self.messages
            .iter()
            .map(|m| m.header.payload_size_bytes)
            .sum()
    }

    /// Iterate over messages.
    #[inline]
    pub fn iter(&self) -> core::slice::Iter<'_, Message<P>> {
        self.messages.iter()
    }

    /// Convenience: is the first message marked FIRST_IN_BATCH (if present)?
    #[inline]
    pub fn first_flagged(&self) -> bool {
        self.messages
            .first()
            .map(|m| m.header.flags.is_first())
            .unwrap_or(false)
    }

    /// Convenience: is the last message marked LAST_IN_BATCH (if present)?
    #[inline]
    pub fn last_flagged(&self) -> bool {
        self.messages
            .last()
            .map(|m| m.header.flags.is_last())
            .unwrap_or(false)
    }

    /// (Optional) Validate flags are consistent with batch boundaries.
    /// Enable only when you want assertions (e.g., in tests) via a feature flag.
    // #[cfg(feature = "validate_batches")]
    #[inline]
    pub fn assert_flags_consistent(&self) {
        if self.is_empty() {
            return;
        }
        debug_assert!(
            self.first_flagged(),
            "batch: first item missing FIRST_IN_BATCH"
        );
        debug_assert!(
            self.last_flagged(),
            "batch: last item missing LAST_IN_BATCH"
        );
        // Optional: internal items should have neither FIRST nor LAST
        for m in &self.messages[1..self.messages.len().saturating_sub(1)] {
            debug_assert!(
                !m.header.flags.is_first() && !m.header.flags.is_last(),
                "batch: internal item has boundary flag"
            );
        }
    }
}
