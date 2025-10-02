//! Graph contracts, descriptors, and a builder for wiring arbitrary topologies.
//!
//! # Layers
//!
//! - [`descriptor`]: storage-only descriptors (borrowed, owned, and const-buffer).
//! - [`validate`]: descriptor validators (no-alloc port checks; acyclicity with/without `alloc`).
//! - [`builder`] (alloc): ergonomic builder that produces a validated owned descriptor.
//!
//! Runtimes consume the **typed** `Graph` trait; tooling and codegen use descriptors.

use crate::node::Node;

use crate::{
    errors::{GraphError, NodeError},
    graph::validate::{GraphDescBuf, GraphValidator},
    message::{payload::Payload, Message},
    node::{link::NodeDescriptor, NodePolicy, StepContext, StepResult},
    policy::EdgePolicy,
    queue::{link::EdgeDescriptor, QueueOccupancy, SpscQueue},
};

pub mod bench;
pub mod validate;

// pub mod builder;

/// Provides indexed access to a graph node.
///
/// This trait is implemented by graph container types (e.g. a generated
/// graph struct) to allow compile-time access to a specific node by index.
///
/// # Type Parameters
/// - `I`: The compile-time index of the node within the graph.
pub trait GraphNodeAccess<const I: usize> {
    /// The concrete node type at index `I`.
    type Node;

    /// Immutable access to the node at index `I`.
    fn node_ref(&self) -> &Self::Node;

    /// Mutable access to the node at index `I`.
    fn node_mut(&mut self) -> &mut Self::Node;
}

// Provides indexed access to a graph edge.
///
/// This trait is implemented by graph container types to allow compile-time
/// access to a specific edge by index.
///
/// # Type Parameters
/// - `E`: The compile-time index of the edge within the graph.
pub trait GraphEdgeAccess<const E: usize> {
    /// The concrete edge type at index `E`.
    type Edge;

    /// Immutable access to the edge at index `E`.
    fn edge_ref(&self) -> &Self::Edge;

    /// Mutable access to the edge at index `E`.
    fn edge_mut(&mut self) -> &mut Self::Edge;
}

/// Defines per-node compile-time types and arity.
///
/// Implemented for each node in the graph by the graph-building macro.  
/// Associates payload types, queue types, and port counts with the node.
///
/// # Type Parameters
/// - `I`:   Compile-time index of the node within the graph.
/// - `IN`:  Number of input ports for the node.
/// - `OUT`: Number of output ports for the node.
pub trait GraphNodeTypes<const I: usize, const IN: usize, const OUT: usize> {
    /// Payload type for messages consumed by the node.
    type InP: Payload;

    /// Payload type for messages produced by the node.
    type OutP: Payload;

    /// Queue type used for input ports.
    type InQ: SpscQueue<Item = Message<Self::InP>>;

    /// Queue type used for output ports.
    type OutQ: SpscQueue<Item = Message<Self::OutP>>;
}

/// Builder for per-node execution contexts.
///
/// This trait is implemented by the graph-building macro. It allows the runtime
/// to create a [`StepContext`] for a given node using compile-time wiring of its
/// ports, queues, and policies.
///
/// # Type Parameters
/// - `I`:   Compile-time index of the node within the graph.
/// - `IN`:  Number of input ports for the node.
/// - `OUT`: Number of output ports for the node.
pub trait GraphNodeContextBuilder<const I: usize, const IN: usize, const OUT: usize>:
    GraphNodeTypes<I, IN, OUT>
{
    /// Construct a [`StepContext`] for node `I`.
    ///
    /// # Parameters
    /// - `clock`:     Reference to the runtime clock abstraction.
    /// - `telemetry`: Mutable reference to the telemetry collector.
    ///
    /// # Returns
    /// A fully wired [`StepContext`] for the node, ready for execution.
    fn make_step_context<'graph, 'telemetry, 'clock, C, T>(
        &'graph mut self,
        clock: &'clock C,
        telemetry: &'telemetry mut T,
    ) -> StepContext<
        'graph,
        'telemetry,
        'clock,
        IN,
        OUT,
        <Self as GraphNodeTypes<I, IN, OUT>>::InP,
        <Self as GraphNodeTypes<I, IN, OUT>>::OutP,
        <Self as GraphNodeTypes<I, IN, OUT>>::InQ,
        <Self as GraphNodeTypes<I, IN, OUT>>::OutQ,
        C,
        T,
    >
    where
        EdgePolicy: Copy;

    /// Borrowed handoff: in one `&mut self` borrow, lend both
    /// `&mut node(I)` and a fully wired `StepContext` to a closure.
    /// This avoids overlapping `&mut` borrows escaping the call.
    fn with_node_and_step_context<'telemetry, 'clock, C, T, R, E>(
        &mut self,
        clock: &'clock C,
        telemetry: &'telemetry mut T,
        f: impl FnOnce(
            &mut <Self as GraphNodeAccess<I>>::Node,
            &mut StepContext<
                '_,
                'telemetry,
                'clock,
                IN,
                OUT,
                <Self as GraphNodeTypes<I, IN, OUT>>::InP,
                <Self as GraphNodeTypes<I, IN, OUT>>::OutP,
                <Self as GraphNodeTypes<I, IN, OUT>>::InQ,
                <Self as GraphNodeTypes<I, IN, OUT>>::OutQ,
                C,
                T,
            >,
        ) -> Result<R, E>,
    ) -> Result<R, E>
    where
        Self: GraphNodeAccess<I>,
        EdgePolicy: Copy;
}

/// Std-only: move `node I` and owned endpoint queues to a worker thread.
#[cfg(feature = "std")]
pub trait GraphNodeOwnedEndpointHandoff<const I: usize, const IN: usize, const OUT: usize>:
    GraphNodeTypes<I, IN, OUT> + GraphNodeAccess<I>
{
    /// Node type to be moved to workers (usually the same as `GraphNodeAccess<I>::Node`).
    type NodeOwned: Node<
            IN,
            OUT,
            <Self as GraphNodeTypes<I, IN, OUT>>::InP,
            <Self as GraphNodeTypes<I, IN, OUT>>::OutP,
        > + Send
        + 'static;

    /// Take ownership of node `I` and produce owned endpoint queues + policies.
    #[allow(clippy::complexity)]
    fn take_node_and_endpoints(
        &mut self,
    ) -> (
        Self::NodeOwned,
        [<Self as GraphNodeTypes<I, IN, OUT>>::InQ; IN],
        [<Self as GraphNodeTypes<I, IN, OUT>>::OutQ; OUT],
        [EdgePolicy; IN],
        [EdgePolicy; OUT],
    )
    where
        <Self as GraphNodeTypes<I, IN, OUT>>::InQ: Send + 'static,
        <Self as GraphNodeTypes<I, IN, OUT>>::OutQ: Send + 'static;

    /// (Optional) Reattach ownership after the worker is done.
    fn put_node_and_endpoints(
        &mut self,
        node: Self::NodeOwned,
        inputs: [<Self as GraphNodeTypes<I, IN, OUT>>::InQ; IN],
        outputs: [<Self as GraphNodeTypes<I, IN, OUT>>::OutQ; OUT],
    );
}

/// Unified runtime-facing graph API.
///
/// Implemented by graph container types generated by the builder macro.
/// Provides access to node and edge descriptors, and convenience helpers for
/// accessing nodes, edges, and building step contexts.
///
/// # Type Parameters
/// - `NODE_COUNT`: Total number of nodes in the graph.
/// - `EDGE_COUNT`: Total number of edges in the graph.
pub trait GraphApi<const NODE_COUNT: usize, const EDGE_COUNT: usize> {
    /// Return static descriptors for all nodes in the graph.
    fn get_node_descriptors(&self) -> [NodeDescriptor; NODE_COUNT];

    /// Return static descriptors for all edges in the graph.
    fn get_edge_descriptors(&self) -> [EdgeDescriptor; EDGE_COUNT];

    /// Return the GrapgDescBuf for this graph.
    fn validate_graph(&self) -> Result<(), GraphError> {
        GraphDescBuf {
            nodes: self.get_node_descriptors(),
            edges: self.get_edge_descriptors(),
        }
        .validate()
    }

    /// Immutable access to node `I`.
    #[inline]
    fn get_node_ref<const I: usize>(&self) -> &<Self as GraphNodeAccess<I>>::Node
    where
        Self: GraphNodeAccess<I>,
    {
        <Self as GraphNodeAccess<I>>::node_ref(self)
    }

    /// Mutable access to node `I`.
    #[inline]
    fn get_node_mut<const I: usize>(&mut self) -> &mut <Self as GraphNodeAccess<I>>::Node
    where
        Self: GraphNodeAccess<I>,
    {
        <Self as GraphNodeAccess<I>>::node_mut(self)
    }

    /// Immutable access to edge `E`.
    #[inline]
    fn get_edge_ref<const E: usize>(&self) -> &<Self as GraphEdgeAccess<E>>::Edge
    where
        Self: GraphEdgeAccess<E>,
    {
        <Self as GraphEdgeAccess<E>>::edge_ref(self)
    }

    /// Mutable access to edge `E`.
    #[inline]
    fn get_edge_mut<const E: usize>(&mut self) -> &mut <Self as GraphEdgeAccess<E>>::Edge
    where
        Self: GraphEdgeAccess<E>,
    {
        <Self as GraphEdgeAccess<E>>::edge_mut(self)
    }

    /// Build a [`StepContext`] for node `I` using its compile-time wiring.
    ///
    /// Delegates to the node’s [`GraphNodeContextBuilder`] implementation.
    #[inline]
    fn make_step_context_for_node<
        'graph,
        'telemetry,
        'clock,
        const I: usize,
        const IN: usize,
        const OUT: usize,
        C,
        T,
    >(
        &'graph mut self,
        clock: &'clock C,
        telemetry: &'telemetry mut T,
    ) -> StepContext<
        'graph,
        'telemetry,
        'clock,
        IN,
        OUT,
        <Self as GraphNodeTypes<I, IN, OUT>>::InP,
        <Self as GraphNodeTypes<I, IN, OUT>>::OutP,
        <Self as GraphNodeTypes<I, IN, OUT>>::InQ,
        <Self as GraphNodeTypes<I, IN, OUT>>::OutQ,
        C,
        T,
    >
    where
        Self: GraphNodeContextBuilder<I, IN, OUT> + GraphNodeTypes<I, IN, OUT>,
        EdgePolicy: Copy,
    {
        <Self as GraphNodeContextBuilder<I, IN, OUT>>::make_step_context(self, clock, telemetry)
    }

    /// Borrowed handoff: in one `&mut self` borrow, lend both
    /// `&mut node(I)` and a fully wired `StepContext` to a closure.
    /// This avoids overlapping `&mut` borrows escaping the call.
    #[inline]
    fn with_node_and_step_context_for<
        'telemetry,
        'clock,
        const I: usize,
        const IN: usize,
        const OUT: usize,
        C,
        T,
        R,
        E,
    >(
        &mut self,
        clock: &'clock C,
        telemetry: &'telemetry mut T,
        f: impl FnOnce(
            &mut <Self as GraphNodeAccess<I>>::Node,
            &mut StepContext<
                '_,
                'telemetry,
                'clock,
                IN,
                OUT,
                <Self as GraphNodeTypes<I, IN, OUT>>::InP,
                <Self as GraphNodeTypes<I, IN, OUT>>::OutP,
                <Self as GraphNodeTypes<I, IN, OUT>>::InQ,
                <Self as GraphNodeTypes<I, IN, OUT>>::OutQ,
                C,
                T,
            >,
        ) -> Result<R, E>,
    ) -> Result<R, E>
    where
        Self: GraphNodeContextBuilder<I, IN, OUT> + GraphNodeTypes<I, IN, OUT> + GraphNodeAccess<I>,
        EdgePolicy: Copy,
    {
        <Self as GraphNodeContextBuilder<I, IN, OUT>>::with_node_and_step_context(
            self, clock, telemetry, f,
        )
    }

    /// Compute the current occupancy for edge `E` (read-only, no stepping).
    fn edge_occupancy_for<const E: usize>(&self) -> Result<QueueOccupancy, GraphError>;

    /// Fill the caller-provided buffer with current occupancies for all edges.
    /// No nodes are stepped; this is a read-only snapshot into a buffer the
    /// runtime owns persistently.
    fn write_all_edge_occupancies(
        &self,
        out: &mut [QueueOccupancy; EDGE_COUNT],
    ) -> Result<(), GraphError>;

    /// Update only the occupancies for edges incident to node `I`.
    /// Proc-macro expands this by writing to `out[e]` for inbound/outbound edges of `I`.
    fn refresh_occupancies_for_node<const I: usize, const IN: usize, const OUT: usize>(
        &self,
        out: &mut [QueueOccupancy; EDGE_COUNT],
    ) -> Result<(), GraphError>;

    /// Step the node at `index` once, using its compile-time-typed StepContext.
    ///
    /// The proc-macro should generate a `match index { ... }` that delegates to
    /// `with_node_and_step_context_for::<I, IN_I, OUT_I>(...)` and calls
    /// `node.step(ctx)`. No allocation, no trait objects.
    fn step_node_by_index<C, T>(
        &mut self,
        index: usize,
        clock: &C,
        telemetry: &mut T,
    ) -> Result<StepResult, NodeError>
    where
        EdgePolicy: Copy;

    /// Return the `NodePolicy` for node `I` without requiring &mut access.
    /// This simply calls `self.get_node_ref::<I>().policy()`. Useful for
    /// schedulers that want static hints alongside occupancy.
    fn node_policy_for<const I: usize, const IN: usize, const OUT: usize>(&self) -> NodePolicy
    where
        Self: GraphNodeAccess<I> + GraphNodeTypes<I, IN, OUT>,
        <Self as GraphNodeAccess<I>>::Node: Node<
            IN,
            OUT,
            <Self as GraphNodeTypes<I, IN, OUT>>::InP,
            <Self as GraphNodeTypes<I, IN, OUT>>::OutP,
        >,
    {
        <<Self as GraphNodeAccess<I>>::Node as Node<
            IN,
            OUT,
            <Self as GraphNodeTypes<I, IN, OUT>>::InP,
            <Self as GraphNodeTypes<I, IN, OUT>>::OutP,
        >>::policy(self.get_node_ref::<I>())
    }

    /// TODO: Add doc.
    #[cfg(feature = "std")]
    #[inline]
    #[allow(clippy::complexity)]
    fn take_node_and_endpoints_for<const I: usize, const IN: usize, const OUT: usize>(
        &mut self,
    ) -> (
        <Self as crate::graph::GraphNodeOwnedEndpointHandoff<I, IN, OUT>>::NodeOwned,
        [<Self as crate::graph::GraphNodeTypes<I, IN, OUT>>::InQ; IN],
        [<Self as crate::graph::GraphNodeTypes<I, IN, OUT>>::OutQ; OUT],
        [crate::policy::EdgePolicy; IN],
        [crate::policy::EdgePolicy; OUT],
    )
    where
        Self: crate::graph::GraphNodeOwnedEndpointHandoff<I, IN, OUT>
            + crate::graph::GraphNodeTypes<I, IN, OUT>,
        <Self as crate::graph::GraphNodeTypes<I, IN, OUT>>::InQ: Send + 'static,
        <Self as crate::graph::GraphNodeTypes<I, IN, OUT>>::OutQ: Send + 'static,
    {
        <Self as crate::graph::GraphNodeOwnedEndpointHandoff<I, IN, OUT>>::take_node_and_endpoints(
            self,
        )
    }

    /// TODO: Add doc.
    #[cfg(feature = "std")]
    #[inline]
    fn put_node_and_endpoints_for<const I: usize, const IN: usize, const OUT: usize>(
        &mut self,
        node: <Self as crate::graph::GraphNodeOwnedEndpointHandoff<I, IN, OUT>>::NodeOwned,
        inputs: [<Self as crate::graph::GraphNodeTypes<I, IN, OUT>>::InQ; IN],
        outputs: [<Self as crate::graph::GraphNodeTypes<I, IN, OUT>>::OutQ; OUT],
    ) where
        Self: crate::graph::GraphNodeOwnedEndpointHandoff<I, IN, OUT>
            + crate::graph::GraphNodeTypes<I, IN, OUT>,
    {
        <Self as crate::graph::GraphNodeOwnedEndpointHandoff<I, IN, OUT>>::put_node_and_endpoints(
            self, node, inputs, outputs,
        )
    }
}
