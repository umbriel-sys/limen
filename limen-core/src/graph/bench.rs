//! (Work)bench [test] Graph implementations.
//!
//! These are the graph structs, and correspoding trait impls that are produced by the limen-build
//! graph builder for the following input, a concerete example has been given here for test purposes.
//!
//! ```text
//! define_graph! {
//!     pub struct TestPipeline;
//!
//!     nodes {
//!         0: { ty: TestCounterSourceU32, in_ports: 0, out_ports: 1, in_payload: (),  out_payload: u32, name: Some("src"), ingress_policy: Q_32_POLICY },
//!         1: { ty: TestIdentityModelNodeU32, in_ports: 1, out_ports: 1, in_payload: u32, out_payload: u32, name: Some("map") },
//!         2: { ty: TestSinkNodeU32,   in_ports: 1, out_ports: 0, in_payload: u32, out_payload: (),  name: Some("snk") }
//!     }
//!
//!     edges {
//!         0: { ty: Q32, payload: u32, manager: StaticMemoryManager<u32, 8>, from: (0,0), to: (1,0), policy: Q_32_POLICY, name: Some("e0") },
//!         1: { ty: Q32, payload: u32, manager: StaticMemoryManager<u32, 8>, from: (1,0), to: (2,0), policy: Q_32_POLICY, name: Some("e1") }
//!     }
//! }
//! ```

use crate::{
    edge::{Edge as _, EdgeOccupancy, NoQueue},
    errors::{GraphError, NodeError},
    graph::{GraphApi, GraphEdgeAccess, GraphNodeAccess, GraphNodeContextBuilder, GraphNodeTypes},
    node::{
        bench::{TestCounterSourceU32_2, TestIdentityModelNodeU32_2, TestSinkNodeU32_2},
        sink::SinkNode,
        source::{Source as _, SourceNode, EXTERNAL_INGRESS_NODE},
        Node as _, StepContext, StepResult,
    },
    policy::{AdmissionPolicy, EdgePolicy, NodePolicy, OverBudgetAction},
    prelude::{
        EdgeDescriptor, EdgeLink, NodeDescriptor, NodeLink, PlatformClock, StaticMemoryManager,
        Telemetry,
    },
    types::{EdgeIndex, NodeIndex, PortId, PortIndex},
};

// Test edge types.
type Q32 = crate::edge::bench::TestSpscRingBuf<8>;
const Q_32_POLICY: EdgePolicy = EdgePolicy {
    caps: crate::policy::QueueCaps {
        max_items: 8,
        soft_items: 8,
        max_bytes: None,
        soft_bytes: None,
    },
    over_budget: OverBudgetAction::Drop,
    admission: AdmissionPolicy::DropOldest,
};

// Test memory manager type (one per real edge).
type Mgr32 = StaticMemoryManager<u32, 8>;

// Test source node types.
#[allow(type_alias_bounds)]
type SrcNode<SrcClk: PlatformClock> = SourceNode<TestCounterSourceU32_2<SrcClk, 32>, u32, 1>;
const INGRESS_POLICY: EdgePolicy = Q_32_POLICY;

// Test model node types.
const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeU32_2<TEST_MAX_BATCH>;

// Test sink node types.
type SnkNode = SinkNode<TestSinkNodeU32_2, u32, 1>;

/// concrete graph implementation used for testing.
#[allow(clippy::complexity)]
pub struct TestPipeline<SrcClk: PlatformClock> {
    /// Nodes held in the graph.
    nodes: (
        NodeLink<SrcNode<SrcClk>, 0, 1, (), u32>,
        NodeLink<MapNode, 1, 1, u32, u32>,
        NodeLink<SnkNode, 1, 0, u32, ()>,
    ),
    /// Edges held in the graph.
    edges: (EdgeLink<Q32>, EdgeLink<Q32>),
    /// Memory managers for all *real* edges in declaration order.
    managers: (Mgr32, Mgr32),
}

impl<SrcClk: PlatformClock> TestPipeline<SrcClk> {
    /// Returns a TestPipeline graph given the nodes and edges.
    #[inline]
    pub fn new(
        node_0: impl Into<SrcNode<SrcClk>>,
        node_1: MapNode,
        node_2: impl Into<SnkNode>,
        q_0: Q32,
        q_1: Q32,
        mgr_0: Mgr32,
        mgr_1: Mgr32,
    ) -> Self {
        let node_0: SrcNode<SrcClk> = node_0.into();
        let node_2: SnkNode = node_2.into();

        let nodes = (
            NodeLink::<SrcNode<SrcClk>, 0, 1, (), u32>::new(
                node_0,
                NodeIndex::from(0usize),
                Some("src"),
            ),
            NodeLink::<MapNode, 1, 1, u32, u32>::new(node_1, NodeIndex::from(1usize), Some("map")),
            NodeLink::<SnkNode, 1, 0, u32, ()>::new(node_2, NodeIndex::from(2usize), Some("snk")),
        );

        let edges = (
            EdgeLink::<Q32>::new(
                q_0,
                EdgeIndex::from(1usize),
                PortId::new(NodeIndex::from(0usize), PortIndex::from(0usize)),
                PortId::new(NodeIndex::from(1usize), PortIndex::from(0usize)),
                Q_32_POLICY,
                Some("e0"),
            ),
            EdgeLink::<Q32>::new(
                q_1,
                EdgeIndex::from(2usize),
                PortId::new(NodeIndex::from(1usize), PortIndex::from(0usize)),
                PortId::new(NodeIndex::from(2usize), PortIndex::from(0usize)),
                Q_32_POLICY,
                Some("e1"),
            ),
        );

        let managers = (mgr_0, mgr_1);

        Self {
            nodes,
            edges,
            managers,
        }
    }
}

// ===== GraphApi<3,3> =====
impl<SrcClk: PlatformClock> GraphApi<3, 3> for TestPipeline<SrcClk> {
    #[inline]
    fn get_node_descriptors(&self) -> [NodeDescriptor; 3] {
        [
            self.nodes.0.descriptor(),
            self.nodes.1.descriptor(),
            self.nodes.2.descriptor(),
        ]
    }
    #[inline]
    fn get_edge_descriptors(&self) -> [EdgeDescriptor; 3] {
        [
            EdgeDescriptor::new(
                EdgeIndex::from(0usize),
                PortId::new(EXTERNAL_INGRESS_NODE, PortIndex::from(0usize)),
                PortId::new(NodeIndex::from(0usize), PortIndex::from(0usize)),
                Some("ingress0"),
            ),
            self.edges.0.descriptor(),
            self.edges.1.descriptor(),
        ]
    }

    #[inline]
    fn get_node_policies(&self) -> [NodePolicy; 3] {
        [
            self.nodes.0.policy(),
            self.nodes.1.policy(),
            self.nodes.2.policy(),
        ]
    }

    #[inline]
    fn get_edge_policies(&self) -> [EdgePolicy; 3] {
        [
            INGRESS_POLICY,
            *self.edges.0.policy(),
            *self.edges.1.policy(),
        ]
    }

    #[inline]
    fn edge_occupancy_for<const E: usize>(&self) -> Result<EdgeOccupancy, GraphError> {
        let occ = match E {
            0 => {
                let src = self.nodes.0.node().source_ref();
                src.ingress_occupancy()
            }
            1 => {
                let e = &self.edges.0;
                e.occupancy(e.policy())
            }
            2 => {
                let e = &self.edges.1;
                e.occupancy(e.policy())
            }
            _ => return Err(GraphError::InvalidEdgeIndex), // use your variant
        };
        Ok(occ)
    }

    #[inline]
    fn write_all_edge_occupancies(&self, out: &mut [EdgeOccupancy; 3]) -> Result<(), GraphError> {
        out[0] = self.edge_occupancy_for::<0>()?;
        out[1] = self.edge_occupancy_for::<1>()?;
        out[2] = self.edge_occupancy_for::<2>()?;
        Ok(())
    }

    #[inline]
    fn refresh_occupancies_for_node<const I: usize, const IN: usize, const OUT: usize>(
        &self,
        out: &mut [EdgeOccupancy; 3],
    ) -> Result<(), GraphError> {
        let node_idx = NodeIndex::from(I);
        // Iterate *all* edges; update those where this node is upstream OR downstream.
        for ed in self.get_edge_descriptors().iter() {
            if ed.upstream().node() == &node_idx || ed.downstream().node() == &node_idx {
                let ei = (ed.id()).as_usize();
                match ei {
                    0 => {
                        out[0] = self.edge_occupancy_for::<0>()?;
                    }
                    1 => {
                        out[1] = self.edge_occupancy_for::<1>()?;
                    }
                    2 => {
                        out[2] = self.edge_occupancy_for::<2>()?;
                    }
                    _ => return Err(GraphError::InvalidEdgeIndex),
                }
            }
        }
        Ok(())
    }

    #[inline]
    fn step_node_by_index<C, T>(
        &mut self,
        index: usize,
        clock: &C,
        telemetry: &mut T,
    ) -> Result<StepResult, NodeError>
    where
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        match index {
            0 => <Self as GraphNodeContextBuilder<0, 0, 1>>::with_node_and_step_context::<
                C,
                T,
                StepResult,
                NodeError,
            >(self, clock, telemetry, |node, ctx| node.step(ctx)),

            1 => <Self as GraphNodeContextBuilder<1, 1, 1>>::with_node_and_step_context::<
                C,
                T,
                StepResult,
                NodeError,
            >(self, clock, telemetry, |node, ctx| node.step(ctx)),

            2 => <Self as GraphNodeContextBuilder<2, 1, 0>>::with_node_and_step_context::<
                C,
                T,
                StepResult,
                NodeError,
            >(self, clock, telemetry, |node, ctx| node.step(ctx)),

            _ => unreachable!("invalid node index"),
        }
    }

    // --- std-only required items: provide no-op stubs for the "no-std" test graph ---
    #[cfg(feature = "std")]
    type OwnedBundle = ();

    #[cfg(feature = "std")]
    #[inline]
    fn take_owned_bundle_by_index(
        &mut self,
        _index: usize,
    ) -> Result<Self::OwnedBundle, GraphError> {
        // This graph doesn't support owned handoff; return an error so tests compile.
        Err(GraphError::InvalidEdgeIndex)
    }

    #[cfg(feature = "std")]
    #[inline]
    fn put_owned_bundle_by_index(&mut self, _bundle: Self::OwnedBundle) -> Result<(), GraphError> {
        // No-op; nothing was taken.
        Ok(())
    }

    #[cfg(feature = "std")]
    #[inline]
    fn step_owned_bundle<C, T>(
        _bundle: &mut Self::OwnedBundle,
        _clock: &C,
        _telemetry: &mut T,
    ) -> Result<StepResult, NodeError>
    where
        EdgePolicy: Copy,
    {
        // Not supported for this graph; make it explicit.
        Err(NodeError::execution_failed().with_code(1))
    }
}

// ===== GraphNodeAccess<I> =====
impl<SrcClk: PlatformClock> GraphNodeAccess<0> for TestPipeline<SrcClk> {
    type Node = NodeLink<SrcNode<SrcClk>, 0, 1, (), u32>;
    #[inline]
    fn node_ref(&self) -> &Self::Node {
        &self.nodes.0
    }
    #[inline]
    fn node_mut(&mut self) -> &mut Self::Node {
        &mut self.nodes.0
    }
}
impl<SrcClk: PlatformClock> GraphNodeAccess<1> for TestPipeline<SrcClk> {
    type Node = NodeLink<MapNode, 1, 1, u32, u32>;
    #[inline]
    fn node_ref(&self) -> &Self::Node {
        &self.nodes.1
    }
    #[inline]
    fn node_mut(&mut self) -> &mut Self::Node {
        &mut self.nodes.1
    }
}
impl<SrcClk: PlatformClock> GraphNodeAccess<2> for TestPipeline<SrcClk> {
    type Node = NodeLink<SnkNode, 1, 0, u32, ()>;
    #[inline]
    fn node_ref(&self) -> &Self::Node {
        &self.nodes.2
    }
    #[inline]
    fn node_mut(&mut self) -> &mut Self::Node {
        &mut self.nodes.2
    }
}

// ===== GraphEdgeAccess<E> =====
impl<SrcClk: PlatformClock> GraphEdgeAccess<1> for TestPipeline<SrcClk> {
    type Edge = EdgeLink<Q32>;
    #[inline]
    fn edge_ref(&self) -> &Self::Edge {
        &self.edges.0
    }
    #[inline]
    fn edge_mut(&mut self) -> &mut Self::Edge {
        &mut self.edges.0
    }
}
impl<SrcClk: PlatformClock> GraphEdgeAccess<2> for TestPipeline<SrcClk> {
    type Edge = EdgeLink<Q32>;
    #[inline]
    fn edge_ref(&self) -> &Self::Edge {
        &self.edges.1
    }
    #[inline]
    fn edge_mut(&mut self) -> &mut Self::Edge {
        &mut self.edges.1
    }
}

// ===== GraphNodeTypes<I, IN, OUT> =====
// node 0: IN=0, OUT=1
impl<SrcClk: PlatformClock> GraphNodeTypes<0, 0, 1> for TestPipeline<SrcClk> {
    type InP = ();
    type OutP = u32;
    type InQ = NoQueue;
    type OutQ = Q32;
    type InM = StaticMemoryManager<(), 1>;
    type OutM = Mgr32;
}
// node 1: IN=1, OUT=1
impl<SrcClk: PlatformClock> GraphNodeTypes<1, 1, 1> for TestPipeline<SrcClk> {
    type InP = u32;
    type OutP = u32;
    type InQ = Q32;
    type OutQ = Q32;
    type InM = Mgr32;
    type OutM = Mgr32;
}
// node 2: IN=1, OUT=0
impl<SrcClk: PlatformClock> GraphNodeTypes<2, 1, 0> for TestPipeline<SrcClk> {
    type InP = u32;
    type OutP = ();
    type InQ = Q32;
    type OutQ = NoQueue;
    type InM = Mgr32;
    type OutM = StaticMemoryManager<(), 1>;
}

// ===== GraphNodeContextBuilder<I, IN, OUT> =====
// node 0: in=[], out=[edge id 1]
impl<SrcClk: PlatformClock> GraphNodeContextBuilder<0, 0, 1> for TestPipeline<SrcClk>
where
    Self: GraphNodeAccess<0, Node = NodeLink<SrcNode<SrcClk>, 0, 1, (), u32>>,
{
    #[inline]
    fn make_step_context<'graph, 'telemetry, 'clock, C, T>(
        &'graph mut self,
        clock: &'clock C,
        telemetry: &'telemetry mut T,
    ) -> StepContext<
        'graph,
        'telemetry,
        'clock,
        0,
        1,
        <Self as GraphNodeTypes<0, 0, 1>>::InP,
        <Self as GraphNodeTypes<0, 0, 1>>::OutP,
        <Self as GraphNodeTypes<0, 0, 1>>::InQ,
        <Self as GraphNodeTypes<0, 0, 1>>::OutQ,
        <Self as GraphNodeTypes<0, 0, 1>>::InM,
        <Self as GraphNodeTypes<0, 0, 1>>::OutM,
        C,
        T,
    >
    where
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let out0_policy = *self.edges.0.policy();

        let inputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::InQ; 0] = [/* empty */];
        let outputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1] = [self.edges.0.queue_mut()];

        let in_managers: [&'graph mut <Self as GraphNodeTypes<0, 0, 1>>::InM; 0] = [];
        let out_managers: [&'graph mut <Self as GraphNodeTypes<0, 0, 1>>::OutM; 1] =
            [&mut self.managers.0];

        let in_policies: [EdgePolicy; 0] = [/* empty */];
        let out_policies: [EdgePolicy; 1] = [out0_policy];

        let node_id: u32 = 0;
        let in_edge_ids: [u32; 0] = [/* empty */];
        let out_edge_ids: [u32; 1] = [1];

        StepContext::<
            'graph,
            'telemetry,
            'clock,
            0,
            1,
            <Self as GraphNodeTypes<0, 0, 1>>::InP,
            <Self as GraphNodeTypes<0, 0, 1>>::OutP,
            <Self as GraphNodeTypes<0, 0, 1>>::InQ,
            <Self as GraphNodeTypes<0, 0, 1>>::OutQ,
            <Self as GraphNodeTypes<0, 0, 1>>::InM,
            <Self as GraphNodeTypes<0, 0, 1>>::OutM,
            C,
            T,
        >::new(
            inputs,
            outputs,
            in_managers,
            out_managers,
            in_policies,
            out_policies,
            node_id,
            in_edge_ids,
            out_edge_ids,
            clock,
            telemetry,
        )
    }

    #[inline]
    fn with_node_and_step_context<'telemetry, 'clock, C, T, R, E>(
        &mut self,
        clock: &'clock C,
        telemetry: &'telemetry mut T,
        f: impl FnOnce(
            &mut <Self as GraphNodeAccess<0>>::Node,
            &mut StepContext<
                '_,
                'telemetry,
                'clock,
                0,
                1,
                <Self as GraphNodeTypes<0, 0, 1>>::InP,
                <Self as GraphNodeTypes<0, 0, 1>>::OutP,
                <Self as GraphNodeTypes<0, 0, 1>>::InQ,
                <Self as GraphNodeTypes<0, 0, 1>>::OutQ,
                <Self as GraphNodeTypes<0, 0, 1>>::InM,
                <Self as GraphNodeTypes<0, 0, 1>>::OutM,
                C,
                T,
            >,
        ) -> Result<R, E>,
    ) -> Result<R, E>
    where
        Self: GraphNodeAccess<0>,
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let node = &mut self.nodes.0;

        let out0_policy = *self.edges.0.policy();

        let inputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::InQ; 0] = [/* empty */];
        let outputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1] = [self.edges.0.queue_mut()];

        let in_managers: [&mut <Self as GraphNodeTypes<0, 0, 1>>::InM; 0] = [/* empty */];
        let out_managers: [&mut <Self as GraphNodeTypes<0, 0, 1>>::OutM; 1] =
            [&mut self.managers.0];

        let in_policies: [EdgePolicy; 0] = [/* empty */];
        let out_policies: [EdgePolicy; 1] = [out0_policy];

        let node_id: u32 = 0;
        let in_edge_ids: [u32; 0] = [/* empty */];
        let out_edge_ids: [u32; 1] = [1];

        let mut ctx = StepContext::new(
            inputs,
            outputs,
            in_managers,
            out_managers,
            in_policies,
            out_policies,
            node_id,
            in_edge_ids,
            out_edge_ids,
            clock,
            telemetry,
        );
        f(node, &mut ctx)
    }
}

// node 1: in=[edge id 1], out=[edge id 2]
impl<SrcClk: PlatformClock> GraphNodeContextBuilder<1, 1, 1> for TestPipeline<SrcClk>
where
    Self: GraphNodeAccess<1, Node = NodeLink<MapNode, 1, 1, u32, u32>>,
{
    #[inline]
    fn make_step_context<'graph, 'telemetry, 'clock, C, T>(
        &'graph mut self,
        clock: &'clock C,
        telemetry: &'telemetry mut T,
    ) -> StepContext<
        'graph,
        'telemetry,
        'clock,
        1,
        1,
        <Self as GraphNodeTypes<1, 1, 1>>::InP,
        <Self as GraphNodeTypes<1, 1, 1>>::OutP,
        <Self as GraphNodeTypes<1, 1, 1>>::InQ,
        <Self as GraphNodeTypes<1, 1, 1>>::OutQ,
        <Self as GraphNodeTypes<1, 1, 1>>::InM,
        <Self as GraphNodeTypes<1, 1, 1>>::OutM,
        C,
        T,
    >
    where
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let in0_policy = *self.edges.0.policy();
        let out1_policy = *self.edges.1.policy();

        let inputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::InQ; 1] = [self.edges.0.queue_mut()];
        let outputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1] = [self.edges.1.queue_mut()];

        let in_managers: [&'graph mut <Self as GraphNodeTypes<1, 1, 1>>::InM; 1] =
            [&mut self.managers.0];
        let out_managers: [&'graph mut <Self as GraphNodeTypes<1, 1, 1>>::OutM; 1] =
            [&mut self.managers.1];

        let in_policies: [EdgePolicy; 1] = [in0_policy];
        let out_policies: [EdgePolicy; 1] = [out1_policy];

        let node_id: u32 = 1;
        let in_edge_ids: [u32; 1] = [1];
        let out_edge_ids: [u32; 1] = [2];

        StepContext::<
            'graph,
            'telemetry,
            'clock,
            1,
            1,
            <Self as GraphNodeTypes<1, 1, 1>>::InP,
            <Self as GraphNodeTypes<1, 1, 1>>::OutP,
            <Self as GraphNodeTypes<1, 1, 1>>::InQ,
            <Self as GraphNodeTypes<1, 1, 1>>::OutQ,
            <Self as GraphNodeTypes<1, 1, 1>>::InM,
            <Self as GraphNodeTypes<1, 1, 1>>::OutM,
            C,
            T,
        >::new(
            inputs,
            outputs,
            in_managers,
            out_managers,
            in_policies,
            out_policies,
            node_id,
            in_edge_ids,
            out_edge_ids,
            clock,
            telemetry,
        )
    }

    #[inline]
    fn with_node_and_step_context<'telemetry, 'clock, C, T, R, E>(
        &mut self,
        clock: &'clock C,
        telemetry: &'telemetry mut T,
        f: impl FnOnce(
            &mut <Self as GraphNodeAccess<1>>::Node,
            &mut StepContext<
                '_,
                'telemetry,
                'clock,
                1,
                1,
                <Self as GraphNodeTypes<1, 1, 1>>::InP,
                <Self as GraphNodeTypes<1, 1, 1>>::OutP,
                <Self as GraphNodeTypes<1, 1, 1>>::InQ,
                <Self as GraphNodeTypes<1, 1, 1>>::OutQ,
                <Self as GraphNodeTypes<1, 1, 1>>::InM,
                <Self as GraphNodeTypes<1, 1, 1>>::OutM,
                C,
                T,
            >,
        ) -> Result<R, E>,
    ) -> Result<R, E>
    where
        Self: GraphNodeAccess<1>,
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let node = &mut self.nodes.1;

        let in0_policy = *self.edges.0.policy();
        let out1_policy = *self.edges.1.policy();

        let inputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::InQ; 1] = [self.edges.0.queue_mut()];
        let outputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1] = [self.edges.1.queue_mut()];

        let in_managers: [&mut <Self as GraphNodeTypes<1, 1, 1>>::InM; 1] = [&mut self.managers.0];
        let out_managers: [&mut <Self as GraphNodeTypes<1, 1, 1>>::OutM; 1] =
            [&mut self.managers.1];

        let in_policies: [EdgePolicy; 1] = [in0_policy];
        let out_policies: [EdgePolicy; 1] = [out1_policy];

        let node_id: u32 = 1;
        let in_edge_ids: [u32; 1] = [1];
        let out_edge_ids: [u32; 1] = [2];

        let mut ctx = StepContext::new(
            inputs,
            outputs,
            in_managers,
            out_managers,
            in_policies,
            out_policies,
            node_id,
            in_edge_ids,
            out_edge_ids,
            clock,
            telemetry,
        );
        f(node, &mut ctx)
    }
}

// node 2: in=[edge id 2], out=[]
impl<SrcClk: PlatformClock> GraphNodeContextBuilder<2, 1, 0> for TestPipeline<SrcClk>
where
    Self: GraphNodeAccess<2, Node = NodeLink<SnkNode, 1, 0, u32, ()>>,
{
    #[inline]
    fn make_step_context<'graph, 'telemetry, 'clock, C, T>(
        &'graph mut self,
        clock: &'clock C,
        telemetry: &'telemetry mut T,
    ) -> StepContext<
        'graph,
        'telemetry,
        'clock,
        1,
        0,
        <Self as GraphNodeTypes<2, 1, 0>>::InP,
        <Self as GraphNodeTypes<2, 1, 0>>::OutP,
        <Self as GraphNodeTypes<2, 1, 0>>::InQ,
        <Self as GraphNodeTypes<2, 1, 0>>::OutQ,
        <Self as GraphNodeTypes<2, 1, 0>>::InM,
        <Self as GraphNodeTypes<2, 1, 0>>::OutM,
        C,
        T,
    >
    where
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let in1_policy = *self.edges.1.policy();

        let inputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::InQ; 1] = [self.edges.1.queue_mut()];
        let outputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0] = [/* empty */];

        let in_managers: [&'graph mut <Self as GraphNodeTypes<2, 1, 0>>::InM; 1] =
            [&mut self.managers.1];
        let out_managers: [&'graph mut <Self as GraphNodeTypes<2, 1, 0>>::OutM; 0] = [/* empty */];

        let in_policies: [EdgePolicy; 1] = [in1_policy];
        let out_policies: [EdgePolicy; 0] = [/* empty */];

        let node_id: u32 = 2;
        let in_edge_ids: [u32; 1] = [2];
        let out_edge_ids: [u32; 0] = [/* empty */];

        StepContext::<
            'graph,
            'telemetry,
            'clock,
            1,
            0,
            <Self as GraphNodeTypes<2, 1, 0>>::InP,
            <Self as GraphNodeTypes<2, 1, 0>>::OutP,
            <Self as GraphNodeTypes<2, 1, 0>>::InQ,
            <Self as GraphNodeTypes<2, 1, 0>>::OutQ,
            <Self as GraphNodeTypes<2, 1, 0>>::InM,
            <Self as GraphNodeTypes<2, 1, 0>>::OutM,
            C,
            T,
        >::new(
            inputs,
            outputs,
            in_managers,
            out_managers,
            in_policies,
            out_policies,
            node_id,
            in_edge_ids,
            out_edge_ids,
            clock,
            telemetry,
        )
    }

    #[inline]
    fn with_node_and_step_context<'telemetry, 'clock, C, T, R, E>(
        &mut self,
        clock: &'clock C,
        telemetry: &'telemetry mut T,
        f: impl FnOnce(
            &mut <Self as GraphNodeAccess<2>>::Node,
            &mut StepContext<
                '_,
                'telemetry,
                'clock,
                1,
                0,
                <Self as GraphNodeTypes<2, 1, 0>>::InP,
                <Self as GraphNodeTypes<2, 1, 0>>::OutP,
                <Self as GraphNodeTypes<2, 1, 0>>::InQ,
                <Self as GraphNodeTypes<2, 1, 0>>::OutQ,
                <Self as GraphNodeTypes<2, 1, 0>>::InM,
                <Self as GraphNodeTypes<2, 1, 0>>::OutM,
                C,
                T,
            >,
        ) -> Result<R, E>,
    ) -> Result<R, E>
    where
        Self: GraphNodeAccess<2>,
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let node = &mut self.nodes.2;

        let in1_policy = *self.edges.1.policy();

        let inputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::InQ; 1] = [self.edges.1.queue_mut()];
        let outputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0] = [/* empty */];

        let in_managers: [&mut <Self as GraphNodeTypes<2, 1, 0>>::InM; 1] = [&mut self.managers.1];
        let out_managers: [&mut <Self as GraphNodeTypes<2, 1, 0>>::OutM; 0] = [/* empty */];

        let in_policies: [EdgePolicy; 1] = [in1_policy];
        let out_policies: [EdgePolicy; 0] = [/* empty */];

        let node_id: u32 = 2;
        let in_edge_ids: [u32; 1] = [2];
        let out_edge_ids: [u32; 0] = [/* empty */];

        let mut ctx = StepContext::new(
            inputs,
            outputs,
            in_managers,
            out_managers,
            in_policies,
            out_policies,
            node_id,
            in_edge_ids,
            out_edge_ids,
            clock,
            telemetry,
        );
        f(node, &mut ctx)
    }
}

/// std graph implementation, for use by concurrent runtimes.
#[cfg(feature = "std")]
pub mod concurrent_graph {
    use super::*;

    use crate::{
        edge::{
            link::ConcurrentEdgeLink,
            spsc_concurrent::{ConcurrentQueue, ConsumerEndpoint, ProducerEndpoint},
            EdgeOccupancy, NoQueue,
        },
        errors::{GraphError, NodeError},
        graph::{
            GraphApi, GraphEdgeAccess, GraphNodeAccess, GraphNodeContextBuilder,
            GraphNodeOwnedEndpointHandoff, GraphNodeTypes,
        },
        node::{
            bench::{TestCounterSourceU32_2, TestIdentityModelNodeU32_2},
            source::{
                probe::{new_probe_edge_pair, ConcurrentIngressEdgeLink, SourceIngressUpdater},
                SourceNode,
            },
            StepContext, StepResult,
        },
        policy::EdgePolicy,
        prelude::{
            ConcurrentMemoryManager, EdgeDescriptor, NodeDescriptor, NodeLink, PlatformClock,
            Telemetry,
        },
        types::{EdgeIndex, NodeIndex, PortId, PortIndex},
    };

    // ===== Endpoint aliases used by nodes in this std graph =====
    type InEpU32 = ConsumerEndpoint<ConcurrentQueue<Q32>>;
    type OutEpU32 = ProducerEndpoint<ConcurrentQueue<Q32>>;

    // Concurrent memory manager type (one per real edge).
    type ConcMgr32 = ConcurrentMemoryManager<u32>;

    // Test source node types.
    #[allow(type_alias_bounds)]
    type SrcNode<SrcClk: PlatformClock + std::marker::Send + 'static> =
        SourceNode<TestCounterSourceU32_2<SrcClk, 32>, u32, 1>;

    // Test model node types.
    const TEST_MAX_BATCH: usize = 32;
    type MapNode = TestIdentityModelNodeU32_2<TEST_MAX_BATCH>;

    // Test sink node types.
    type SnkNode = SinkNode<TestSinkNodeU32_2, u32, 1>;

    /// concrete graph implementation (std / concurrent).
    #[allow(clippy::complexity)]
    pub struct TestPipelineStd<SrcClk: PlatformClock + std::marker::Send + 'static> {
        // Nodes. We keep them as Options to support "move-out" for owned handoff.
        nodes: (
            Option<NodeLink<SrcNode<SrcClk>, 0, 1, (), u32>>,
            Option<NodeLink<MapNode, 1, 1, u32, u32>>,
            Option<NodeLink<SnkNode, 1, 0, u32, ()>>,
        ),
        // Edges: one Arc<Mutex<_>> each + metadata.
        edges: (
            ConcurrentEdgeLink<Q32>, // e0: 0->1
            ConcurrentEdgeLink<Q32>, // e1: 1->2
        ),
        // Persistent endpoint views (borrowed path uses &mut to these).
        // Each edge has a consumer (downstream side) and a producer (upstream side).
        endpoints: (
            (InEpU32, OutEpU32), // e0: (to node1 in0, from node0 out0)
            (InEpU32, OutEpU32), // e1: (to node2 in0, from node1 out0)
        ),
        // Synthetic ingress edges (ids [0..S)), backed by atomic probes. S = 1 here.
        ingress_edges: [ConcurrentIngressEdgeLink<u32>; 1],
        // One updater per source; moved to that source worker; kept as Option so we can take().
        ingress_updaters: [Option<SourceIngressUpdater>; 1],
        // Cache node descriptors so validation still works after nodes are moved out.
        node_descs: [NodeDescriptor; 3],
        // Cache node policies so get_node_policies still works after nodes are moved out.
        node_policies: [NodePolicy; 3],
        /// Memory managers for all *real* edges in declaration order.
        managers: (ConcMgr32, ConcMgr32),
    }

    impl<SrcClk: PlatformClock + std::marker::Send + 'static> TestPipelineStd<SrcClk> {
        /// Build the std graph from nodes and concrete queues.
        #[inline]
        pub fn new(
            node_0: impl Into<SrcNode<SrcClk>>,
            node_1: MapNode,
            node_2: impl Into<SnkNode>,
            q_0: Q32,
            q_1: Q32,
            mgr_1: ConcMgr32,
            mgr_2: ConcMgr32,
        ) -> Self {
            // Build nodes
            let node_0: SrcNode<SrcClk> = node_0.into();
            let node_2: SnkNode = node_2.into();

            let node_0_link = NodeLink::<SrcNode<SrcClk>, 0, 1, (), u32>::new(
                node_0,
                NodeIndex::from(0usize),
                Some("src"),
            );
            let node_1_link = NodeLink::<MapNode, 1, 1, u32, u32>::new(
                node_1,
                NodeIndex::from(1usize),
                Some("map"),
            );
            let node_2_link = NodeLink::<SnkNode, 1, 0, u32, ()>::new(
                node_2,
                NodeIndex::from(2usize),
                Some("snk"),
            );

            let node_descs = [
                node_0_link.descriptor(),
                node_1_link.descriptor(),
                node_2_link.descriptor(),
            ];
            let node_policies = [
                node_0_link.policy(),
                node_1_link.policy(),
                node_2_link.policy(),
            ];

            let nodes = (Some(node_0_link), Some(node_1_link), Some(node_2_link));

            // Build edges
            let e0 = ConcurrentEdgeLink::<Q32>::new(
                q_0,
                EdgeIndex::from(1usize),
                PortId::new(NodeIndex::from(0usize), PortIndex::from(0)),
                PortId::new(NodeIndex::from(1usize), PortIndex::from(0)),
                Q_32_POLICY,
                Some("e0"),
            );
            let e1 = ConcurrentEdgeLink::<Q32>::new(
                q_1,
                EdgeIndex::from(2usize),
                PortId::new(NodeIndex::from(1usize), PortIndex::from(0)),
                PortId::new(NodeIndex::from(2usize), PortIndex::from(0)),
                Q_32_POLICY,
                Some("e1"),
            );

            // Mint persistent endpoints from the Arcs.
            let endpoints = {
                // e0 endpoints
                let c0 = ConcurrentQueue::from_arc(e0.arc());
                let p0 = ConcurrentQueue::from_arc(e0.arc());
                let e0_cons = InEpU32::new(c0);
                let e0_prod = OutEpU32::new(p0);
                // e1 endpoints
                let c1 = ConcurrentQueue::from_arc(e1.arc());
                let p1 = ConcurrentQueue::from_arc(e1.arc());
                let e1_cons = InEpU32::new(c1);
                let e1_prod = OutEpU32::new(p1);

                ((e0_cons, e0_prod), (e1_cons, e1_prod))
            };

            // Ingress probe(s): typed edge(s) held by graph + updater(s) moved to source worker(s)
            let (probe_edge_0, updater_0) = new_probe_edge_pair::<u32>();
            let ingress_edge_0 = ConcurrentIngressEdgeLink::from_probe(
                probe_edge_0,
                EdgeIndex::from(0usize),
                PortId::new(EXTERNAL_INGRESS_NODE, PortIndex::from(0)),
                PortId::new(NodeIndex::from(0usize), PortIndex::from(0)),
                nodes
                    .0
                    .as_ref()
                    .unwrap()
                    .node()
                    .source_ref()
                    .ingress_policy(),
                Some("ingress0"),
            );
            let ingress_edges = [ingress_edge_0];
            let ingress_updaters = [Some(updater_0)];

            let managers = (mgr_1, mgr_2);

            Self {
                nodes,
                edges: (e0, e1),
                endpoints,
                ingress_edges,
                ingress_updaters,
                node_descs,
                node_policies,
                managers,
            }
        }
    }

    /// ===== std-only opaque owned-bundle used by GraphApi take/put =====
    #[allow(clippy::large_enum_variant)]
    #[allow(missing_docs)]
    pub enum TestPipelineStdOwnedBundle<SrcClk: PlatformClock + std::marker::Send + 'static> {
        N0 {
            node: NodeLink<SrcNode<SrcClk>, 0, 1, (), u32>,
            ins: [NoQueue; 0],
            outs: [OutEpU32; 1],
            in_managers: [StaticMemoryManager<(), 1>; 0],
            out_managers: [ConcMgr32; 1],
            in_policies: [EdgePolicy; 0],
            out_policies: [EdgePolicy; 1],
            ingress_updater: SourceIngressUpdater,
        },
        N1 {
            node: NodeLink<MapNode, 1, 1, u32, u32>,
            ins: [InEpU32; 1],
            outs: [OutEpU32; 1],
            in_managers: [ConcMgr32; 1],
            out_managers: [ConcMgr32; 1],
            in_policies: [EdgePolicy; 1],
            out_policies: [EdgePolicy; 1],
        },
        N2 {
            node: NodeLink<SnkNode, 1, 0, u32, ()>,
            ins: [InEpU32; 1],
            outs: [NoQueue; 0],
            in_managers: [ConcMgr32; 1],
            out_managers: [StaticMemoryManager<(), 1>; 0],
            in_policies: [EdgePolicy; 1],
            out_policies: [EdgePolicy; 0],
        },
    }

    // ===== GraphApi<3,3> =====
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphApi<3, 3>
        for TestPipelineStd<SrcClk>
    {
        #[inline]
        fn get_node_descriptors(&self) -> [NodeDescriptor; 3] {
            self.node_descs.clone()
        }

        #[inline]
        fn get_edge_descriptors(&self) -> [EdgeDescriptor; 3] {
            [
                self.ingress_edges[0].descriptor(),
                self.edges.0.descriptor(),
                self.edges.1.descriptor(),
            ]
        }

        #[inline]
        fn get_node_policies(&self) -> [NodePolicy; 3] {
            self.node_policies
        }

        #[inline]
        fn get_edge_policies(&self) -> [EdgePolicy; 3] {
            [
                self.nodes
                    .0
                    .as_ref()
                    .unwrap()
                    .node()
                    .source_ref()
                    .ingress_policy(),
                *self.edges.0.policy(),
                *self.edges.1.policy(),
            ]
        }

        #[inline]
        fn edge_occupancy_for<const E: usize>(&self) -> Result<EdgeOccupancy, GraphError> {
            let occ = match E {
                0 => {
                    // synthetic ingress: read atomic probe
                    self.ingress_edges[0].occupancy(&self.ingress_edges[0].policy())
                }
                1 => {
                    let e = &self.edges.0;
                    e.occupancy(e.policy())
                }
                2 => {
                    let e = &self.edges.1;
                    e.occupancy(e.policy())
                }
                _ => return Err(GraphError::InvalidEdgeIndex),
            };
            Ok(occ)
        }

        #[inline]
        fn write_all_edge_occupancies(
            &self,
            out: &mut [EdgeOccupancy; 3],
        ) -> Result<(), GraphError> {
            out[0] = self.edge_occupancy_for::<0>()?;
            out[1] = self.edge_occupancy_for::<1>()?;
            out[2] = self.edge_occupancy_for::<2>()?;
            Ok(())
        }

        #[inline]
        fn refresh_occupancies_for_node<const I: usize, const IN: usize, const OUT: usize>(
            &self,
            out: &mut [EdgeOccupancy; 3],
        ) -> Result<(), GraphError> {
            let node_idx = NodeIndex::from(I);
            for ed in self.get_edge_descriptors().iter() {
                if ed.upstream().node() == &node_idx || ed.downstream().node() == &node_idx {
                    match (ed.id()).as_usize() {
                        0 => out[0] = self.edge_occupancy_for::<0>()?,
                        1 => out[1] = self.edge_occupancy_for::<1>()?,
                        2 => out[2] = self.edge_occupancy_for::<2>()?,
                        _ => return Err(GraphError::InvalidEdgeIndex),
                    }
                }
            }
            Ok(())
        }

        #[inline]
        fn step_node_by_index<C, T>(
            &mut self,
            index: usize,
            clock: &C,
            telemetry: &mut T,
        ) -> Result<StepResult, NodeError>
        where
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            match index {
                0 => <Self as GraphNodeContextBuilder<0, 0, 1>>::with_node_and_step_context::<
                    C,
                    T,
                    StepResult,
                    NodeError,
                >(self, clock, telemetry, |node, ctx| node.step(ctx)),
                1 => <Self as GraphNodeContextBuilder<1, 1, 1>>::with_node_and_step_context::<
                    C,
                    T,
                    StepResult,
                    NodeError,
                >(self, clock, telemetry, |node, ctx| node.step(ctx)),
                2 => <Self as GraphNodeContextBuilder<2, 1, 0>>::with_node_and_step_context::<
                    C,
                    T,
                    StepResult,
                    NodeError,
                >(self, clock, telemetry, |node, ctx| node.step(ctx)),
                _ => unreachable!("invalid node index"),
            }
        }

        type OwnedBundle = TestPipelineStdOwnedBundle<SrcClk>;

        #[cfg(feature = "std")]
        fn take_owned_bundle_by_index(
            &mut self,
            index: usize,
        ) -> Result<Self::OwnedBundle, GraphError> {
            match index {
                0 => {
                    let node = self.nodes.0.take().ok_or(GraphError::InvalidEdgeIndex)?;
                    let ingress_updater = self.ingress_updaters[0]
                        .take()
                        .expect("ingress updater already taken");
                    Ok(TestPipelineStdOwnedBundle::N0 {
                        node,
                        ins: [],
                        outs: [(self.endpoints.0).1.clone()],
                        in_managers: [],
                        out_managers: [self.managers.0.clone()],
                        in_policies: [],
                        out_policies: [*self.edges.0.policy()],
                        ingress_updater,
                    })
                }
                1 => {
                    let node = self.nodes.1.take().ok_or(GraphError::InvalidEdgeIndex)?;
                    Ok(TestPipelineStdOwnedBundle::N1 {
                        node,
                        ins: [(self.endpoints.0).0.clone()],
                        outs: [(self.endpoints.1).1.clone()],
                        in_managers: [self.managers.0.clone()],
                        out_managers: [self.managers.1.clone()],
                        in_policies: [*self.edges.0.policy()],
                        out_policies: [*self.edges.1.policy()],
                    })
                }
                2 => {
                    let node = self.nodes.2.take().ok_or(GraphError::InvalidEdgeIndex)?;
                    Ok(TestPipelineStdOwnedBundle::N2 {
                        node,
                        ins: [(self.endpoints.1).0.clone()],
                        outs: [],
                        in_managers: [self.managers.1.clone()],
                        out_managers: [],
                        in_policies: [*self.edges.1.policy()],
                        out_policies: [],
                    })
                }
                _ => Err(GraphError::InvalidEdgeIndex),
            }
        }

        #[cfg(feature = "std")]
        fn put_owned_bundle_by_index(
            &mut self,
            bundle: Self::OwnedBundle,
        ) -> Result<(), GraphError> {
            match bundle {
                TestPipelineStdOwnedBundle::N0 {
                    node,
                    ins: _,
                    outs,
                    in_managers: _,
                    out_managers,
                    in_policies: _,
                    out_policies: _,
                    ingress_updater,
                } => {
                    assert!(self.nodes.0.is_none(), "node 0 already present");
                    self.nodes.0 = Some(node);
                    (self.endpoints.0).1 = outs[0].clone();
                    self.managers.0 = out_managers[0].clone();
                    self.ingress_updaters[0] = Some(ingress_updater);
                    Ok(())
                }
                TestPipelineStdOwnedBundle::N1 {
                    node,
                    ins,
                    outs,
                    in_managers,
                    out_managers,
                    in_policies: _,
                    out_policies: _,
                } => {
                    assert!(self.nodes.1.is_none(), "node 1 already present");
                    self.nodes.1 = Some(node);
                    (self.endpoints.0).0 = ins[0].clone();
                    (self.endpoints.1).1 = outs[0].clone();
                    self.managers.0 = in_managers[0].clone();
                    self.managers.1 = out_managers[0].clone();
                    Ok(())
                }
                TestPipelineStdOwnedBundle::N2 {
                    node,
                    ins,
                    outs: _,
                    in_managers,
                    out_managers: _,
                    in_policies: _,
                    out_policies: _,
                } => {
                    assert!(self.nodes.2.is_none(), "node 2 already present");
                    self.nodes.2 = Some(node);
                    (self.endpoints.1).0 = ins[0].clone();
                    self.managers.1 = in_managers[0].clone();
                    Ok(())
                }
            }
        }

        #[cfg(feature = "std")]
        #[inline]
        fn step_owned_bundle<C, T>(
            bundle: &mut Self::OwnedBundle,
            clock: &C,
            telemetry: &mut T,
        ) -> Result<StepResult, NodeError>
        where
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            match bundle {
                TestPipelineStdOwnedBundle::N0 {
                    node,
                    ins: _,
                    outs,
                    in_managers: _,
                    out_managers,
                    in_policies,
                    out_policies,
                    ingress_updater,
                } => {
                    let occ = node.node().source_ref().ingress_occupancy();
                    ingress_updater.update(*occ.items(), *occ.bytes());

                    let inputs: [&mut NoQueue; 0] = [];
                    let outputs: [&mut OutEpU32; 1] = [&mut outs[0]];
                    let in_mgr_arr: [&mut StaticMemoryManager<(), 1>; 0] = [];
                    let out_mgr_arr: [&mut ConcMgr32; 1] = [&mut out_managers[0]];
                    let node_id: u32 = 0;
                    let in_edge_ids: [u32; 0] = [];
                    let out_edge_ids: [u32; 1] = [1];

                    let mut ctx = crate::node::StepContext::new(
                        inputs,
                        outputs,
                        in_mgr_arr,
                        out_mgr_arr,
                        *in_policies,
                        *out_policies,
                        node_id,
                        in_edge_ids,
                        out_edge_ids,
                        clock,
                        telemetry,
                    );
                    node.step(&mut ctx)
                }
                TestPipelineStdOwnedBundle::N1 {
                    node,
                    ins,
                    outs,
                    in_managers,
                    out_managers,
                    in_policies,
                    out_policies,
                } => {
                    let inputs: [&mut InEpU32; 1] = [&mut ins[0]];
                    let outputs: [&mut OutEpU32; 1] = [&mut outs[0]];
                    let in_mgr_arr: [&mut ConcMgr32; 1] = [&mut in_managers[0]];
                    let out_mgr_arr: [&mut ConcMgr32; 1] = [&mut out_managers[0]];
                    let node_id: u32 = 1;
                    let in_edge_ids: [u32; 1] = [1];
                    let out_edge_ids: [u32; 1] = [2];

                    let mut ctx = crate::node::StepContext::new(
                        inputs,
                        outputs,
                        in_mgr_arr,
                        out_mgr_arr,
                        *in_policies,
                        *out_policies,
                        node_id,
                        in_edge_ids,
                        out_edge_ids,
                        clock,
                        telemetry,
                    );
                    node.step(&mut ctx)
                }
                TestPipelineStdOwnedBundle::N2 {
                    node,
                    ins,
                    outs: _,
                    in_managers,
                    out_managers: _,
                    in_policies,
                    out_policies,
                } => {
                    let inputs: [&mut InEpU32; 1] = [&mut ins[0]];
                    let outputs: [&mut NoQueue; 0] = [];
                    let in_mgr_arr: [&mut ConcMgr32; 1] = [&mut in_managers[0]];
                    let out_mgr_arr: [&mut StaticMemoryManager<(), 1>; 0] = [];
                    let node_id: u32 = 2;
                    let in_edge_ids: [u32; 1] = [2];
                    let out_edge_ids: [u32; 0] = [];

                    let mut ctx = crate::node::StepContext::new(
                        inputs,
                        outputs,
                        in_mgr_arr,
                        out_mgr_arr,
                        *in_policies,
                        *out_policies,
                        node_id,
                        in_edge_ids,
                        out_edge_ids,
                        clock,
                        telemetry,
                    );
                    node.step(&mut ctx)
                }
            }
        }
    }

    // ===== GraphNodeAccess<I> =====
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeAccess<0>
        for TestPipelineStd<SrcClk>
    {
        type Node = NodeLink<SrcNode<SrcClk>, 0, 1, (), u32>;
        #[inline]
        fn node_ref(&self) -> &Self::Node {
            self.nodes.0.as_ref().expect("node 0 moved")
        }
        #[inline]
        fn node_mut(&mut self) -> &mut Self::Node {
            self.nodes.0.as_mut().expect("node 0 moved")
        }
    }
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeAccess<1>
        for TestPipelineStd<SrcClk>
    {
        type Node = NodeLink<MapNode, 1, 1, u32, u32>;
        #[inline]
        fn node_ref(&self) -> &Self::Node {
            self.nodes.1.as_ref().expect("node 1 moved")
        }
        #[inline]
        fn node_mut(&mut self) -> &mut Self::Node {
            self.nodes.1.as_mut().expect("node 1 moved")
        }
    }
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeAccess<2>
        for TestPipelineStd<SrcClk>
    {
        type Node = NodeLink<SnkNode, 1, 0, u32, ()>;
        #[inline]
        fn node_ref(&self) -> &Self::Node {
            self.nodes.2.as_ref().expect("node 2 moved")
        }
        #[inline]
        fn node_mut(&mut self) -> &mut Self::Node {
            self.nodes.2.as_mut().expect("node 2 moved")
        }
    }

    // ===== GraphEdgeAccess<E> =====
    // We expose our StdEdge; runtimes/tooling can still read descriptors/policies.
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphEdgeAccess<1>
        for TestPipelineStd<SrcClk>
    {
        type Edge = ConcurrentEdgeLink<Q32>;
        #[inline]
        fn edge_ref(&self) -> &Self::Edge {
            &self.edges.0
        }
        #[inline]
        fn edge_mut(&mut self) -> &mut Self::Edge {
            &mut self.edges.0
        }
    }
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphEdgeAccess<2>
        for TestPipelineStd<SrcClk>
    {
        type Edge = ConcurrentEdgeLink<Q32>;
        #[inline]
        fn edge_ref(&self) -> &Self::Edge {
            &self.edges.1
        }
        #[inline]
        fn edge_mut(&mut self) -> &mut Self::Edge {
            &mut self.edges.1
        }
    }

    // ===== GraphNodeTypes<I, IN, OUT> =====
    // node 0: IN=0, OUT=1  (src)
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeTypes<0, 0, 1>
        for TestPipelineStd<SrcClk>
    {
        type InP = ();
        type OutP = u32;
        type InQ = NoQueue;
        type OutQ = OutEpU32;
        type InM = StaticMemoryManager<(), 1>;
        type OutM = ConcMgr32;
    }
    // node 1: IN=1, OUT=1  (map)
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeTypes<1, 1, 1>
        for TestPipelineStd<SrcClk>
    {
        type InP = u32;
        type OutP = u32;
        type InQ = InEpU32;
        type OutQ = OutEpU32;
        type InM = ConcMgr32;
        type OutM = ConcMgr32;
    }
    // node 2: IN=1, OUT=0  (snk)
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeTypes<2, 1, 0>
        for TestPipelineStd<SrcClk>
    {
        type InP = u32;
        type OutP = ();
        type InQ = InEpU32;
        type OutQ = NoQueue;
        type InM = ConcMgr32;
        type OutM = StaticMemoryManager<(), 1>;
    }

    // ===== GraphNodeContextBuilder<I, IN, OUT> =====
    //
    // We implement ONLY the borrowed handoff method here (the runtime should call it).
    // If you still want `make_step_context(..)` as well, you can add it by borrowing
    // these same endpoint fields; the borrowed method is the key to avoid borrow overlap.

    // node 0: in=[], out=[edge id 1]
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeContextBuilder<0, 0, 1>
        for TestPipelineStd<SrcClk>
    where
        Self: GraphNodeAccess<0, Node = NodeLink<SrcNode<SrcClk>, 0, 1, (), u32>>,
    {
        #[inline]
        fn make_step_context<'graph, 'telemetry, 'clock, C, T>(
            &'graph mut self,
            clock: &'clock C,
            telemetry: &'telemetry mut T,
        ) -> StepContext<
            'graph,
            'telemetry,
            'clock,
            0,
            1,
            <Self as GraphNodeTypes<0, 0, 1>>::InP,
            <Self as GraphNodeTypes<0, 0, 1>>::OutP,
            <Self as GraphNodeTypes<0, 0, 1>>::InQ,
            <Self as GraphNodeTypes<0, 0, 1>>::OutQ,
            <Self as GraphNodeTypes<0, 0, 1>>::InM,
            <Self as GraphNodeTypes<0, 0, 1>>::OutM,
            C,
            T,
        >
        where
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            let out0_policy = *self.edges.0.policy();

            let inputs: [&'graph mut <Self as GraphNodeTypes<0, 0, 1>>::InQ; 0] = [];
            let outputs: [&'graph mut <Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1] =
                [&mut (self.endpoints.0).1];

            let in_managers: [&'graph mut <Self as GraphNodeTypes<0, 0, 1>>::InM; 0] = [];
            let out_managers: [&'graph mut <Self as GraphNodeTypes<0, 0, 1>>::OutM; 1] =
                [&mut self.managers.0];

            let in_policies: [EdgePolicy; 0] = [/* empty */];
            let out_policies: [EdgePolicy; 1] = [out0_policy];

            let node_id: u32 = 0;
            let in_edge_ids: [u32; 0] = [/* empty */];
            let out_edge_ids: [u32; 1] = [1];

            StepContext::new(
                inputs,
                outputs,
                in_managers,
                out_managers,
                in_policies,
                out_policies,
                node_id,
                in_edge_ids,
                out_edge_ids,
                clock,
                telemetry,
            )
        }

        #[inline]
        fn with_node_and_step_context<'telemetry, 'clock, C, T, R, E>(
            &mut self,
            clock: &'clock C,
            telemetry: &'telemetry mut T,
            f: impl FnOnce(
                &mut <Self as GraphNodeAccess<0>>::Node,
                &mut StepContext<
                    '_,
                    'telemetry,
                    'clock,
                    0,
                    1,
                    <Self as GraphNodeTypes<0, 0, 1>>::InP,
                    <Self as GraphNodeTypes<0, 0, 1>>::OutP,
                    <Self as GraphNodeTypes<0, 0, 1>>::InQ,
                    <Self as GraphNodeTypes<0, 0, 1>>::OutQ,
                    <Self as GraphNodeTypes<0, 0, 1>>::InM,
                    <Self as GraphNodeTypes<0, 0, 1>>::OutM,
                    C,
                    T,
                >,
            ) -> Result<R, E>,
        ) -> Result<R, E>
        where
            Self: GraphNodeAccess<0>,
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            let node = self.nodes.0.as_mut().expect("node 0 moved");
            let out0_policy = *self.edges.0.policy();

            let inputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::InQ; 0] = [];
            let outputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1] =
                [&mut (self.endpoints.0).1];

            let in_managers: [&mut <Self as GraphNodeTypes<0, 0, 1>>::InM; 0] = [];
            let out_managers: [&mut <Self as GraphNodeTypes<0, 0, 1>>::OutM; 1] =
                [&mut self.managers.0];

            let in_policies: [EdgePolicy; 0] = [/* empty */];
            let out_policies: [EdgePolicy; 1] = [out0_policy];

            let node_id: u32 = 0;
            let in_edge_ids: [u32; 0] = [/* empty */];
            let out_edge_ids: [u32; 1] = [1];

            let mut ctx = StepContext::new(
                inputs,
                outputs,
                in_managers,
                out_managers,
                in_policies,
                out_policies,
                node_id,
                in_edge_ids,
                out_edge_ids,
                clock,
                telemetry,
            );
            f(node, &mut ctx)
        }
    }

    // node 1: in=[edge id 1], out=[edge id 2]
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeContextBuilder<1, 1, 1>
        for TestPipelineStd<SrcClk>
    where
        Self: GraphNodeAccess<1, Node = NodeLink<MapNode, 1, 1, u32, u32>>,
    {
        #[inline]
        fn make_step_context<'graph, 'telemetry, 'clock, C, T>(
            &'graph mut self,
            clock: &'clock C,
            telemetry: &'telemetry mut T,
        ) -> StepContext<
            'graph,
            'telemetry,
            'clock,
            1,
            1,
            <Self as GraphNodeTypes<1, 1, 1>>::InP,
            <Self as GraphNodeTypes<1, 1, 1>>::OutP,
            <Self as GraphNodeTypes<1, 1, 1>>::InQ,
            <Self as GraphNodeTypes<1, 1, 1>>::OutQ,
            <Self as GraphNodeTypes<1, 1, 1>>::InM,
            <Self as GraphNodeTypes<1, 1, 1>>::OutM,
            C,
            T,
        >
        where
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            let in0_policy = *self.edges.0.policy();
            let out1_policy = *self.edges.1.policy();

            let inputs: [&'graph mut <Self as GraphNodeTypes<1, 1, 1>>::InQ; 1] =
                [&mut (self.endpoints.0).0];
            let outputs: [&'graph mut <Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1] =
                [&mut (self.endpoints.1).1];

            let in_managers: [&'graph mut <Self as GraphNodeTypes<1, 1, 1>>::InM; 1] =
                [&mut self.managers.0];
            let out_managers: [&'graph mut <Self as GraphNodeTypes<1, 1, 1>>::OutM; 1] =
                [&mut self.managers.1];

            let in_policies: [EdgePolicy; 1] = [in0_policy];
            let out_policies: [EdgePolicy; 1] = [out1_policy];

            let node_id: u32 = 1;
            let in_edge_ids: [u32; 1] = [1];
            let out_edge_ids: [u32; 1] = [2];

            StepContext::new(
                inputs,
                outputs,
                in_managers,
                out_managers,
                in_policies,
                out_policies,
                node_id,
                in_edge_ids,
                out_edge_ids,
                clock,
                telemetry,
            )
        }

        #[inline]
        fn with_node_and_step_context<'telemetry, 'clock, C, T, R, E>(
            &mut self,
            clock: &'clock C,
            telemetry: &'telemetry mut T,
            f: impl FnOnce(
                &mut <Self as GraphNodeAccess<1>>::Node,
                &mut StepContext<
                    '_,
                    'telemetry,
                    'clock,
                    1,
                    1,
                    <Self as GraphNodeTypes<1, 1, 1>>::InP,
                    <Self as GraphNodeTypes<1, 1, 1>>::OutP,
                    <Self as GraphNodeTypes<1, 1, 1>>::InQ,
                    <Self as GraphNodeTypes<1, 1, 1>>::OutQ,
                    <Self as GraphNodeTypes<1, 1, 1>>::InM,
                    <Self as GraphNodeTypes<1, 1, 1>>::OutM,
                    C,
                    T,
                >,
            ) -> Result<R, E>,
        ) -> Result<R, E>
        where
            Self: GraphNodeAccess<1>,
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            let node = self.nodes.1.as_mut().expect("node 1 moved");
            let in0_policy = *self.edges.0.policy();
            let out1_policy = *self.edges.1.policy();

            let inputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::InQ; 1] =
                [&mut (self.endpoints.0).0];
            let outputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1] =
                [&mut (self.endpoints.1).1];

            let in_managers: [&mut <Self as GraphNodeTypes<1, 1, 1>>::InM; 1] =
                [&mut self.managers.0];
            let out_managers: [&mut <Self as GraphNodeTypes<1, 1, 1>>::OutM; 1] =
                [&mut self.managers.1];

            let in_policies: [EdgePolicy; 1] = [in0_policy];
            let out_policies: [EdgePolicy; 1] = [out1_policy];

            let node_id: u32 = 1;
            let in_edge_ids: [u32; 1] = [1];
            let out_edge_ids: [u32; 1] = [2];

            let mut ctx = StepContext::new(
                inputs,
                outputs,
                in_managers,
                out_managers,
                in_policies,
                out_policies,
                node_id,
                in_edge_ids,
                out_edge_ids,
                clock,
                telemetry,
            );
            f(node, &mut ctx)
        }
    }

    // node 2: in=[edge id 2], out=[]
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeContextBuilder<2, 1, 0>
        for TestPipelineStd<SrcClk>
    where
        Self: GraphNodeAccess<2, Node = NodeLink<SnkNode, 1, 0, u32, ()>>,
    {
        #[inline]
        fn make_step_context<'graph, 'telemetry, 'clock, C, T>(
            &'graph mut self,
            clock: &'clock C,
            telemetry: &'telemetry mut T,
        ) -> StepContext<
            'graph,
            'telemetry,
            'clock,
            1,
            0,
            <Self as GraphNodeTypes<2, 1, 0>>::InP,
            <Self as GraphNodeTypes<2, 1, 0>>::OutP,
            <Self as GraphNodeTypes<2, 1, 0>>::InQ,
            <Self as GraphNodeTypes<2, 1, 0>>::OutQ,
            <Self as GraphNodeTypes<2, 1, 0>>::InM,
            <Self as GraphNodeTypes<2, 1, 0>>::OutM,
            C,
            T,
        >
        where
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            let in1_policy = *self.edges.1.policy();

            let inputs: [&'graph mut <Self as GraphNodeTypes<2, 1, 0>>::InQ; 1] =
                [&mut (self.endpoints.1).0];
            let outputs: [&'graph mut <Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0] = [];

            let in_managers: [&'graph mut <Self as GraphNodeTypes<2, 1, 0>>::InM; 1] =
                [&mut self.managers.1];
            let out_managers: [&'graph mut <Self as GraphNodeTypes<2, 1, 0>>::OutM; 0] = [];

            let in_policies: [EdgePolicy; 1] = [in1_policy];
            let out_policies: [EdgePolicy; 0] = [/* empty */];

            let node_id: u32 = 2;
            let in_edge_ids: [u32; 1] = [2];
            let out_edge_ids: [u32; 0] = [/* empty */];

            StepContext::new(
                inputs,
                outputs,
                in_managers,
                out_managers,
                in_policies,
                out_policies,
                node_id,
                in_edge_ids,
                out_edge_ids,
                clock,
                telemetry,
            )
        }

        #[inline]
        fn with_node_and_step_context<'telemetry, 'clock, C, T, R, E>(
            &mut self,
            clock: &'clock C,
            telemetry: &'telemetry mut T,
            f: impl FnOnce(
                &mut <Self as GraphNodeAccess<2>>::Node,
                &mut StepContext<
                    '_,
                    'telemetry,
                    'clock,
                    1,
                    0,
                    <Self as GraphNodeTypes<2, 1, 0>>::InP,
                    <Self as GraphNodeTypes<2, 1, 0>>::OutP,
                    <Self as GraphNodeTypes<2, 1, 0>>::InQ,
                    <Self as GraphNodeTypes<2, 1, 0>>::OutQ,
                    <Self as GraphNodeTypes<2, 1, 0>>::InM,
                    <Self as GraphNodeTypes<2, 1, 0>>::OutM,
                    C,
                    T,
                >,
            ) -> Result<R, E>,
        ) -> Result<R, E>
        where
            Self: GraphNodeAccess<2>,
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            let node = self.nodes.2.as_mut().expect("node 2 moved");
            let in1_policy = *self.edges.1.policy();

            let inputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::InQ; 1] =
                [&mut (self.endpoints.1).0];
            let outputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0] = [];

            let in_managers: [&mut <Self as GraphNodeTypes<2, 1, 0>>::InM; 1] =
                [&mut self.managers.1];
            let out_managers: [&mut <Self as GraphNodeTypes<2, 1, 0>>::OutM; 0] = [];

            let in_policies: [EdgePolicy; 1] = [in1_policy];
            let out_policies: [EdgePolicy; 0] = [/* empty */];

            let node_id: u32 = 2;
            let in_edge_ids: [u32; 1] = [2];
            let out_edge_ids: [u32; 0] = [/* empty */];

            let mut ctx = StepContext::new(
                inputs,
                outputs,
                in_managers,
                out_managers,
                in_policies,
                out_policies,
                node_id,
                in_edge_ids,
                out_edge_ids,
                clock,
                telemetry,
            );
            f(node, &mut ctx)
        }
    }

    // ===== Std-only owned handoff =====
    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeOwnedEndpointHandoff<0, 0, 1>
        for TestPipelineStd<SrcClk>
    {
        type NodeOwned = NodeLink<SrcNode<SrcClk>, 0, 1, (), u32>;

        fn take_node_and_endpoints(
            &mut self,
        ) -> (
            Self::NodeOwned,
            [<Self as GraphNodeTypes<0, 0, 1>>::InQ; 0],
            [<Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1],
            [<Self as GraphNodeTypes<0, 0, 1>>::InM; 0],
            [<Self as GraphNodeTypes<0, 0, 1>>::OutM; 1],
            [EdgePolicy; 0],
            [EdgePolicy; 1],
        )
        where
            <Self as GraphNodeTypes<0, 0, 1>>::InQ: Send + 'static,
            <Self as GraphNodeTypes<0, 0, 1>>::OutQ: Send + 'static,
            <Self as GraphNodeTypes<0, 0, 1>>::InM: Send + 'static,
            <Self as GraphNodeTypes<0, 0, 1>>::OutM: Send + 'static,
        {
            let node = self.nodes.0.take().expect("node 0 already taken");
            let out0_policy = *self.edges.0.policy();
            let out0 = (self.endpoints.0).1.clone();

            (
                node,
                [],
                [out0],
                [],
                [self.managers.0.clone()],
                [],
                [out0_policy],
            )
        }

        fn put_node_and_endpoints(
            &mut self,
            node: Self::NodeOwned,
            _inputs: [<Self as GraphNodeTypes<0, 0, 1>>::InQ; 0],
            outputs: [<Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1],
            _in_managers: [<Self as GraphNodeTypes<0, 0, 1>>::InM; 0],
            out_managers: [<Self as GraphNodeTypes<0, 0, 1>>::OutM; 1],
        ) {
            assert!(self.nodes.0.is_none(), "node 0 already present");
            self.nodes.0 = Some(node);
            (self.endpoints.0).1 = outputs[0].clone();
            self.managers.0 = out_managers[0].clone();
        }
    }

    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeOwnedEndpointHandoff<1, 1, 1>
        for TestPipelineStd<SrcClk>
    {
        type NodeOwned = NodeLink<MapNode, 1, 1, u32, u32>;

        fn take_node_and_endpoints(
            &mut self,
        ) -> (
            Self::NodeOwned,
            [<Self as GraphNodeTypes<1, 1, 1>>::InQ; 1],
            [<Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1],
            [<Self as GraphNodeTypes<1, 1, 1>>::InM; 1],
            [<Self as GraphNodeTypes<1, 1, 1>>::OutM; 1],
            [EdgePolicy; 1],
            [EdgePolicy; 1],
        )
        where
            <Self as GraphNodeTypes<1, 1, 1>>::InQ: Send + 'static,
            <Self as GraphNodeTypes<1, 1, 1>>::OutQ: Send + 'static,
            <Self as GraphNodeTypes<1, 1, 1>>::InM: Send + 'static,
            <Self as GraphNodeTypes<1, 1, 1>>::OutM: Send + 'static,
        {
            let node = self.nodes.1.take().expect("node 1 already taken");
            let in0_policy = *self.edges.0.policy();
            let out1_policy = *self.edges.1.policy();
            let in0 = (self.endpoints.0).0.clone();
            let out1 = (self.endpoints.1).1.clone();

            (
                node,
                [in0],
                [out1],
                [self.managers.0.clone()],
                [self.managers.1.clone()],
                [in0_policy],
                [out1_policy],
            )
        }
        fn put_node_and_endpoints(
            &mut self,
            node: Self::NodeOwned,
            inputs: [<Self as GraphNodeTypes<1, 1, 1>>::InQ; 1],
            outputs: [<Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1],
            in_managers: [<Self as GraphNodeTypes<1, 1, 1>>::InM; 1],
            out_managers: [<Self as GraphNodeTypes<1, 1, 1>>::OutM; 1],
        ) {
            assert!(self.nodes.1.is_none(), "node 1 already present");
            self.nodes.1 = Some(node);
            (self.endpoints.0).0 = inputs[0].clone();
            (self.endpoints.1).1 = outputs[0].clone();
            self.managers.0 = in_managers[0].clone();
            self.managers.1 = out_managers[0].clone();
        }
    }

    impl<SrcClk: PlatformClock + std::marker::Send + 'static> GraphNodeOwnedEndpointHandoff<2, 1, 0>
        for TestPipelineStd<SrcClk>
    {
        type NodeOwned = NodeLink<SnkNode, 1, 0, u32, ()>;

        fn take_node_and_endpoints(
            &mut self,
        ) -> (
            Self::NodeOwned,
            [<Self as GraphNodeTypes<2, 1, 0>>::InQ; 1],
            [<Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0],
            [<Self as GraphNodeTypes<2, 1, 0>>::InM; 1],
            [<Self as GraphNodeTypes<2, 1, 0>>::OutM; 0],
            [EdgePolicy; 1],
            [EdgePolicy; 0],
        )
        where
            <Self as GraphNodeTypes<2, 1, 0>>::InQ: Send + 'static,
            <Self as GraphNodeTypes<2, 1, 0>>::OutQ: Send + 'static,
            <Self as GraphNodeTypes<2, 1, 0>>::InM: Send + 'static,
            <Self as GraphNodeTypes<2, 1, 0>>::OutM: Send + 'static,
        {
            let node = self.nodes.2.take().expect("node 2 already taken");
            let in1_policy = *self.edges.1.policy();
            let in1 = (self.endpoints.1).0.clone();
            (
                node,
                [in1],
                [],
                [self.managers.1.clone()],
                [],
                [in1_policy],
                [],
            )
        }

        fn put_node_and_endpoints(
            &mut self,
            node: Self::NodeOwned,
            inputs: [<Self as GraphNodeTypes<2, 1, 0>>::InQ; 1],
            _outputs: [<Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0],
            in_managers: [<Self as GraphNodeTypes<2, 1, 0>>::InM; 1],
            _out_managers: [<Self as GraphNodeTypes<2, 1, 0>>::OutM; 0],
        ) {
            assert!(self.nodes.2.is_none(), "node 2 already present");
            self.nodes.2 = Some(node);
            (self.endpoints.1).0 = inputs[0].clone();
            self.managers.1 = in_managers[0].clone();
        }
    }
}
