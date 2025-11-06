//! Edge graph-link and descriptor types.

use crate::{
    edge::{Edge, EdgeOccupancy, EnqueueResult},
    errors::QueueError,
    message::{payload::Payload, Message},
    policy::EdgePolicy,
    types::{EdgeIndex, PortId},
};

/// A lightweight descriptor that **links to** the concrete queue instance
/// backing a graph edge, along with its routing and policy metadata.
///
/// Unlike a pure descriptor, `EdgeLink` **borrows** (`&'a mut`) the queue
/// implementation. This keeps it zero-alloc and allows direct, policy-aware
/// operations on the buffer.
///
/// - `'a`: lifetime of the borrowed queue
/// - `Q`: concrete queue type implementing `SpscQueue<Item = Message<P>>`
/// - `P`: message payload type
#[derive(Debug)]
pub struct EdgeLink<Q, P>
where
    P: Payload,
    Q: Edge<Item = Message<P>>,
{
    /// Borrowed handle to the concrete queue instance for this edge.
    queue: Q,

    /// Unique identifier of this edge in the graph.
    pub id: EdgeIndex,

    /// Upstream node's output port.
    upstream_port: PortId,

    /// Downstream node's input port.
    downstream_port: PortId,

    /// Admission and scheduling policy applied to this edge.
    policy: EdgePolicy,

    /// Optional static name used for diagnostics or graph tooling.
    name: Option<&'static str>,

    /// Marker to bind `P` without storing it.
    _payload_marker: core::marker::PhantomData<P>,
}

impl<Q, P> EdgeLink<Q, P>
where
    P: Payload,
    Q: Edge<Item = Message<P>>,
{
    /// Construct a new `EdgeLink` that borrows the given queue and records its metadata.
    #[inline]
    pub fn new(
        queue: Q,
        id: EdgeIndex,
        upstream_port: PortId,
        downstream_port: PortId,
        policy: EdgePolicy,
        name: Option<&'static str>,
    ) -> Self {
        Self {
            queue,
            id,
            upstream_port,
            downstream_port,
            policy,
            name,
            _payload_marker: core::marker::PhantomData,
        }
    }

    /// Get a reference to the inner queue.
    #[inline]
    pub fn queue(&self) -> &Q {
        &self.queue
    }

    /// Get a mutable reference to the inner queue
    #[inline]
    pub fn queue_mut(&mut self) -> &mut Q {
        &mut self.queue
    }

    /// Get the unique identifier of this edge.
    #[inline]
    pub fn id(&self) -> EdgeIndex {
        self.id
    }

    /// Get the upstream output port index.
    #[inline]
    pub fn upstream_port(&self) -> PortId {
        self.upstream_port
    }

    /// Get the downstream input port index.
    #[inline]
    pub fn downstream_port(&self) -> PortId {
        self.downstream_port
    }

    /// Get the edge policy applied to this queue.
    #[inline]
    pub fn policy(&self) -> EdgePolicy {
        self.policy
    }

    /// Get the optional static name of this queue link.
    #[inline]
    pub fn name(&self) -> Option<&'static str> {
        self.name
    }

    /// Return the `EdgeDescriptor` for this `EdgeLink`.
    #[inline]
    pub fn descriptor(&self) -> EdgeDescriptor {
        EdgeDescriptor {
            id: self.id(),
            upstream: self.upstream_port(),
            downstream: self.downstream_port(),
            name: self.name(),
        }
    }
}

impl<Q, P> Edge for EdgeLink<Q, P>
where
    P: Payload,
    Q: Edge<Item = Message<P>>,
{
    type Item = Message<P>;

    #[inline]
    fn try_push(&mut self, item: Self::Item, policy: &EdgePolicy) -> EnqueueResult {
        self.queue.try_push(item, policy)
    }

    #[inline]
    fn try_pop(&mut self) -> Result<Self::Item, QueueError> {
        self.queue.try_pop()
    }

    #[inline]
    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        self.queue.occupancy(policy)
    }

    #[inline]
    fn try_peek(&self) -> Result<&Self::Item, QueueError> {
        self.queue.try_peek()
    }
}

/// An edge couples one output port to one input port with an admission policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeDescriptor {
    /// Unique identifier of this edge in the graph.
    pub id: EdgeIndex,
    /// Identifier of the upstream node / port.
    pub upstream: PortId,
    /// Identifier of the downstream node / port.
    pub downstream: PortId,
    /// Optional static name (for diagnostics or graph tooling).
    pub name: Option<&'static str>,
}

/// Std-only, owning edge link that stores the concrete queue behind an
/// `Arc<Mutex<Q>>` plus static routing/policy metadata.
///
/// This is the thread-safe sibling of `EdgeLink`: instead of borrowing a queue,
/// it **owns** it in an `Arc<Mutex<_>>` so concurrent runtimes can hand out
/// cloned, thread-safe endpoints to worker threads.
#[cfg(feature = "std")]
#[derive(Debug)]
pub struct ConcurrentEdgeLink<Q, P>
where
    P: Payload,
    Q: Edge<Item = Message<P>>,
{
    /// Shared, thread-safe buffer for this edge.
    buf: std::sync::Arc<std::sync::Mutex<Q>>,
    /// Unique identifier for this edge in the graph.
    id: EdgeIndex,
    /// Upstream node/port (producer).
    upstream: PortId,
    /// Downstream node/port (consumer).
    downstream: PortId,
    /// Admission/scheduling policy for this edge.
    policy: EdgePolicy,
    /// Optional static name for diagnostics/tooling.
    name: Option<&'static str>,
    /// Bind payload type at the type level without storing it.
    _marker: core::marker::PhantomData<P>,
}

#[cfg(feature = "std")]
impl<Q, P> ConcurrentEdgeLink<Q, P>
where
    P: Payload,
    Q: Edge<Item = Message<P>>,
{
    /// Create from a concrete queue and edge metadata.
    ///
    /// The queue is placed behind an `Arc<Mutex<_>>` so callers can construct
    /// concurrent producer/consumer endpoints that share the same buffer.
    #[inline]
    pub fn new(
        queue: Q,
        id: EdgeIndex,
        upstream: PortId,
        downstream: PortId,
        policy: EdgePolicy,
        name: Option<&'static str>,
    ) -> Self {
        Self {
            buf: std::sync::Arc::new(std::sync::Mutex::new(queue)),
            id,
            upstream,
            downstream,
            policy,
            name,
            _marker: core::marker::PhantomData,
        }
    }

    /// Return a lightweight descriptor for this edge (IDs/ports/name).
    #[inline]
    pub fn descriptor(&self) -> EdgeDescriptor {
        EdgeDescriptor {
            id: self.id,
            upstream: self.upstream,
            downstream: self.downstream,
            name: self.name,
        }
    }

    /// Get the edge policy (admission/watermarks/over-budget behavior).
    #[inline]
    pub fn policy(&self) -> EdgePolicy {
        self.policy
    }

    /// Access the shared `Arc<Mutex<Q>>` to build thread-safe endpoints.
    #[inline]
    pub fn arc(&self) -> std::sync::Arc<std::sync::Mutex<Q>> {
        std::sync::Arc::clone(&self.buf)
    }
}

#[cfg(feature = "std")]
impl<Q, P> crate::edge::Edge for ConcurrentEdgeLink<Q, P>
where
    P: crate::message::payload::Payload + Clone,
    Q: crate::edge::Edge<Item = crate::message::Message<P>> + Send + 'static,
{
    type Item = crate::message::Message<P>;

    #[inline]
    fn try_push(
        &mut self,
        item: Self::Item,
        policy: &crate::policy::EdgePolicy,
    ) -> crate::edge::EnqueueResult {
        match self.buf.lock() {
            Ok(mut q) => q.try_push(item, policy),
            Err(_) => crate::edge::EnqueueResult::Rejected,
        }
    }

    #[inline]
    fn try_pop(&mut self) -> Result<Self::Item, crate::errors::QueueError> {
        match self.buf.lock() {
            Ok(mut q) => q.try_pop(),
            Err(_) => Err(crate::errors::QueueError::Poisoned),
        }
    }

    #[inline]
    fn occupancy(&self, policy: &crate::policy::EdgePolicy) -> crate::edge::EdgeOccupancy {
        match self.buf.lock() {
            Ok(q) => q.occupancy(policy),
            Err(_) => crate::edge::EdgeOccupancy {
                items: 0,
                bytes: 0,
                watermark: crate::policy::WatermarkState::AtOrAboveHard,
            },
        }
    }

    // Cannot return `&Item` through a Mutex guard.
    #[inline]
    fn try_peek(&self) -> Result<&Self::Item, crate::errors::QueueError> {
        Err(crate::errors::QueueError::Empty)
    }

    #[cfg(feature = "std")]
    #[inline]
    fn try_peek_cloned(&self) -> Result<Self::Item, crate::errors::QueueError> {
        match self.buf.lock() {
            Ok(q) => q.try_peek_cloned(),
            Err(_) => Err(crate::errors::QueueError::Poisoned),
        }
    }
}
