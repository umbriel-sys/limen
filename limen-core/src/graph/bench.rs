//! (Work)bench [test] Graph implementations.
//!
//! This module contains two hand-written graph implementations that exactly
//! mirror what `limen-codegen` / `limen-build` produce for the following DSL
//! input. They exist so that the codegen output is always verifiable against a
//! known-correct, reviewed reference.
//!
//! ## Two separate codegen invocations
//!
//! `TestPipeline` (no-`std`, single-threaded) and `TestPipelineStd` (std,
//! concurrent) are produced by **two independent codegen invocations** of the
//! same logical pipeline. In a real project using the builder API this looks
//! like:
//!
//! ```text
//! // Invocation 1 — no_std graph (single-threaded runtime):
//! GraphBuilder::new()
//!     .vis(pub)
//!     .name("TestPipeline")
//!     .node(Node::new(0).ty("TestCounterSourceTensor<C, 32>").in_ports(0).out_ports(1)
//!           .in_payload("()").out_payload("TestTensor").name("src").ingress_policy("Q_32_POLICY"))
//!     .node(Node::new(1).ty("TestIdentityModelNodeTensor<32>").in_ports(1).out_ports(1)
//!           .in_payload("TestTensor").out_payload("TestTensor").name("map"))
//!     .node(Node::new(2).ty("TestSinkNodeTensor").in_ports(1).out_ports(0)
//!           .in_payload("TestTensor").out_payload("()").name("snk"))
//!     .edge(Edge::new(0).ty("Q32").payload("TestTensor").manager("StaticMemoryManager<TestTensor, 8>")
//!           .from(0, 0).to(1, 0).policy("Q_32_POLICY").name("e0"))
//!     .edge(Edge::new(1).ty("Q32").payload("TestTensor").manager("StaticMemoryManager<TestTensor, 8>")
//!           .from(1, 0).to(2, 0).policy("Q_32_POLICY").name("e1"))
//!     .concurrent(false)          // ← no ScopedGraphApi emitted
//!     .finish()
//!
//! // Invocation 2 — std graph (concurrent runtime, ScopedGraphApi):
//! GraphBuilder::new()
//!     // ... same nodes ...
//!     .edge(Edge::new(0).ty("ConcurrentEdge").payload("TestTensor").manager("ConcurrentMemoryManager<TestTensor>")
//!           .from(0, 0).to(1, 0).policy("Q_32_POLICY").name("e0"))
//!     .edge(Edge::new(1).ty("ConcurrentEdge").payload("TestTensor").manager("ConcurrentMemoryManager<TestTensor>")
//!           .from(1, 0).to(2, 0).policy("Q_32_POLICY").name("e1"))
//!     .concurrent(true)           // ← emits ScopedGraphApi + run_scoped impl
//!     .finish()
//! ```
//!
//! When using the proc-macro (`limen-build`), the second invocation is written
//! with the `concurrent;` keyword in the DSL block. The edge types must be
//! changed to `ConcurrentEdge`/`ConcurrentMemoryManager` manually (or via a
//! separate `define_graph!` block) since the DSL does not auto-promote queue
//! types.
//!
//! ## Why two invocations instead of one?
//!
//! The graph struct is monomorphized over its concrete edge and manager types.
//! A no-`std` graph uses `StaticRing<N>` / `StaticMemoryManager<P, N>`;
//! a concurrent graph uses `ConcurrentEdge` / `ConcurrentMemoryManager<P>`.
//! These are structurally different types — they cannot be unified in a single
//! struct. The codegen therefore requires a dedicated invocation per target
//! flavor, each producing a distinct named struct.

use crate::{
    edge::{Edge as _, EdgeOccupancy, NoQueue},
    errors::{GraphError, NodeError},
    graph::{GraphApi, GraphEdgeAccess, GraphNodeAccess, GraphNodeContextBuilder, GraphNodeTypes},
    node::{
        bench::{TestCounterSourceTensor, TestIdentityModelNodeTensor, TestSinkNodeTensor},
        sink::SinkNode,
        source::{Source as _, SourceNode, EXTERNAL_INGRESS_NODE},
        Node as _, StepContext, StepResult,
    },
    policy::{AdmissionPolicy, EdgePolicy, NodePolicy, OverBudgetAction},
    prelude::{
        EdgeDescriptor, EdgeLink, NodeDescriptor, NodeLink, PlatformClock, StaticMemoryManager,
        Telemetry, TestTensor,
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
type Mgr32 = StaticMemoryManager<TestTensor, 8>;

// Test source node types.
#[allow(type_alias_bounds)]
type SrcNode<SrcClk: PlatformClock> =
    SourceNode<TestCounterSourceTensor<SrcClk, 32>, TestTensor, 1>;
const INGRESS_POLICY: EdgePolicy = Q_32_POLICY;

// Test model node types.
const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeTensor<TEST_MAX_BATCH>;

// Test sink node types.
type SnkNode = SinkNode<TestSinkNodeTensor, TestTensor, 1>;

/// concrete graph implementation used for testing.
#[allow(clippy::complexity)]
pub struct TestPipeline<SrcClk: PlatformClock> {
    /// Nodes held in the graph.
    nodes: (
        NodeLink<SrcNode<SrcClk>, 0, 1, (), TestTensor>,
        NodeLink<MapNode, 1, 1, TestTensor, TestTensor>,
        NodeLink<SnkNode, 1, 0, TestTensor, ()>,
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
            NodeLink::<SrcNode<SrcClk>, 0, 1, (), TestTensor>::new(
                node_0,
                NodeIndex::from(0usize),
                Some("src"),
            ),
            NodeLink::<MapNode, 1, 1, TestTensor, TestTensor>::new(
                node_1,
                NodeIndex::from(1usize),
                Some("map"),
            ),
            NodeLink::<SnkNode, 1, 0, TestTensor, ()>::new(
                node_2,
                NodeIndex::from(2usize),
                Some("snk"),
            ),
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
}

// ===== GraphNodeAccess<I> =====
impl<SrcClk: PlatformClock> GraphNodeAccess<0> for TestPipeline<SrcClk> {
    type Node = NodeLink<SrcNode<SrcClk>, 0, 1, (), TestTensor>;
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
    type Node = NodeLink<MapNode, 1, 1, TestTensor, TestTensor>;
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
    type Node = NodeLink<SnkNode, 1, 0, TestTensor, ()>;
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
    type OutP = TestTensor;
    type InQ = NoQueue;
    type OutQ = Q32;
    type InM = StaticMemoryManager<(), 1>;
    type OutM = Mgr32;
}
// node 1: IN=1, OUT=1
impl<SrcClk: PlatformClock> GraphNodeTypes<1, 1, 1> for TestPipeline<SrcClk> {
    type InP = TestTensor;
    type OutP = TestTensor;
    type InQ = Q32;
    type OutQ = Q32;
    type InM = Mgr32;
    type OutM = Mgr32;
}
// node 2: IN=1, OUT=0
impl<SrcClk: PlatformClock> GraphNodeTypes<2, 1, 0> for TestPipeline<SrcClk> {
    type InP = TestTensor;
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
    Self: GraphNodeAccess<0, Node = NodeLink<SrcNode<SrcClk>, 0, 1, (), TestTensor>>,
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
    Self: GraphNodeAccess<1, Node = NodeLink<MapNode, 1, 1, TestTensor, TestTensor>>,
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
    Self: GraphNodeAccess<2, Node = NodeLink<SnkNode, 1, 0, TestTensor, ()>>,
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

/// Concurrent (std) graph implementation — the output of a second, separate
/// codegen invocation with `.concurrent(true)` (builder API) or `concurrent;`
/// (DSL).
///
/// This module hand-mirrors the code that `limen-codegen` emits when
/// `emit_concurrent = true`. It serves as a reference implementation and a
/// regression guard: if this module stops compiling or its tests fail, the
/// corresponding codegen change is broken.
///
/// Key structural differences from `TestPipeline` (the first invocation):
///
/// - All real edges use `ConcurrentEdge` (Arc-backed, `Clone + Send + Sync`).
/// - All managers use `ConcurrentMemoryManager<u32>`.
/// - The struct has only three fields: `nodes`, `edges`, `managers`.
///   Ingress edges are **not** stored — their occupancy and policies are
///   obtained from the owning source node at runtime (same as codegen output).
/// - A `ScopedGraphApi<3, 3>` impl is emitted with `where` bounds requiring
///   `ConcurrentEdge: ScopedEdge` and `ConcurrentMemoryManager<u32>: ScopedManager<u32>`.
/// - `run_scoped` uses `ScopedEdge::scoped_handle` / `ScopedManager::scoped_handle`
///   instead of `.clone()`, enabling future lock-free, non-Clone edge types.
#[cfg(feature = "std")]
pub mod concurrent_graph {
    use super::*;

    use crate::{
        edge::{spsc_concurrent::ConcurrentEdge, EdgeOccupancy, NoQueue},
        errors::{GraphError, NodeError},
        graph::{
            GraphApi, GraphEdgeAccess, GraphNodeAccess, GraphNodeContextBuilder, GraphNodeTypes,
            ScopedGraphApi,
        },
        node::{
            bench::{TestCounterSourceTensor, TestIdentityModelNodeTensor},
            source::SourceNode,
            StepContext, StepResult,
        },
        policy::EdgePolicy,
        prelude::{
            ConcurrentMemoryManager, EdgeDescriptor, EdgeLink, NodeDescriptor, NodeLink,
            PlatformClock, StaticMemoryManager, Telemetry, WorkerDecision, WorkerScheduler,
        },
        types::{EdgeIndex, NodeIndex, PortId, PortIndex},
    };

    type ConcMgr32 = ConcurrentMemoryManager<TestTensor>;

    #[allow(type_alias_bounds)]
    type SrcNode<SrcClk: PlatformClock + Send + 'static> =
        SourceNode<TestCounterSourceTensor<SrcClk, 32>, TestTensor, 1>;

    const TEST_MAX_BATCH: usize = 32;
    type MapNode = TestIdentityModelNodeTensor<TEST_MAX_BATCH>;
    type SnkNode = SinkNode<TestSinkNodeTensor, TestTensor, 1>;

    /// Concrete std graph using `ConcurrentEdge` (Arc-backed, Clone+Send+Sync).
    ///
    /// Mirrors the output of a `concurrent(true)` codegen invocation.
    /// Ingress edges are not stored: occupancy and policy are obtained from the
    /// source node at runtime, matching codegen output exactly.
    #[allow(clippy::complexity)]
    pub struct TestPipelineStd<SrcClk: PlatformClock + Send + 'static> {
        nodes: (
            NodeLink<SrcNode<SrcClk>, 0, 1, (), TestTensor>,
            NodeLink<MapNode, 1, 1, TestTensor, TestTensor>,
            NodeLink<SnkNode, 1, 0, TestTensor, ()>,
        ),
        edges: (EdgeLink<ConcurrentEdge>, EdgeLink<ConcurrentEdge>),
        managers: (ConcMgr32, ConcMgr32),
    }

    impl<SrcClk: PlatformClock + Send + 'static> TestPipelineStd<SrcClk> {
        /// Construct a new concurrent graph instance.
        ///
        /// Mirrors the `new(..)` constructor emitted by codegen for a
        /// `concurrent(true)` graph. Ingress probe edges are not created here —
        /// ingress occupancy and policy are read directly from the source node.
        #[inline]
        pub fn new(
            node_0: impl Into<SrcNode<SrcClk>>,
            node_1: MapNode,
            node_2: impl Into<SnkNode>,
            q_1: ConcurrentEdge,
            q_2: ConcurrentEdge,
            mgr_1: ConcMgr32,
            mgr_2: ConcMgr32,
        ) -> Self {
            let node_0: SrcNode<SrcClk> = node_0.into();
            let node_2: SnkNode = node_2.into();

            let n0 = NodeLink::<SrcNode<SrcClk>, 0, 1, (), TestTensor>::new(
                node_0,
                NodeIndex::from(0usize),
                Some("src"),
            );
            let n1 = NodeLink::<MapNode, 1, 1, TestTensor, TestTensor>::new(
                node_1,
                NodeIndex::from(1usize),
                Some("map"),
            );
            let n2 = NodeLink::<SnkNode, 1, 0, TestTensor, ()>::new(
                node_2,
                NodeIndex::from(2usize),
                Some("snk"),
            );

            let e0 = EdgeLink::new(
                q_1,
                EdgeIndex::from(1usize),
                PortId::new(NodeIndex::from(0usize), PortIndex::from(0)),
                PortId::new(NodeIndex::from(1usize), PortIndex::from(0)),
                Q_32_POLICY,
                Some("e0"),
            );
            let e1 = EdgeLink::new(
                q_2,
                EdgeIndex::from(2usize),
                PortId::new(NodeIndex::from(1usize), PortIndex::from(0)),
                PortId::new(NodeIndex::from(2usize), PortIndex::from(0)),
                Q_32_POLICY,
                Some("e1"),
            );

            Self {
                nodes: (n0, n1, n2),
                edges: (e0, e1),
                managers: (mgr_1, mgr_2),
            }
        }

        /// Execute all nodes concurrently via `std::thread::scope`.
        ///
        /// This method mirrors the body of `run_scoped` as emitted by codegen
        /// when `emit_concurrent = true`. Structure:
        ///
        /// - Step 1: edge policy copies (before node borrows; `EdgePolicy: Copy`)
        /// - Step 2: per-node scoped edge + manager handles via `ScopedEdge` /
        ///   `ScopedManager` — future lock-free non-Clone types work transparently
        /// - Step 3: per-worker telemetry clones
        /// - Step 4: disjoint node borrows (Rust tracks tuple fields separately)
        /// - Step 5: one scoped thread per node, scheduler-driven
        fn run_scoped_impl<C, T, S>(&mut self, clock: C, telemetry: T, scheduler: S)
        where
            C: PlatformClock + Clone + Send + Sync + 'static,
            T: Telemetry + Clone + Send + 'static,
            S: WorkerScheduler + 'static,
            SrcNode<SrcClk>: Send,
            MapNode: Send,
            SnkNode: Send,
        {
            // --- Step 1: edge policy copies ---
            let pol_0 = *self.edges.0.policy();
            let pol_1 = *self.edges.1.policy();

            // --- Step 2: per-node scoped edge handles + manager handles ---
            // Uses ScopedEdge / ScopedManager instead of Clone so future
            // lock-free, non-Clone edge and manager types work transparently.
            //
            // node 0 (source): no inputs, 1 output (real edge 0)
            let out_e_0_0 = crate::edge::ScopedEdge::scoped_handle(
                self.edges.0.queue(),
                crate::edge::EdgeHandleKind::Producer,
            );
            let out_m_0_0 = crate::memory::manager::ScopedManager::scoped_handle(&self.managers.0);
            // node 1 (model): 1 input (real edge 0), 1 output (real edge 1)
            let in_e_1_0 = crate::edge::ScopedEdge::scoped_handle(
                self.edges.0.queue(),
                crate::edge::EdgeHandleKind::Consumer,
            );
            let in_m_1_0 = crate::memory::manager::ScopedManager::scoped_handle(&self.managers.0);
            let out_e_1_0 = crate::edge::ScopedEdge::scoped_handle(
                self.edges.1.queue(),
                crate::edge::EdgeHandleKind::Producer,
            );
            let out_m_1_0 = crate::memory::manager::ScopedManager::scoped_handle(&self.managers.1);
            // node 2 (sink): 1 input (real edge 1), no outputs
            let in_e_2_0 = crate::edge::ScopedEdge::scoped_handle(
                self.edges.1.queue(),
                crate::edge::EdgeHandleKind::Consumer,
            );
            let in_m_2_0 = crate::memory::manager::ScopedManager::scoped_handle(&self.managers.1);

            // --- Step 3: per-worker telemetry ---
            let telem_0 = telemetry.clone();
            let telem_1 = telemetry.clone();
            let telem_2 = telemetry;

            // --- Step 4: disjoint node borrows ---
            let n0 = &mut self.nodes.0;
            let n1 = &mut self.nodes.1;
            let n2 = &mut self.nodes.2;

            let clock_ref = &clock;
            let sched_ref = &scheduler;

            ::std::thread::scope(|scope| {
                // --- Node 0: source (readiness from ingress_occupancy) ---
                {
                    fn _assert_send<_T: core::marker::Send>() {}
                    _assert_send::<SrcNode<SrcClk>>();
                }
                scope.spawn(move || {
                    let mut out_e_0_0 = out_e_0_0;
                    let mut out_m_0_0 = out_m_0_0;
                    let mut telem = telem_0;

                    let mut state =
                        crate::scheduling::WorkerState::new(0, 3, clock_ref.now_ticks());
                    loop {
                        state.current_tick = clock_ref.now_ticks();

                        let _ingress_occ = n0.node().source_ref().ingress_occupancy();
                        let _any_input = *_ingress_occ.items() > 0;

                        let mut _max_wm = crate::policy::WatermarkState::BelowSoft;
                        {
                            let _occ = crate::edge::Edge::occupancy(&out_e_0_0, &pol_0);
                            if *_occ.watermark() > _max_wm {
                                _max_wm = *_occ.watermark();
                            }
                        }
                        state.backpressure = _max_wm;

                        state.readiness = if !_any_input {
                            crate::scheduling::Readiness::NotReady
                        } else if _max_wm >= crate::policy::WatermarkState::BetweenSoftAndHard {
                            crate::scheduling::Readiness::ReadyUnderPressure
                        } else {
                            crate::scheduling::Readiness::Ready
                        };

                        match sched_ref.decide(&state) {
                            WorkerDecision::Step => {
                                let mut ctx = crate::node::StepContext::new(
                                    [] as [&mut NoQueue; 0],
                                    [&mut out_e_0_0],
                                    [] as [&mut StaticMemoryManager<(), 1>; 0],
                                    [&mut out_m_0_0],
                                    [],
                                    [pol_0],
                                    0u32,
                                    [],
                                    [1u32],
                                    clock_ref,
                                    &mut telem,
                                );
                                match n0.step(&mut ctx) {
                                    Ok(sr) => {
                                        state.last_step = Some(sr);
                                        state.last_error = false;
                                    }
                                    Err(_e) => {
                                        state.last_step = None;
                                        state.last_error = true;
                                    }
                                }
                            }
                            WorkerDecision::WaitMicros(d) => {
                                ::std::thread::sleep(::std::time::Duration::from_micros(d));
                                state.last_step = None;
                                state.last_error = false;
                            }
                            WorkerDecision::Stop => break,
                        }
                    }
                });

                // --- Node 1: model (readiness from input edge 0) ---
                {
                    fn _assert_send<_T: core::marker::Send>() {}
                    _assert_send::<MapNode>();
                }
                scope.spawn(move || {
                    let mut in_e_1_0 = in_e_1_0;
                    let mut in_m_1_0 = in_m_1_0;
                    let mut out_e_1_0 = out_e_1_0;
                    let mut out_m_1_0 = out_m_1_0;
                    let mut telem = telem_1;

                    let mut state =
                        crate::scheduling::WorkerState::new(1, 3, clock_ref.now_ticks());
                    loop {
                        state.current_tick = clock_ref.now_ticks();

                        let _any_input =
                            *crate::edge::Edge::occupancy(&in_e_1_0, &pol_0).items() > 0;

                        let mut _max_wm = crate::policy::WatermarkState::BelowSoft;
                        {
                            let _occ = crate::edge::Edge::occupancy(&out_e_1_0, &pol_1);
                            if *_occ.watermark() > _max_wm {
                                _max_wm = *_occ.watermark();
                            }
                        }
                        state.backpressure = _max_wm;

                        state.readiness = if !_any_input {
                            crate::scheduling::Readiness::NotReady
                        } else if _max_wm >= crate::policy::WatermarkState::BetweenSoftAndHard {
                            crate::scheduling::Readiness::ReadyUnderPressure
                        } else {
                            crate::scheduling::Readiness::Ready
                        };

                        match sched_ref.decide(&state) {
                            WorkerDecision::Step => {
                                let mut ctx = crate::node::StepContext::new(
                                    [&mut in_e_1_0],
                                    [&mut out_e_1_0],
                                    [&mut in_m_1_0],
                                    [&mut out_m_1_0],
                                    [pol_0],
                                    [pol_1],
                                    1u32,
                                    [1u32],
                                    [2u32],
                                    clock_ref,
                                    &mut telem,
                                );
                                match n1.step(&mut ctx) {
                                    Ok(sr) => {
                                        state.last_step = Some(sr);
                                        state.last_error = false;
                                    }
                                    Err(_e) => {
                                        state.last_step = None;
                                        state.last_error = true;
                                    }
                                }
                            }
                            WorkerDecision::WaitMicros(d) => {
                                ::std::thread::sleep(::std::time::Duration::from_micros(d));
                                state.last_step = None;
                                state.last_error = false;
                            }
                            WorkerDecision::Stop => break,
                        }
                    }
                });

                // --- Node 2: sink (readiness from input edge 1, no outputs) ---
                {
                    fn _assert_send<_T: core::marker::Send>() {}
                    _assert_send::<SnkNode>();
                }
                scope.spawn(move || {
                    let mut in_e_2_0 = in_e_2_0;
                    let mut in_m_2_0 = in_m_2_0;
                    let mut telem = telem_2;

                    let mut state =
                        crate::scheduling::WorkerState::new(2, 3, clock_ref.now_ticks());
                    loop {
                        state.current_tick = clock_ref.now_ticks();

                        let _any_input =
                            *crate::edge::Edge::occupancy(&in_e_2_0, &pol_1).items() > 0;

                        let mut _max_wm = crate::policy::WatermarkState::BelowSoft;
                        // no output edges — no watermark check
                        state.backpressure = _max_wm;

                        state.readiness = if !_any_input {
                            crate::scheduling::Readiness::NotReady
                        } else if _max_wm >= crate::policy::WatermarkState::BetweenSoftAndHard {
                            crate::scheduling::Readiness::ReadyUnderPressure
                        } else {
                            crate::scheduling::Readiness::Ready
                        };

                        match sched_ref.decide(&state) {
                            WorkerDecision::Step => {
                                let mut ctx = crate::node::StepContext::new(
                                    [&mut in_e_2_0],
                                    [] as [&mut NoQueue; 0],
                                    [&mut in_m_2_0],
                                    [] as [&mut StaticMemoryManager<(), 1>; 0],
                                    [pol_1],
                                    [],
                                    2u32,
                                    [2u32],
                                    [],
                                    clock_ref,
                                    &mut telem,
                                );
                                match n2.step(&mut ctx) {
                                    Ok(sr) => {
                                        state.last_step = Some(sr);
                                        state.last_error = false;
                                    }
                                    Err(_e) => {
                                        state.last_step = None;
                                        state.last_error = true;
                                    }
                                }
                            }
                            WorkerDecision::WaitMicros(d) => {
                                ::std::thread::sleep(::std::time::Duration::from_micros(d));
                                state.last_step = None;
                                state.last_error = false;
                            }
                            WorkerDecision::Stop => break,
                        }
                    }
                });
            });
        }
    }

    // ===== ScopedGraphApi<3, 3> =====
    impl<SrcClk: PlatformClock + Clone + Send + Sync + 'static> ScopedGraphApi<3, 3>
        for TestPipelineStd<SrcClk>
    where
        SrcNode<SrcClk>: Send,
        ConcurrentEdge: crate::edge::ScopedEdge,
        ConcMgr32: crate::memory::manager::ScopedManager<TestTensor>,
    {
        fn run_scoped<C, T, S>(&mut self, clock: C, telemetry: T, scheduler: S)
        where
            C: PlatformClock + Clone + Send + Sync + 'static,
            T: Telemetry + Clone + Send + 'static,
            S: WorkerScheduler + 'static,
        {
            self.run_scoped_impl(clock, telemetry, scheduler)
        }
    }

    // ===== GraphApi<3, 3> =====
    impl<SrcClk: PlatformClock + Send + 'static> GraphApi<3, 3> for TestPipelineStd<SrcClk> {
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
                    PortId::new(EXTERNAL_INGRESS_NODE, PortIndex::from(0)),
                    PortId::new(NodeIndex::from(0usize), PortIndex::from(0)),
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
                self.nodes.0.node().source_ref().ingress_policy(),
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
                    match ed.id().as_usize() {
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
    }

    // ===== GraphNodeAccess<I> =====
    impl<SrcClk: PlatformClock + Send + 'static> GraphNodeAccess<0> for TestPipelineStd<SrcClk> {
        type Node = NodeLink<SrcNode<SrcClk>, 0, 1, (), TestTensor>;
        #[inline]
        fn node_ref(&self) -> &Self::Node {
            &self.nodes.0
        }
        #[inline]
        fn node_mut(&mut self) -> &mut Self::Node {
            &mut self.nodes.0
        }
    }
    impl<SrcClk: PlatformClock + Send + 'static> GraphNodeAccess<1> for TestPipelineStd<SrcClk> {
        type Node = NodeLink<MapNode, 1, 1, TestTensor, TestTensor>;
        #[inline]
        fn node_ref(&self) -> &Self::Node {
            &self.nodes.1
        }
        #[inline]
        fn node_mut(&mut self) -> &mut Self::Node {
            &mut self.nodes.1
        }
    }
    impl<SrcClk: PlatformClock + Send + 'static> GraphNodeAccess<2> for TestPipelineStd<SrcClk> {
        type Node = NodeLink<SnkNode, 1, 0, TestTensor, ()>;
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
    impl<SrcClk: PlatformClock + Send + 'static> GraphEdgeAccess<1> for TestPipelineStd<SrcClk> {
        type Edge = EdgeLink<ConcurrentEdge>;
        #[inline]
        fn edge_ref(&self) -> &Self::Edge {
            &self.edges.0
        }
        #[inline]
        fn edge_mut(&mut self) -> &mut Self::Edge {
            &mut self.edges.0
        }
    }
    impl<SrcClk: PlatformClock + Send + 'static> GraphEdgeAccess<2> for TestPipelineStd<SrcClk> {
        type Edge = EdgeLink<ConcurrentEdge>;
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
    impl<SrcClk: PlatformClock + Send + 'static> GraphNodeTypes<0, 0, 1> for TestPipelineStd<SrcClk> {
        type InP = ();
        type OutP = TestTensor;
        type InQ = NoQueue;
        type OutQ = ConcurrentEdge;
        type InM = StaticMemoryManager<(), 1>;
        type OutM = ConcMgr32;
    }
    impl<SrcClk: PlatformClock + Send + 'static> GraphNodeTypes<1, 1, 1> for TestPipelineStd<SrcClk> {
        type InP = TestTensor;
        type OutP = TestTensor;
        type InQ = ConcurrentEdge;
        type OutQ = ConcurrentEdge;
        type InM = ConcMgr32;
        type OutM = ConcMgr32;
    }
    impl<SrcClk: PlatformClock + Send + 'static> GraphNodeTypes<2, 1, 0> for TestPipelineStd<SrcClk> {
        type InP = TestTensor;
        type OutP = ();
        type InQ = ConcurrentEdge;
        type OutQ = NoQueue;
        type InM = ConcMgr32;
        type OutM = StaticMemoryManager<(), 1>;
    }

    // ===== GraphNodeContextBuilder<I, IN, OUT> =====

    // node 0: in=[], out=[e0]
    impl<SrcClk: PlatformClock + Send + 'static> GraphNodeContextBuilder<0, 0, 1>
        for TestPipelineStd<SrcClk>
    where
        Self: GraphNodeAccess<0, Node = NodeLink<SrcNode<SrcClk>, 0, 1, (), TestTensor>>,
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
            StepContext::new(
                [],
                [self.edges.0.queue_mut()],
                [],
                [&mut self.managers.0],
                [],
                [out0_policy],
                0u32,
                [],
                [1u32],
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
            let outputs = [self.edges.0.queue_mut()];
            let out_managers = [&mut self.managers.0];
            let mut ctx = StepContext::new(
                [],
                outputs,
                [],
                out_managers,
                [],
                [out0_policy],
                0u32,
                [],
                [1u32],
                clock,
                telemetry,
            );
            f(node, &mut ctx)
        }
    }

    // node 1: in=[e0], out=[e1]
    impl<SrcClk: PlatformClock + Send + 'static> GraphNodeContextBuilder<1, 1, 1>
        for TestPipelineStd<SrcClk>
    where
        Self: GraphNodeAccess<1, Node = NodeLink<MapNode, 1, 1, TestTensor, TestTensor>>,
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
            StepContext::new(
                [self.edges.0.queue_mut()],
                [self.edges.1.queue_mut()],
                [&mut self.managers.0],
                [&mut self.managers.1],
                [in0_policy],
                [out1_policy],
                1u32,
                [1u32],
                [2u32],
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
            let inputs = [self.edges.0.queue_mut()];
            let outputs = [self.edges.1.queue_mut()];
            let in_mgrs = [&mut self.managers.0];
            let out_mgrs = [&mut self.managers.1];
            let mut ctx = StepContext::new(
                inputs,
                outputs,
                in_mgrs,
                out_mgrs,
                [in0_policy],
                [out1_policy],
                1u32,
                [1u32],
                [2u32],
                clock,
                telemetry,
            );
            f(node, &mut ctx)
        }
    }

    // node 2: in=[e1], out=[]
    impl<SrcClk: PlatformClock + Send + 'static> GraphNodeContextBuilder<2, 1, 0>
        for TestPipelineStd<SrcClk>
    where
        Self: GraphNodeAccess<2, Node = NodeLink<SnkNode, 1, 0, TestTensor, ()>>,
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
            StepContext::new(
                [self.edges.1.queue_mut()],
                [],
                [&mut self.managers.1],
                [],
                [in1_policy],
                [],
                2u32,
                [2u32],
                [],
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
            let inputs = [self.edges.1.queue_mut()];
            let in_mgrs = [&mut self.managers.1];
            let mut ctx = StepContext::new(
                inputs,
                [],
                in_mgrs,
                [],
                [in1_policy],
                [],
                2u32,
                [2u32],
                [],
                clock,
                telemetry,
            );
            f(node, &mut ctx)
        }
    }
}
