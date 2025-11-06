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
//!         0: { ty: TestCounterSourceU32, in_ports: 0, out_ports: 1, in_payload: (),  out_payload: u32, name: Some("src") },
//!         1: { ty: TestIdentityModelNodeU32, in_ports: 1, out_ports: 1, in_payload: u32, out_payload: u32, name: Some("map") },
//!         2: { ty: TestSinkNodeU32,   in_ports: 1, out_ports: 0, in_payload: u32, out_payload: (),  name: Some("snk") }
//!     }
//!
//!     edges {
//!         0: { ty: Q32, payload: u32, from: (0,0), to: (1,0), policy: Q_32_POLICY, name: Some("e0") },
//!         1: { ty: Q32, payload: u32, from: (1,0), to: (2,0), policy: Q_32_POLICY, name: Some("e1") }
//!     }
//!
//!     wiring {
//!         node 0: { in: [ ],   out: [ 0 ] },
//!         node 1: { in: [ 0 ], out: [ 1 ] },
//!         node 2: { in: [ 1 ], out: [ ] }
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
    policy::EdgePolicy,
    prelude::{EdgeDescriptor, EdgeLink, NodeDescriptor, NodeLink, PlatformClock, Telemetry},
    types::{EdgeIndex, NodeIndex, PortId, PortIndex},
};

// Test edge types.
type Q32 = crate::edge::bench::TestSpscRingBuf<crate::message::Message<u32>, 8>;
const Q_32_POLICY: EdgePolicy = EdgePolicy {
    caps: crate::policy::QueueCaps {
        max_items: 8,
        soft_items: 8,
        max_bytes: None,
        soft_bytes: None,
    },
    over_budget: crate::policy::OverBudgetAction::Drop,
    admission: crate::policy::AdmissionPolicy::DropOldest,
};

// Test source node types.
type SrcNode = SourceNode<TestCounterSourceU32_2, u32, 1>;
const INGRESS_POLICY: EdgePolicy = Q_32_POLICY;

// Test model node types.
const TEST_MAX_BATCH: usize = 32;
type MapNode = TestIdentityModelNodeU32_2<TEST_MAX_BATCH>;

// Test sink node types.
type SnkNode = SinkNode<TestSinkNodeU32_2, u32, 1>;

/// concrete graph implementation used for testing.
#[allow(clippy::complexity)]
pub struct TestPipeline {
    /// Nodes held in the graph.
    nodes: (
        NodeLink<SrcNode, 0, 1, (), u32>,
        NodeLink<MapNode, 1, 1, u32, u32>,
        NodeLink<SnkNode, 1, 0, u32, ()>,
    ),
    /// Edges held in the graph.
    edges: (EdgeLink<Q32, u32>, EdgeLink<Q32, u32>),
}

impl TestPipeline {
    /// Returns a TestPipeline graph given the nodes and edges.
    #[inline]
    pub fn new(
        node_0: impl Into<SrcNode>,
        node_1: MapNode,
        node_2: impl Into<SnkNode>,
        q_0: Q32,
        q_1: Q32,
    ) -> Self {
        let node_0: SrcNode = node_0.into();
        let node_2: SnkNode = node_2.into();

        let nodes = (
            NodeLink::<SrcNode, 0, 1, (), u32>::new(node_0, NodeIndex::from(0usize), Some("src")),
            NodeLink::<MapNode, 1, 1, u32, u32>::new(node_1, NodeIndex::from(1usize), Some("map")),
            NodeLink::<SnkNode, 1, 0, u32, ()>::new(node_2, NodeIndex::from(2usize), Some("snk")),
        );

        let edges = (
            EdgeLink::<Q32, u32>::new(
                q_0,
                EdgeIndex::from(1usize),
                PortId {
                    node: NodeIndex::from(0usize),
                    port: PortIndex(0usize),
                },
                PortId {
                    node: NodeIndex::from(1usize),
                    port: PortIndex(0usize),
                },
                Q_32_POLICY,
                Some("e0"),
            ),
            EdgeLink::<Q32, u32>::new(
                q_1,
                EdgeIndex::from(2usize),
                PortId {
                    node: NodeIndex::from(1usize),
                    port: PortIndex(0usize),
                },
                PortId {
                    node: NodeIndex::from(2usize),
                    port: PortIndex(0usize),
                },
                Q_32_POLICY,
                Some("e1"),
            ),
        );

        Self { nodes, edges }
    }
}

// ===== GraphApi<3,3> =====
impl GraphApi<3, 3> for TestPipeline {
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
            EdgeDescriptor {
                id: EdgeIndex::from(0usize),
                upstream: PortId {
                    node: EXTERNAL_INGRESS_NODE,
                    port: PortIndex(0usize),
                },
                downstream: PortId {
                    node: NodeIndex::from(0usize),
                    port: PortIndex(0usize),
                },
                name: Some("ingress0"),
            },
            self.edges.0.descriptor(),
            self.edges.1.descriptor(),
        ]
    }

    #[inline]
    fn edge_occupancy_for<const E: usize>(&self) -> Result<EdgeOccupancy, GraphError> {
        let occ = match E {
            0 => {
                let src = self.nodes.0.node().source_ref();
                src.ingress_occupancy(&INGRESS_POLICY)
            }
            1 => {
                let e = &self.edges.0;
                e.occupancy(&e.policy())
            }
            2 => {
                let e = &self.edges.1;
                e.occupancy(&e.policy())
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
            if ed.upstream.node == node_idx || ed.downstream.node == node_idx {
                let ei = (ed.id).0; // EdgeIndex(pub usize)
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
impl GraphNodeAccess<0> for TestPipeline {
    type Node = NodeLink<SrcNode, 0, 1, (), u32>;
    #[inline]
    fn node_ref(&self) -> &Self::Node {
        &self.nodes.0
    }
    #[inline]
    fn node_mut(&mut self) -> &mut Self::Node {
        &mut self.nodes.0
    }
}
impl GraphNodeAccess<1> for TestPipeline {
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
impl GraphNodeAccess<2> for TestPipeline {
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
impl GraphEdgeAccess<1> for TestPipeline {
    type Edge = EdgeLink<Q32, u32>;
    #[inline]
    fn edge_ref(&self) -> &Self::Edge {
        &self.edges.0
    }
    #[inline]
    fn edge_mut(&mut self) -> &mut Self::Edge {
        &mut self.edges.0
    }
}
impl GraphEdgeAccess<2> for TestPipeline {
    type Edge = EdgeLink<Q32, u32>;
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
impl GraphNodeTypes<0, 0, 1> for TestPipeline {
    type InP = ();
    type OutP = u32;
    type InQ = NoQueue<()>;
    type OutQ = Q32;
}
// node 1: IN=1, OUT=1
impl GraphNodeTypes<1, 1, 1> for TestPipeline {
    type InP = u32;
    type OutP = u32;
    type InQ = Q32;
    type OutQ = Q32;
}
// node 2: IN=1, OUT=0
impl GraphNodeTypes<2, 1, 0> for TestPipeline {
    type InP = u32;
    type OutP = ();
    type InQ = Q32;
    type OutQ = NoQueue<()>;
}

// ===== GraphNodeContextBuilder<I, IN, OUT> =====
// node 0: in=[], out=[edge id 1]
impl GraphNodeContextBuilder<0, 0, 1> for TestPipeline {
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
        C,
        T,
    >
    where
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let out0_policy = self.edges.0.policy();

        let inputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::InQ; 0] = [/* empty */];
        let outputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1] = [self.edges.0.queue_mut()];

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
            C,
            T,
        >::new(
            inputs,
            outputs,
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
        // Disjoint borrows: nodes and edges are separate fields.
        let node = &mut self.nodes.0;

        let out0_policy = self.edges.0.policy();

        let inputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::InQ; 0] = [];
        let outputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1] = [self.edges.0.queue_mut()];

        let in_policies: [EdgePolicy; 0] = [/* empty */];
        let out_policies: [EdgePolicy; 1] = [out0_policy];

        let node_id: u32 = 0;
        let in_edge_ids: [u32; 0] = [/* empty */];
        let out_edge_ids: [u32; 1] = [1];

        let mut ctx = StepContext::new(
            inputs,
            outputs,
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
impl GraphNodeContextBuilder<1, 1, 1> for TestPipeline {
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
        C,
        T,
    >
    where
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let in0_policy = self.edges.0.policy();
        let out1_policy = self.edges.1.policy();

        let inputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::InQ; 1] = [self.edges.0.queue_mut()];
        let outputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1] = [self.edges.1.queue_mut()];

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
            C,
            T,
        >::new(
            inputs,
            outputs,
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

        let in0_policy = self.edges.0.policy();
        let out1_policy = self.edges.1.policy();

        let inputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::InQ; 1] = [self.edges.0.queue_mut()];
        let outputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1] = [self.edges.1.queue_mut()];

        let in_policies: [EdgePolicy; 1] = [in0_policy];
        let out_policies: [EdgePolicy; 1] = [out1_policy];

        let node_id: u32 = 1;
        let in_edge_ids: [u32; 1] = [1];
        let out_edge_ids: [u32; 1] = [2];

        let mut ctx = StepContext::new(
            inputs,
            outputs,
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
impl GraphNodeContextBuilder<2, 1, 0> for TestPipeline {
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
        C,
        T,
    >
    where
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized,
    {
        let in1_policy = self.edges.1.policy();

        let inputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::InQ; 1] = [self.edges.1.queue_mut()];
        let outputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0] = [/* empty */];

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
            C,
            T,
        >::new(
            inputs,
            outputs,
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

        let in1_policy = self.edges.1.policy();

        let inputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::InQ; 1] = [self.edges.1.queue_mut()];
        let outputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0] = [];
        let in_policies: [EdgePolicy; 1] = [in1_policy];
        let out_policies: [EdgePolicy; 0] = [/* empty */];

        let node_id: u32 = 2;
        let in_edge_ids: [u32; 1] = [2];
        let out_edge_ids: [u32; 0] = [/* empty */];

        let mut ctx = StepContext::new(
            inputs,
            outputs,
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
        prelude::{EdgeDescriptor, NodeDescriptor, NodeLink, PlatformClock, Telemetry},
        types::{EdgeIndex, NodeIndex, PortId, PortIndex},
    };

    // ===== Endpoint aliases used by nodes in this std graph =====
    type InEpU32 = ConsumerEndpoint<u32, ConcurrentQueue<Q32>>;
    type OutEpU32 = ProducerEndpoint<u32, ConcurrentQueue<Q32>>;

    // Test source node types.
    type SrcNode = SourceNode<TestCounterSourceU32_2, u32, 1>;
    const INGRESS_POLICY: EdgePolicy = Q_32_POLICY;

    // Test model node types.
    const TEST_MAX_BATCH: usize = 32;
    type MapNode = TestIdentityModelNodeU32_2<TEST_MAX_BATCH>;

    // Test sink node types.
    type SnkNode = SinkNode<TestSinkNodeU32_2, u32, 1>;

    /// concrete graph implementation (std / concurrent).
    #[allow(clippy::complexity)]
    pub struct TestPipelineStd {
        // Nodes. We keep them as Options to support "move-out" for owned handoff.
        nodes: (
            Option<NodeLink<SrcNode, 0, 1, (), u32>>,
            Option<NodeLink<MapNode, 1, 1, u32, u32>>,
            Option<NodeLink<SnkNode, 1, 0, u32, ()>>,
        ),
        // Edges: one Arc<Mutex<_>> each + metadata.
        edges: (
            ConcurrentEdgeLink<Q32, u32>, // e0: 0->1
            ConcurrentEdgeLink<Q32, u32>, // e1: 1->2
        ),
        // Persistent endpoint views (borrowed path uses &mut to these).
        // Each edge has a consumer (downstream side) and a producer (upstream side).
        endpoints: (
            (InEpU32, OutEpU32), // e0: (to node1 in0, from node0 out0)
            (InEpU32, OutEpU32), // e1: (to node2 in0, from node1 out0)
        ),
        // Synthetic ingress edge (id 0) backed by atomic probe.
        ingress_edge: ConcurrentIngressEdgeLink<u32>,
        // Updater to be moved to node 0's worker; kept as Option so we can take().
        ingress_updater: Option<SourceIngressUpdater>,
        // Cache node descriptors so validation still works after nodes are moved out
        node_descs: [NodeDescriptor; 3],
    }

    impl TestPipelineStd {
        /// Build the std graph from nodes and concrete queues.
        #[inline]
        pub fn new(
            node_0: impl Into<SrcNode>,
            node_1: MapNode,
            node_2: impl Into<SnkNode>,
            q_0: Q32,
            q_1: Q32,
        ) -> Self {
            // Build nodes
            let node_0: SrcNode = node_0.into();
            let node_2: SnkNode = node_2.into();

            let nodes = (
                Some(NodeLink::<SrcNode, 0, 1, (), u32>::new(
                    node_0,
                    NodeIndex::from(0usize),
                    Some("src"),
                )),
                Some(NodeLink::<MapNode, 1, 1, u32, u32>::new(
                    node_1,
                    NodeIndex::from(1usize),
                    Some("map"),
                )),
                Some(NodeLink::<SnkNode, 1, 0, u32, ()>::new(
                    node_2,
                    NodeIndex::from(2usize),
                    Some("snk"),
                )),
            );

            // Build edges
            let e0 = ConcurrentEdgeLink::<Q32, u32>::new(
                q_0,
                EdgeIndex::from(1usize),
                PortId {
                    node: NodeIndex::from(0usize),
                    port: PortIndex(0),
                },
                PortId {
                    node: NodeIndex::from(1usize),
                    port: PortIndex(0),
                },
                Q_32_POLICY,
                Some("e0"),
            );
            let e1 = ConcurrentEdgeLink::<Q32, u32>::new(
                q_1,
                EdgeIndex::from(2usize),
                PortId {
                    node: NodeIndex::from(1usize),
                    port: PortIndex(0),
                },
                PortId {
                    node: NodeIndex::from(2usize),
                    port: PortIndex(0),
                },
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

            // Ingress probe pair: typed edge (held by graph) + updater (moved to worker 0)
            let (probe_edge, updater) = new_probe_edge_pair::<u32>();
            let ingress_edge = ConcurrentIngressEdgeLink::from_probe(
                probe_edge,
                EdgeIndex::from(0usize),
                PortId {
                    node: EXTERNAL_INGRESS_NODE,
                    port: PortIndex(0),
                },
                PortId {
                    node: NodeIndex::from(0usize),
                    port: PortIndex(0),
                },
                INGRESS_POLICY,
                Some("ingress0"),
            );

            let node_descs = [
                nodes.0.as_ref().unwrap().descriptor(),
                nodes.1.as_ref().unwrap().descriptor(),
                nodes.2.as_ref().unwrap().descriptor(),
            ];

            Self {
                nodes,
                edges: (e0, e1),
                endpoints,
                ingress_edge,
                ingress_updater: Some(updater),
                node_descs,
            }
        }
    }

    /// ===== std-only opaque owned-bundle used by GraphApi take/put =====
    pub enum TestPipelineStdOwnedBundle {
        /// node 0: out=[e1.out] + ingress updater.
        N0 {
            /// The detached node link for node 0 (TestSourceNodeU32).
            node: NodeLink<SrcNode, 0, 1, (), u32>,
            /// Owned output endpoint for edge e0.out.
            out0: OutEpU32,
            /// Static edge policy for out0 (e0).
            out0_policy: EdgePolicy,
            /// TODO: Comment..
            ingress_updater: SourceIngressUpdater,
        },
        /// node 1: in=[e1.in], out=[e2.out].
        N1 {
            /// The detached node link for node 1 (TestIdentityModelNodeU32 alias).
            node: NodeLink<MapNode, 1, 1, u32, u32>,
            /// Owned input endpoint for edge e0.in.
            in0: InEpU32,
            /// Owned output endpoint for edge e1.out.
            out1: OutEpU32,
            /// Static edge policy for in0 (e0).
            in0_policy: EdgePolicy,
            /// Static edge policy for out1 (e1).
            out1_policy: EdgePolicy,
        },
        /// node 2: in=[e2.in]
        N2 {
            /// The detached node link for node 2 (TestSinkNodeU32).
            node: NodeLink<SnkNode, 1, 0, u32, ()>,
            /// Owned input endpoint for edge e1.in.
            in1: InEpU32,
            /// Static edge policy for in1 (e1).
            in1_policy: EdgePolicy,
        },
    }

    // ===== GraphApi<3,3> =====
    impl GraphApi<3, 3> for TestPipelineStd {
        #[inline]
        fn get_node_descriptors(&self) -> [NodeDescriptor; 3] {
            self.node_descs.clone()
        }
        #[inline]
        fn get_edge_descriptors(&self) -> [EdgeDescriptor; 3] {
            [
                self.ingress_edge.descriptor(),
                self.edges.0.descriptor(),
                self.edges.1.descriptor(),
            ]
        }

        #[inline]
        fn edge_occupancy_for<const E: usize>(&self) -> Result<EdgeOccupancy, GraphError> {
            let occ = match E {
                0 => {
                    // synthetic ingress: read atomic probe
                    self.ingress_edge.occupancy(&self.ingress_edge.policy())
                }
                1 => {
                    let e = &self.edges.0;
                    e.occupancy(&e.policy())
                }
                2 => {
                    let e = &self.edges.1;
                    e.occupancy(&e.policy())
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
                if ed.upstream.node == node_idx || ed.downstream.node == node_idx {
                    match (ed.id).0 {
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

        type OwnedBundle = TestPipelineStdOwnedBundle;

        #[cfg(feature = "std")]
        fn take_owned_bundle_by_index(
            &mut self,
            index: usize,
        ) -> Result<Self::OwnedBundle, GraphError> {
            match index {
                0 => {
                    let node = self.nodes.0.take().ok_or(GraphError::InvalidEdgeIndex)?; // or a NodeIndex error variant if you have it
                    let out0_policy = self.edges.0.policy();
                    let out0 = (self.endpoints.0).1.clone();
                    let ingress_updater = self
                        .ingress_updater
                        .take()
                        .expect("ingress updater already taken");
                    Ok(TestPipelineStdOwnedBundle::N0 {
                        node,
                        out0,
                        out0_policy,
                        ingress_updater,
                    })
                }
                1 => {
                    let node = self.nodes.1.take().ok_or(GraphError::InvalidEdgeIndex)?;
                    let in0_policy = self.edges.0.policy();
                    let out1_policy = self.edges.1.policy();
                    let in0 = (self.endpoints.0).0.clone();
                    let out1 = (self.endpoints.1).1.clone();
                    Ok(TestPipelineStdOwnedBundle::N1 {
                        node,
                        in0,
                        out1,
                        in0_policy,
                        out1_policy,
                    })
                }
                2 => {
                    let node = self.nodes.2.take().ok_or(GraphError::InvalidEdgeIndex)?;
                    let in1_policy = self.edges.1.policy();
                    let in1 = (self.endpoints.1).0.clone();
                    Ok(TestPipelineStdOwnedBundle::N2 {
                        node,
                        in1,
                        in1_policy,
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
                    out0,
                    out0_policy: _,
                    ingress_updater,
                } => {
                    assert!(self.nodes.0.is_none(), "node 0 already present");
                    self.nodes.0 = Some(node);
                    (self.endpoints.0).1 = out0;
                    // restore updater so future take() works again
                    self.ingress_updater = Some(ingress_updater);
                    Ok(())
                }
                TestPipelineStdOwnedBundle::N1 {
                    node, in0, out1, ..
                } => {
                    assert!(self.nodes.1.is_none(), "node 1 already present");
                    self.nodes.1 = Some(node);
                    (self.endpoints.0).0 = in0;
                    (self.endpoints.1).1 = out1;
                    Ok(())
                }
                TestPipelineStdOwnedBundle::N2 { node, in1, .. } => {
                    assert!(self.nodes.2.is_none(), "node 2 already present");
                    self.nodes.2 = Some(node);
                    (self.endpoints.1).0 = in1;
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
                    out0,
                    out0_policy,
                    ingress_updater,
                } => {
                    // Update ingress probe from the live source before stepping.
                    let occ = node.node().source_ref().ingress_occupancy(&INGRESS_POLICY);
                    ingress_updater.update(occ.items, occ.bytes);

                    let inputs: [&mut NoQueue<()>; 0] = [/* empty */];
                    let outputs: [&mut OutEpU32; 1] = [out0];

                    let in_policies: [EdgePolicy; 0] = [/* empty */];
                    let out_policies: [EdgePolicy; 1] = [*out0_policy];

                    let node_id: u32 = 0;
                    let in_edge_ids: [u32; 0] = [/* empty */];
                    let out_edge_ids: [u32; 1] = [1];

                    let mut ctx = crate::node::StepContext::new(
                        inputs,
                        outputs,
                        in_policies,
                        out_policies,
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
                    in0,
                    out1,
                    in0_policy,
                    out1_policy,
                } => {
                    let inputs: [&mut InEpU32; 1] = [in0];
                    let outputs: [&mut OutEpU32; 1] = [out1];
                    let in_policies: [EdgePolicy; 1] = [*in0_policy];
                    let out_policies: [EdgePolicy; 1] = [*out1_policy];

                    let node_id: u32 = 1;
                    let in_edge_ids: [u32; 1] = [1];
                    let out_edge_ids: [u32; 1] = [2];

                    let mut ctx = crate::node::StepContext::new(
                        inputs,
                        outputs,
                        in_policies,
                        out_policies,
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
                    in1,
                    in1_policy,
                } => {
                    let inputs: [&mut InEpU32; 1] = [in1];
                    let outputs: [&mut NoQueue<()>; 0] = [/* empty */];
                    let in_policies: [EdgePolicy; 1] = [*in1_policy];
                    let out_policies: [EdgePolicy; 0] = [/* empty */];

                    let node_id: u32 = 2;
                    let in_edge_ids: [u32; 1] = [2];
                    let out_edge_ids: [u32; 0] = [/* empty */];

                    let mut ctx = crate::node::StepContext::new(
                        inputs,
                        outputs,
                        in_policies,
                        out_policies,
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
    impl GraphNodeAccess<0> for TestPipelineStd {
        type Node = NodeLink<SrcNode, 0, 1, (), u32>;
        #[inline]
        fn node_ref(&self) -> &Self::Node {
            self.nodes.0.as_ref().expect("node 0 moved")
        }
        #[inline]
        fn node_mut(&mut self) -> &mut Self::Node {
            self.nodes.0.as_mut().expect("node 0 moved")
        }
    }
    impl GraphNodeAccess<1> for TestPipelineStd {
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
    impl GraphNodeAccess<2> for TestPipelineStd {
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
    impl GraphEdgeAccess<1> for TestPipelineStd {
        type Edge = ConcurrentEdgeLink<Q32, u32>;
        #[inline]
        fn edge_ref(&self) -> &Self::Edge {
            &self.edges.0
        }
        #[inline]
        fn edge_mut(&mut self) -> &mut Self::Edge {
            &mut self.edges.0
        }
    }
    impl GraphEdgeAccess<2> for TestPipelineStd {
        type Edge = ConcurrentEdgeLink<Q32, u32>;
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
    impl GraphNodeTypes<0, 0, 1> for TestPipelineStd {
        type InP = ();
        type OutP = u32;
        type InQ = NoQueue<()>;
        type OutQ = OutEpU32;
    }
    // node 1: IN=1, OUT=1  (map)
    impl GraphNodeTypes<1, 1, 1> for TestPipelineStd {
        type InP = u32;
        type OutP = u32;
        type InQ = InEpU32;
        type OutQ = OutEpU32;
    }
    // node 2: IN=1, OUT=0  (snk)
    impl GraphNodeTypes<2, 1, 0> for TestPipelineStd {
        type InP = u32;
        type OutP = ();
        type InQ = InEpU32;
        type OutQ = NoQueue<()>;
    }

    // ===== GraphNodeContextBuilder<I, IN, OUT> =====
    //
    // We implement ONLY the borrowed handoff method here (the runtime should call it).
    // If you still want `make_step_context(..)` as well, you can add it by borrowing
    // these same endpoint fields; the borrowed method is the key to avoid borrow overlap.

    // node 0: in=[], out=[edge id 1]
    impl GraphNodeContextBuilder<0, 0, 1> for TestPipelineStd {
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
            C,
            T,
        >
        where
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            let out0_policy = self.edges.0.policy();

            let inputs: [&'graph mut <Self as GraphNodeTypes<0, 0, 1>>::InQ; 0] = [];
            let outputs: [&'graph mut <Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1] =
                [&mut (self.endpoints.0).1];

            let in_policies: [EdgePolicy; 0] = [/* empty */];
            let out_policies: [EdgePolicy; 1] = [out0_policy];

            let node_id: u32 = 0;
            let in_edge_ids: [u32; 0] = [/* empty */];
            let out_edge_ids: [u32; 1] = [1];

            StepContext::new(
                inputs,
                outputs,
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
            let out0_policy = self.edges.0.policy();

            let inputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::InQ; 0] = [];
            let outputs: [&mut <Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1] =
                [&mut (self.endpoints.0).1];

            let in_policies: [EdgePolicy; 0] = [/* empty */];
            let out_policies: [EdgePolicy; 1] = [out0_policy];

            let node_id: u32 = 0;
            let in_edge_ids: [u32; 0] = [/* empty */];
            let out_edge_ids: [u32; 1] = [1];

            let mut ctx = StepContext::new(
                inputs,
                outputs,
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
    impl GraphNodeContextBuilder<1, 1, 1> for TestPipelineStd {
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
            C,
            T,
        >
        where
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            let in0_policy = self.edges.0.policy();
            let out1_policy = self.edges.1.policy();

            let inputs: [&'graph mut <Self as GraphNodeTypes<1, 1, 1>>::InQ; 1] =
                [&mut (self.endpoints.0).0];
            let outputs: [&'graph mut <Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1] =
                [&mut (self.endpoints.1).1];

            let in_policies: [EdgePolicy; 1] = [in0_policy];
            let out_policies: [EdgePolicy; 1] = [out1_policy];

            let node_id: u32 = 1;
            let in_edge_ids: [u32; 1] = [1];
            let out_edge_ids: [u32; 1] = [2];

            StepContext::new(
                inputs,
                outputs,
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
            let in0_policy = self.edges.0.policy();
            let out1_policy = self.edges.1.policy();

            let inputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::InQ; 1] =
                [&mut (self.endpoints.0).0];
            let outputs: [&mut <Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1] =
                [&mut (self.endpoints.1).1];

            let in_policies: [EdgePolicy; 1] = [in0_policy];
            let out_policies: [EdgePolicy; 1] = [out1_policy];

            let node_id: u32 = 1;
            let in_edge_ids: [u32; 1] = [1];
            let out_edge_ids: [u32; 1] = [2];

            let mut ctx = StepContext::new(
                inputs,
                outputs,
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
    impl GraphNodeContextBuilder<2, 1, 0> for TestPipelineStd {
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
            C,
            T,
        >
        where
            EdgePolicy: Copy,
            C: PlatformClock + Sized,
            T: Telemetry + Sized,
        {
            let in1_policy = self.edges.1.policy();

            let inputs: [&'graph mut <Self as GraphNodeTypes<2, 1, 0>>::InQ; 1] =
                [&mut (self.endpoints.1).0];
            let outputs: [&'graph mut <Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0] = [];

            let in_policies: [EdgePolicy; 1] = [in1_policy];
            let out_policies: [EdgePolicy; 0] = [/* empty */];

            let node_id: u32 = 2;
            let in_edge_ids: [u32; 1] = [2];
            let out_edge_ids: [u32; 0] = [/* empty */];

            StepContext::new(
                inputs,
                outputs,
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
            let in1_policy = self.edges.1.policy();

            let inputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::InQ; 1] =
                [&mut (self.endpoints.1).0];
            let outputs: [&mut <Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0] = [];

            let in_policies: [EdgePolicy; 1] = [in1_policy];
            let out_policies: [EdgePolicy; 0] = [/* empty */];

            let node_id: u32 = 2;
            let in_edge_ids: [u32; 1] = [2];
            let out_edge_ids: [u32; 0] = [/* empty */];

            let mut ctx = StepContext::new(
                inputs,
                outputs,
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
    impl GraphNodeOwnedEndpointHandoff<0, 0, 1> for TestPipelineStd {
        type NodeOwned = NodeLink<SrcNode, 0, 1, (), u32>;

        fn take_node_and_endpoints(
            &mut self,
        ) -> (
            Self::NodeOwned,
            [<Self as GraphNodeTypes<0, 0, 1>>::InQ; 0],
            [<Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1],
            [EdgePolicy; 0],
            [EdgePolicy; 1],
        )
        where
            <Self as GraphNodeTypes<0, 0, 1>>::InQ: Send + 'static,
            <Self as GraphNodeTypes<0, 0, 1>>::OutQ: Send + 'static,
        {
            let node = self.nodes.0.take().expect("node 0 already taken");
            let out0_policy = self.edges.0.policy();

            // clone endpoints (cheap; they share Arc)
            let out0 = (self.endpoints.0).1.clone();

            (node, [], [out0], [], [out0_policy])
        }

        fn put_node_and_endpoints(
            &mut self,
            node: Self::NodeOwned,
            _inputs: [<Self as GraphNodeTypes<0, 0, 1>>::InQ; 0],
            outputs: [<Self as GraphNodeTypes<0, 0, 1>>::OutQ; 1],
        ) {
            assert!(self.nodes.0.is_none(), "node 0 already present");
            self.nodes.0 = Some(node);
            // Optionally refresh our persistent endpoint with returned one.
            (self.endpoints.0).1 = outputs[0].clone();
        }
    }

    impl GraphNodeOwnedEndpointHandoff<1, 1, 1> for TestPipelineStd {
        type NodeOwned = NodeLink<MapNode, 1, 1, u32, u32>;

        fn take_node_and_endpoints(
            &mut self,
        ) -> (
            Self::NodeOwned,
            [<Self as GraphNodeTypes<1, 1, 1>>::InQ; 1],
            [<Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1],
            [EdgePolicy; 1],
            [EdgePolicy; 1],
        )
        where
            <Self as GraphNodeTypes<1, 1, 1>>::InQ: Send + 'static,
            <Self as GraphNodeTypes<1, 1, 1>>::OutQ: Send + 'static,
        {
            let node = self.nodes.1.take().expect("node 1 already taken");
            let in0_policy = self.edges.0.policy();
            let out1_policy = self.edges.1.policy();

            let in0 = (self.endpoints.0).0.clone();
            let out1 = (self.endpoints.1).1.clone();

            (node, [in0], [out1], [in0_policy], [out1_policy])
        }

        fn put_node_and_endpoints(
            &mut self,
            node: Self::NodeOwned,
            inputs: [<Self as GraphNodeTypes<1, 1, 1>>::InQ; 1],
            outputs: [<Self as GraphNodeTypes<1, 1, 1>>::OutQ; 1],
        ) {
            assert!(self.nodes.1.is_none(), "node 1 already present");
            self.nodes.1 = Some(node);
            (self.endpoints.0).0 = inputs[0].clone();
            (self.endpoints.1).1 = outputs[0].clone();
        }
    }

    impl GraphNodeOwnedEndpointHandoff<2, 1, 0> for TestPipelineStd {
        type NodeOwned = NodeLink<SnkNode, 1, 0, u32, ()>;

        fn take_node_and_endpoints(
            &mut self,
        ) -> (
            Self::NodeOwned,
            [<Self as GraphNodeTypes<2, 1, 0>>::InQ; 1],
            [<Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0],
            [EdgePolicy; 1],
            [EdgePolicy; 0],
        )
        where
            <Self as GraphNodeTypes<2, 1, 0>>::InQ: Send + 'static,
            <Self as GraphNodeTypes<2, 1, 0>>::OutQ: Send + 'static,
        {
            let node = self.nodes.2.take().expect("node 2 already taken");
            let in1_policy = self.edges.1.policy();
            let in1 = (self.endpoints.1).0.clone();
            (node, [in1], [], [in1_policy], [])
        }

        fn put_node_and_endpoints(
            &mut self,
            node: Self::NodeOwned,
            inputs: [<Self as GraphNodeTypes<2, 1, 0>>::InQ; 1],
            _outputs: [<Self as GraphNodeTypes<2, 1, 0>>::OutQ; 0],
        ) {
            assert!(self.nodes.2.is_none(), "node 2 already present");
            self.nodes.2 = Some(node);
            (self.endpoints.1).0 = inputs[0].clone();
        }
    }
}
