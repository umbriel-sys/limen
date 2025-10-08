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

use crate::prelude::Telemetry;
use crate::{
    edge::{link::EdgeDescriptor, Edge, EdgeOccupancy},
    errors::{GraphError, NodeError},
    graph::validate::{GraphDescBuf, GraphValidator},
    message::{payload::Payload, Message},
    node::{link::NodeDescriptor, NodePolicy, StepContext, StepResult},
    policy::EdgePolicy,
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
    type InQ: Edge<Item = Message<Self::InP>>;

    /// Queue type used for output ports.
    type OutQ: Edge<Item = Message<Self::OutP>>;
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
        EdgePolicy: Copy,
        T: Telemetry;

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
        EdgePolicy: Copy,
        T: Telemetry;
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
/// Exposes the minimal surface shared by all runtimes (P0, P1, P2, P2Concurrent).
/// The `NODE_COUNT` and `EDGE_COUNT` const generics define the compile-time
/// sizes for node and edge descriptor arrays and occupancy snapshots.
pub trait GraphApi<const NODE_COUNT: usize, const EDGE_COUNT: usize> {
    // ----- Descriptors & validation -----

    /// Returns the static descriptors for all nodes in the graph.
    ///
    /// The returned array length must equal `NODE_COUNT` and be consistent with
    /// the edge descriptors exposed by [`get_edge_descriptors`](Self::get_edge_descriptors).
    fn get_node_descriptors(&self) -> [NodeDescriptor; NODE_COUNT];

    /// Returns the static descriptors for all edges in the graph.
    ///
    /// The returned array length must equal `EDGE_COUNT` and reference only valid
    /// node indices described by [`get_node_descriptors`](Self::get_node_descriptors).
    fn get_edge_descriptors(&self) -> [EdgeDescriptor; EDGE_COUNT];

    /// Validates the graph topology and policies derived from node and edge descriptors.
    ///
    /// This checks index bounds, arities, endpoint compatibility, and any static
    /// policy invariants enforced by `GraphDescBuf::validate`.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphError`] if the descriptors are inconsistent or violate
    /// graph-level constraints.
    #[inline]
    fn validate_graph(&self) -> Result<(), GraphError> {
        GraphDescBuf {
            nodes: self.get_node_descriptors(),
            edges: self.get_edge_descriptors(),
        }
        .validate()
    }

    // ----- Occupancy snapshot helpers -----

    /// Returns a one-shot occupancy snapshot for edge `E`.
    ///
    /// Useful for lightweight telemetry or scheduling decisions that only need
    /// the latest observed queue depth for a specific edge.
    ///
    /// # Type Parameters
    ///
    /// * `E` — The compile-time edge index in `0..EDGE_COUNT`.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphError`] if `E` is out of range or the occupancy cannot
    /// be sampled.
    fn edge_occupancy_for<const E: usize>(&self) -> Result<EdgeOccupancy, GraphError>;

    /// Writes occupancy snapshots for **all** edges into `out`.
    ///
    /// This is the most efficient way to bulk-refresh edge depths for schedulers
    /// or exporters that need a full graph view.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphError`] if any edge occupancy cannot be sampled.
    fn write_all_edge_occupancies(
        &self,
        out: &mut [EdgeOccupancy; EDGE_COUNT],
    ) -> Result<(), GraphError>;

    /// Refreshes occupancies only for edges incident to node `I`.
    ///
    /// This selectively updates `out` entries corresponding to the `IN` inputs
    /// and `OUT` outputs of node `I`, leaving all other entries unchanged.
    ///
    /// # Type Parameters
    ///
    /// * `I` — The compile-time node index in `0..NODE_COUNT`.
    /// * `IN` — The node’s input arity.
    /// * `OUT` — The node’s output arity.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphError`] if `I` is out of range or the related edge
    /// occupancies cannot be sampled.
    fn refresh_occupancies_for_node<const I: usize, const IN: usize, const OUT: usize>(
        &self,
        out: &mut [EdgeOccupancy; EDGE_COUNT],
    ) -> Result<(), GraphError>;

    // ----- Generic step-by-index (for P0/P1) -----

    /// Drives a single scheduling step for the node at `index`.
    ///
    /// Runtimes P0/P1 use this to advance nodes generically over an abstract
    /// clock and telemetry sink without requiring node-specific types.
    ///
    /// # Parameters
    ///
    /// * `index` — The dynamic node index to step.
    /// * `clock` — A clock-like source used by the node during execution.
    /// * `telemetry` — A sink for emitting per-step metrics or traces.
    ///
    /// # Returns
    ///
    /// A [`StepResult`] indicating whether work was performed or the node is idle.
    ///
    /// # Errors
    ///
    /// Returns a [`NodeError`] if the node fails to execute its step.
    fn step_node_by_index<C, T>(
        &mut self,
        index: usize,
        clock: &C,
        telemetry: &mut T,
    ) -> Result<StepResult, NodeError>
    where
        EdgePolicy: Copy,
        T: Telemetry;

    // ----- Optional: static node policy read -----

    /// Returns the static [`NodePolicy`] for node `I` (compile-time index).
    ///
    /// This queries the node type directly, without requiring an instance step,
    /// and is useful for planning or verifying scheduling constraints.
    ///
    /// # Type Parameters
    ///
    /// * `I` — The compile-time node index in `0..NODE_COUNT`.
    /// * `IN` — The node’s input arity.
    /// * `OUT` — The node’s output arity.
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
        >>::policy(<Self as GraphNodeAccess<I>>::node_ref(self))
    }

    // ===== std-only: by-index owned handoff for worker threads =====

    /// Opaque, owned bundle containing a node and its owned endpoints for
    /// transfer to worker threads.
    #[cfg(feature = "std")]
    type OwnedBundle: Send + 'static;

    /// Moves the node at `index` and its owned endpoints out of the graph.
    ///
    /// The returned bundle can be executed off-thread and later reattached via
    /// [`put_owned_bundle_by_index`](Self::put_owned_bundle_by_index).
    ///
    /// # Errors
    ///
    /// Returns a [`GraphError`] if `index` is invalid, the node is already
    /// detached, or ownership cannot be transferred.
    #[cfg(feature = "std")]
    fn take_owned_bundle_by_index(&mut self, index: usize)
        -> Result<Self::OwnedBundle, GraphError>;

    /// Reattaches a previously taken owned bundle back into the graph.
    ///
    /// After reattachment, the node is once again managed by the graph and can
    /// be stepped by index or handed off again.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphError`] if the bundle does not match the target graph
    /// slot or the slot is already occupied.
    #[cfg(feature = "std")]
    fn put_owned_bundle_by_index(&mut self, bundle: Self::OwnedBundle) -> Result<(), GraphError>;

    /// Executes a single step on an owned bundle outside the graph.
    ///
    /// This is the worker-thread counterpart to
    /// [`step_node_by_index`](Self::step_node_by_index), operating directly on
    /// the detached bundle.
    ///
    /// # Parameters
    ///
    /// * `bundle` — The detached node bundle to execute.
    /// * `clock` — A clock-like source used by the node during execution.
    /// * `telemetry` — A sink for emitting per-step metrics or traces.
    ///
    /// # Returns
    ///
    /// A [`StepResult`] indicating the outcome of the step.
    ///
    /// # Errors
    ///
    /// Returns a [`NodeError`] if the node’s step fails.
    #[cfg(feature = "std")]
    fn step_owned_bundle<C, T>(
        bundle: &mut Self::OwnedBundle,
        clock: &C,
        telemetry: &mut T,
    ) -> Result<StepResult, NodeError>
    where
        EdgePolicy: Copy,
        T: Telemetry;
}
