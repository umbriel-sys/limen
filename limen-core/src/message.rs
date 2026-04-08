//! Message header, flags, and typed message wrapper.
//!
//! Every value in the graph is a [`Message<P>`] carrying a fixed
//! [`MessageHeader`] and a generic payload `P: Payload`.
//!
//! - [`MessageHeader`] — trace ID, sequence number, creation tick, optional
//!   deadline, QoS class, payload size, flags, and memory class.
//! - [`MessageFlags`] — compact bitfield for batch boundary and degrade hints.
//! - [`Message<P>`] — the header/payload pair; implements [`Payload`] itself
//!   so batches of messages can be nested.
//!
//! Submodules:
//! - [`payload`] — the [`Payload`] trait and blanket impls for slices/arrays/scalars.
//! - [`tensor`] — owned, fixed-capacity, `no_std`/`no_alloc` [`Tensor`](tensor::Tensor) type.
//! - [`batch`] — [`Batch`](batch::Batch) view, [`BatchView`](batch::BatchView) container, and [`BatchMessageIter`](batch::BatchMessageIter).

pub mod batch;
pub mod payload;
pub mod tensor;

use core::mem;

use crate::memory::{BufferDescriptor, MemoryClass};
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
    pub const fn bits(&self) -> &u32 {
        &self.0
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
        self.payload_size_bytes = *desc.bytes();
    }

    /// Return the trace id.
    #[inline]
    pub const fn trace_id(&self) -> &TraceId {
        &self.trace_id
    }

    /// Set the trace id.
    #[inline]
    pub fn set_trace_id(&mut self, trace_id: TraceId) {
        self.trace_id = trace_id;
    }

    /// Return the sequence number.
    #[inline]
    pub const fn sequence(&self) -> &SequenceNumber {
        &self.sequence
    }

    /// Set the sequence number.
    #[inline]
    pub fn set_sequence(&mut self, sequence: SequenceNumber) {
        self.sequence = sequence;
    }

    /// Return the creation tick.
    #[inline]
    pub const fn creation_tick(&self) -> &Ticks {
        &self.creation_tick
    }

    /// Set the creation tick.
    #[inline]
    pub fn set_creation_tick(&mut self, creation_tick: Ticks) {
        self.creation_tick = creation_tick;
    }

    /// Return the optional deadline.
    #[inline]
    pub const fn deadline_ns(&self) -> &Option<DeadlineNs> {
        &self.deadline_ns
    }

    /// Set the optional deadline in nanoseconds since boot.
    #[inline]
    pub fn set_deadline_ns(&mut self, deadline_ns: Option<DeadlineNs>) {
        self.deadline_ns = deadline_ns;
    }

    /// Return the QoS class.
    #[inline]
    pub const fn qos(&self) -> &QoSClass {
        &self.qos
    }

    /// Set the QoS class.
    #[inline]
    pub fn set_qos(&mut self, qos: QoSClass) {
        self.qos = qos;
    }

    /// Return the payload size in bytes.
    #[inline]
    pub const fn payload_size_bytes(&self) -> &usize {
        &self.payload_size_bytes
    }

    /// Set the payload size in bytes.
    #[inline]
    pub fn set_payload_size_bytes(&mut self, payload_size_bytes: usize) {
        self.payload_size_bytes = payload_size_bytes;
    }

    /// Return the message flags.
    #[inline]
    pub const fn flags(&self) -> &MessageFlags {
        &self.flags
    }

    /// Set the message flags.
    #[inline]
    pub fn set_flags(&mut self, flags: MessageFlags) {
        self.flags = flags;
    }

    /// Return the memory class.
    #[inline]
    pub const fn memory_class(&self) -> &MemoryClass {
        &self.memory_class
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
        header.payload_size_bytes = *desc.bytes();
        Self { header, payload }
    }

    /// Swap payloads while recalculating header fields.
    #[inline]
    pub fn with_payload<Q: Payload>(self, payload: Q) -> Message<Q> {
        let mut header = self.header;
        let desc = payload.buffer_descriptor();
        header.payload_size_bytes = *desc.bytes();
        Message { header, payload }
    }

    /// Transform payloads while preserving header metadata correctly.
    #[inline]
    pub fn map_payload<Q: Payload>(self, f: impl FnOnce(P) -> Q) -> Message<Q> {
        let Message {
            mut header,
            payload,
        } = self;

        let new_payload = f(payload);
        let desc = new_payload.buffer_descriptor();
        header.payload_size_bytes = *desc.bytes();

        Message {
            header,
            payload: new_payload,
        }
    }

    /// Borrow the payload.
    #[inline]
    pub fn payload(&self) -> &P {
        &self.payload
    }

    /// Mutable borrow of the payload.
    #[inline]
    pub fn payload_mut(&mut self) -> &mut P {
        &mut self.payload
    }

    /// Borrow the header.
    #[inline]
    pub fn header(&self) -> &MessageHeader {
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

impl<P: Payload> Payload for Message<P> {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        let payload_desc = self.payload.buffer_descriptor();
        // Add header size to the payload byte size, keep the payload memory class.
        BufferDescriptor::new(*payload_desc.bytes() + mem::size_of::<MessageHeader>())
    }
}

// Also useful: implement for borrowed Message references to match other impls above.
impl<'a, P: Payload + 'a> Payload for &'a Message<P> {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        let payload_desc = self.payload.buffer_descriptor();
        BufferDescriptor::new(*payload_desc.bytes() + mem::size_of::<MessageHeader>())
    }
}

impl<P: Payload + Clone + Default> Default for Message<P> {
    /// Default `Message<P>` constructed from an empty header and `P::default()`.
    fn default() -> Self {
        Message::new(MessageHeader::empty(), P::default())
    }
}
