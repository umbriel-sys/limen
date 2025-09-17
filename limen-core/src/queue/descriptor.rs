//! Edge descriptor types.

use crate::{
    message::{payload::Payload, Message},
    policy::EdgePolicy,
    queue::SpscQueue,
    types::{EdgeIndex, PortId},
};

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
pub struct EdgeLink<'a, Q, P>
where
    P: Payload,
    Q: SpscQueue<Item = Message<P>>,
{
    /// Borrowed handle to the concrete queue instance for this edge.
    //
    // TODO: Remove ref?
    queue: &'a mut Q,

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

impl<'a, Q, P> EdgeLink<'a, Q, P>
where
    P: Payload,
    Q: SpscQueue<Item = Message<P>>,
{
    /// Construct a new `EdgeLink` that borrows the given queue and records its metadata.
    #[inline]
    pub fn new(
        queue: &'a mut Q,
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

    /// Get a mutable reference to the underlying queue.
    #[inline]
    pub fn queue(&mut self) -> &mut Q {
        self.queue
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
