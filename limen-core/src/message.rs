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
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Set a flag bit.
    pub const fn with(mut self, bit: u32) -> Self {
        self.0 |= bit;
        self
    }

    /// Check whether a flag bit is set.
    pub const fn contains(&self, bit: u32) -> bool {
        (self.0 & bit) != 0
    }

    /// Return the raw flag bits.
    pub const fn bits(&self) -> u32 {
        self.0
    }
}

/// Fixed header present on all messages that traverse the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHeader {
    /// Correlation identifier for tracing across nodes.
    pub trace_id: TraceId,
    /// Monotonic sequence number assigned by producers/routers.
    pub sequence: SequenceNumber,
    /// Creation tick (monotonic; platform-defined units).
    pub creation_tick: Ticks,
    /// Optional absolute deadline in nanoseconds since boot (P2).
    pub deadline_ns: Option<DeadlineNs>,
    /// QoS class used by admission/scheduling.
    pub qos: QoSClass,
    /// Reported payload size (bytes), used for byte-cap admission.
    pub payload_size_bytes: usize,
    /// Message flags (batch boundaries, degrade hints).
    pub flags: MessageFlags,
    /// Memory class where the payload currently resides.
    pub memory_class: MemoryClass,
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
}

/// A message with a generic payload `P`.
#[derive(Debug, Clone, Copy)]
pub struct Message<P: Payload> {
    /// The header fields.
    pub header: MessageHeader,
    /// The payload object or view.
    pub payload: P,
}

impl<P: Payload> Message<P> {
    /// Construct a new message from a header and payload, fixing size and class.
    pub fn new(mut header: MessageHeader, payload: P) -> Self {
        let desc = payload.buffer_descriptor();
        header.payload_size_bytes = desc.bytes;
        header.memory_class = desc.class;
        Self { header, payload }
    }
}

/// A thin batch view over a slice of messages.
///
/// Batch formation is runtime-specific; the core only provides
/// a convenient immutable view for policies and telemetry.
#[derive(Debug, Copy, Clone)]
pub struct Batch<'a, P: Payload> {
    /// The ordered messages in the batch.
    pub messages: &'a [Message<P>],
}

impl<'a, P: Payload> Batch<'a, P> {
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
}
