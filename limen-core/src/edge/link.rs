//! Edge graph-link and descriptor types.

use crate::{
    edge::{Edge, EdgeOccupancy, EnqueueResult},
    errors::QueueError,
    policy::EdgePolicy,
    prelude::{BatchView, HeaderStore},
    types::{EdgeIndex, MessageToken, PortId},
};

/// A lightweight descriptor that **links to** the concrete queue instance
/// backing a graph edge, along with its routing and policy metadata.
///
/// Unlike a pure descriptor, `EdgeLink` **owns** the queue
/// implementation. This keeps it zero-alloc and allows direct, policy-aware
/// operations on the buffer.
///
/// - `Q`: concrete queue type implementing `Edge`
#[non_exhaustive]
#[derive(Debug)]
pub struct EdgeLink<Q>
where
    Q: Edge,
{
    /// Owned handle to the concrete queue instance for this edge.
    queue: Q,

    /// Unique identifier of this edge in the graph.
    id: EdgeIndex,

    /// Upstream node's output port.
    upstream_port: PortId,

    /// Downstream node's input port.
    downstream_port: PortId,

    /// Admission and scheduling policy applied to this edge.
    policy: EdgePolicy,

    /// Optional static name used for diagnostics or graph tooling.
    name: Option<&'static str>,
}

impl<Q> EdgeLink<Q>
where
    Q: Edge,
{
    /// Construct a new `EdgeLink` that owns the given queue and records its metadata.
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
        }
    }

    /// Get a reference to the inner queue.
    #[inline]
    pub fn queue(&self) -> &Q {
        &self.queue
    }

    /// Get a mutable reference to the inner queue.
    #[inline]
    pub fn queue_mut(&mut self) -> &mut Q {
        &mut self.queue
    }

    /// Get the unique identifier of this edge.
    #[inline]
    pub fn id(&self) -> &EdgeIndex {
        &self.id
    }

    /// Get the upstream output port index.
    #[inline]
    pub fn upstream_port(&self) -> &PortId {
        &self.upstream_port
    }

    /// Get the downstream input port index.
    #[inline]
    pub fn downstream_port(&self) -> &PortId {
        &self.downstream_port
    }

    /// Get the edge policy applied to this queue.
    #[inline]
    pub fn policy(&self) -> &EdgePolicy {
        &self.policy
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
            id: self.id,
            upstream: self.upstream_port,
            downstream: self.downstream_port,
            name: self.name,
        }
    }
}

impl<Q> Edge for EdgeLink<Q>
where
    Q: Edge,
{
    fn try_push<H: HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult {
        self.queue.try_push(token, policy, headers)
    }

    fn try_pop<H: HeaderStore>(&mut self, headers: &H) -> Result<MessageToken, QueueError> {
        self.queue.try_pop(headers)
    }

    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        self.queue.occupancy(policy)
    }

    fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        self.queue.try_peek()
    }

    fn try_peek_at(&self, index: usize) -> Result<MessageToken, QueueError> {
        self.queue.try_peek_at(index)
    }

    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        policy: &crate::policy::BatchingPolicy,
        headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        self.queue.try_pop_batch(policy, headers)
    }
}

/// An edge couples one output port to one input port with an admission policy.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeDescriptor {
    /// Unique identifier of this edge in the graph.
    id: EdgeIndex,
    /// Identifier of the upstream node / port.
    upstream: PortId,
    /// Identifier of the downstream node / port.
    downstream: PortId,
    /// Optional static name (for diagnostics or graph tooling).
    name: Option<&'static str>,
}

impl EdgeDescriptor {
    /// Construct a new `EdgeDescriptor`.
    #[inline]
    pub fn new(
        id: EdgeIndex,
        upstream: PortId,
        downstream: PortId,
        name: Option<&'static str>,
    ) -> Self {
        Self {
            id,
            upstream,
            downstream,
            name,
        }
    }

    /// Unique identifier of this edge in the graph.
    #[inline]
    pub fn id(&self) -> &EdgeIndex {
        &self.id
    }

    /// Identifier of the upstream node / port.
    #[inline]
    pub fn upstream(&self) -> &PortId {
        &self.upstream
    }

    /// Identifier of the downstream node / port.
    #[inline]
    pub fn downstream(&self) -> &PortId {
        &self.downstream
    }

    /// Optional static name (for diagnostics or graph tooling).
    #[inline]
    pub fn name(&self) -> Option<&'static str> {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::edge::bench::TestSpscRingBuf;
    use crate::edge::{Edge, EnqueueResult};
    use crate::errors::QueueError;
    use crate::memory::manager::MemoryManager;
    use crate::memory::static_manager::StaticMemoryManager;
    use crate::message::{Message, MessageHeader};
    use crate::policy::{AdmissionPolicy, EdgePolicy, OverBudgetAction, QueueCaps};
    use crate::prelude::{create_test_tensor_filled_with, TestTensor};
    use crate::types::{EdgeIndex, NodeIndex, PortId, PortIndex, Ticks};

    const POLICY: EdgePolicy = EdgePolicy::new(
        QueueCaps::new(8, 6, None, None),
        AdmissionPolicy::DropNewest,
        OverBudgetAction::Drop,
    );

    const MGR_DEPTH: usize = 32;

    fn make_msg_tensor(tick: u64) -> Message<TestTensor> {
        let mut h = MessageHeader::empty();
        h.set_creation_tick(Ticks::new(tick));
        Message::new(h, create_test_tensor_filled_with(0))
    }

    fn make_link() -> EdgeLink<TestSpscRingBuf<16>> {
        let queue = TestSpscRingBuf::<16>::new();
        let id = EdgeIndex::new(0);
        let upstream_port = PortId::new(NodeIndex::new(0), PortIndex::new(0));
        let downstream_port = PortId::new(NodeIndex::new(1), PortIndex::new(0));

        EdgeLink::new(
            queue,
            id,
            upstream_port,
            downstream_port,
            POLICY,
            Some("edge:hi"),
        )
    }

    // --- Run the full Edge contract suite against EdgeLink ---
    crate::run_edge_contract_tests!(edge_link_contract, || make_link());

    #[test]
    fn edge_link_metadata_accessors_and_descriptor() {
        let link = make_link();

        assert_eq!(link.id(), &EdgeIndex::new(0));
        assert_eq!(
            link.upstream_port(),
            &PortId::new(NodeIndex::new(0), PortIndex::new(0))
        );
        assert_eq!(
            link.downstream_port(),
            &PortId::new(NodeIndex::new(1), PortIndex::new(0))
        );
        assert_eq!(link.policy(), &POLICY);
        assert_eq!(link.name(), Some("edge:hi"));

        let d = link.descriptor();
        assert_eq!(d.id(), &EdgeIndex::new(0));
        assert_eq!(
            d.upstream(),
            &PortId::new(NodeIndex::new(0), PortIndex::new(0))
        );
        assert_eq!(
            d.downstream(),
            &PortId::new(NodeIndex::new(1), PortIndex::new(0))
        );
        assert_eq!(d.name(), Some("edge:hi"));
    }

    #[test]
    fn edge_link_forwards_to_inner_queue() {
        let mut link = make_link();
        let mut mgr: StaticMemoryManager<TestTensor, MGR_DEPTH> = StaticMemoryManager::new();

        // Push a token via the link.
        let m = make_msg_tensor(42);
        let token = mgr.store(m).expect("store");
        assert_eq!(link.try_push(token, &POLICY, &mgr), EnqueueResult::Enqueued);

        // Peek via the link.
        let peek_token = link.try_peek().expect("peek");
        assert_eq!(peek_token, token);
        let peek_h = mgr.peek_header(peek_token).expect("peek header");
        assert_eq!(*peek_h.creation_tick(), Ticks::new(42));

        // Pop via the link.
        let popped = link.try_pop(&mgr).expect("pop");
        assert_eq!(popped, token);
        let popped_h = mgr.peek_header(popped).expect("popped header");
        assert_eq!(*popped_h.creation_tick(), Ticks::new(42));

        // Back to empty.
        assert!(link.is_empty());
        assert!(matches!(link.try_pop(&mgr), Err(QueueError::Empty)));
    }

    #[test]
    fn edge_link_occupancy_delegates() {
        let mut link = make_link();
        let mut mgr: StaticMemoryManager<TestTensor, MGR_DEPTH> = StaticMemoryManager::new();

        let occ0 = link.occupancy(&POLICY);
        assert_eq!(*occ0.items(), 0usize);

        let t1 = mgr.store(make_msg_tensor(1)).expect("store");
        let t2 = mgr.store(make_msg_tensor(2)).expect("store");
        assert_eq!(link.try_push(t1, &POLICY, &mgr), EnqueueResult::Enqueued);
        assert_eq!(link.try_push(t2, &POLICY, &mgr), EnqueueResult::Enqueued);

        let occ2 = link.occupancy(&POLICY);
        assert_eq!(*occ2.items(), 2usize);
    }

    #[test]
    fn edge_link_queue_accessor() {
        let mut link = make_link();
        let mut mgr: StaticMemoryManager<TestTensor, MGR_DEPTH> = StaticMemoryManager::new();

        // Push via the link, then verify through the inner queue accessor.
        let token = mgr.store(make_msg_tensor(7)).expect("store");
        assert_eq!(link.try_push(token, &POLICY, &mgr), EnqueueResult::Enqueued);

        assert!(!link.queue().is_empty());

        // Pop via inner queue_mut.
        let popped = link.queue_mut().try_pop(&mgr).expect("pop via queue_mut");
        assert_eq!(popped, token);
        assert!(link.queue().is_empty());
    }
}
