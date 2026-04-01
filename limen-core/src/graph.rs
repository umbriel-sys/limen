//! Graph contracts, descriptors, and a builder for wiring arbitrary topologies.
//!
//! # Layers
//!
//! - [`descriptor`]: storage-only descriptors (borrowed, owned, and const-buffer).
//! - [`validate`]: descriptor validators (no-alloc port checks; acyclicity with/without `alloc`).
//! - [`builder`] (alloc): ergonomic builder that produces a validated owned descriptor.
//!
//! Runtimes consume the **typed** `Graph` trait; tooling and codegen use descriptors.

pub mod validate;

#[cfg(any(test, feature = "bench"))]
pub mod bench;

use crate::node::Node;
use crate::prelude::{MemoryManager, PlatformClock, Telemetry};
use crate::{
    edge::{link::EdgeDescriptor, Edge, EdgeOccupancy},
    errors::{GraphError, NodeError},
    graph::validate::{GraphDescBuf, GraphValidator},
    message::payload::Payload,
    node::{link::NodeDescriptor, StepContext, StepResult},
    policy::{EdgePolicy, NodePolicy},
};

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
    type InQ: Edge;

    /// Queue type used for output ports.
    type OutQ: Edge;

    /// Memory manager type for input ports.
    type InM: MemoryManager<Self::InP>;

    /// Memory manager type for output ports.
    type OutM: MemoryManager<Self::OutP>;
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
    #[allow(clippy::type_complexity)]
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
        <Self as GraphNodeTypes<I, IN, OUT>>::InM,
        <Self as GraphNodeTypes<I, IN, OUT>>::OutM,
        C,
        T,
    >
    where
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized;

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
                <Self as GraphNodeTypes<I, IN, OUT>>::InM,
                <Self as GraphNodeTypes<I, IN, OUT>>::OutM,
                C,
                T,
            >,
        ) -> Result<R, E>,
    ) -> Result<R, E>
    where
        Self: GraphNodeAccess<I>,
        EdgePolicy: Copy,
        C: PlatformClock + Sized,
        T: Telemetry + Sized;
}

/// Unified runtime-facing graph API.
///
/// Exposes the minimal surface shared by all runtimes (P0, P1, P2, P2Concurrent).
/// The `NODE_COUNT` and `EDGE_COUNT` const generics define the compile-time
/// sizes for node and edge descriptor arrays and occupancy snapshots.
///
/// ## Occupancy buffer semantics
///
/// - `write_all_edge_occupancies` writes **current** occupancy for **every** edge
///   into the slot whose index equals that edge’s `EdgeIndex.0`.
///   It must not depend on pre-existing contents of `out`.
///
/// - `refresh_occupancies_for_node` is a **partial, in-place** refresh:
///   it MUST update only entries for edges incident to node `I` (either upstream
///   or downstream) and MUST NOT modify any other slots.
///
/// If sampling a particular edge fails, implementations should return
/// `Err(GraphError::OccupancySampleFailed(edge_idx))`.
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

    /// Returns the static `NodePolicy` for every node in the graph.
    ///
    /// The returned array length must equal `NODE_COUNT`, and index `i`
    /// corresponds to `NodeIndex::from(i)`.
    fn get_node_policies(&self) -> [NodePolicy; NODE_COUNT];

    /// Returns the static `EdgePolicy` for every edge in the graph.
    ///
    /// The returned array length must equal `EDGE_COUNT`, and index `e`
    /// corresponds to `EdgeIndex::from(e)`.
    fn get_edge_policies(&self) -> [EdgePolicy; EDGE_COUNT];

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
        GraphDescBuf::new(self.get_node_descriptors(), self.get_edge_descriptors()).validate()
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

    /// Write **current** occupancy for **all** edges into `out`.
    ///
    /// Contract:
    /// - Must populate every `out[k]` with the current occupancy for the edge
    ///   whose `EdgeIndex.0 == k`.
    /// - Must not depend on prior `out` contents.
    /// - On per-edge sampling failure, return
    ///   `Err(GraphError::OccupancySampleFailed(edge_idx))`.
    fn write_all_edge_occupancies(
        &self,
        out: &mut [EdgeOccupancy; EDGE_COUNT],
    ) -> Result<(), GraphError>;

    /// **Partial refresh**: update only entries for edges incident to node `I`.
    ///
    /// Contract:
    /// - MUST update `out[k]` iff edge `k` is upstream **or** downstream of node `I`.
    /// - MUST NOT modify any other `out[k]`.
    /// - If node `I` has no incident edges, this is a no-op that returns `Ok(())`.
    /// - On sampling failure for any incident edge, return
    ///   `Err(GraphError::OccupancySampleFailed(edge_idx))`.
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
        C: PlatformClock + Sized,
        T: Telemetry + Sized;

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
}

/// Opaque, runtime-owned buffer of edge occupancy snapshots.
///
/// # Semantics
/// - This buffer is **owned by the runtime** and passed by mutable reference to
///   graph APIs that *write into it* (see [`GraphApi::write_all_edge_occupancies`]
///   and [`GraphApi::refresh_occupancies_for_node`]).
/// - Each entry is a point-in-time [`EdgeOccupancy`] snapshot for the edge at the
///   same index as returned by `GraphApi::get_edge_descriptors()`.
/// - **Writers must not re-order** entries. Writers may update some or all slots,
///   but any slot not explicitly written must be left untouched.
///
/// # Contracts
/// - `GraphApi::write_all_edge_occupancies(&mut EdgeOccupancyBuf<E>)` **must write**
///   a fresh value for **every** slot `[0..E)`.
/// - `GraphApi::refresh_occupancies_for_node<I, IN, OUT>(&mut EdgeOccupancyBuf<E>)`
///   is a **partial refresh**: it **must only** update slots corresponding to edges
///   that are upstream or downstream of node `I` and **must not** modify any other
///   slots.
///
/// # Usage
/// - Runtimes typically allocate one `EdgeOccupancyBuf<E>` and reuse it across
///   sampling intervals. After a full write, they may call partial refreshes to keep
///   entries warm for the currently stepped node without touching unrelated edges.
/// - Consumers should treat the contents as **snapshots** only; values may change
///   immediately after sampling due to concurrent producers/consumers.
///
/// See also: [`EdgeOccupancy`], [`GraphApi`].
pub type EdgeOccupancyBuf<const E: usize> = [EdgeOccupancy; E];

// ---------------------------------------------------------------------------
// Concurrent scoped execution
// ---------------------------------------------------------------------------

/// Extension trait for graphs that support concurrent execution via scoped threads.
///
/// The runtime provides a [`WorkerScheduler`] that controls per-worker stepping.
/// The graph sets up scoped threads and disjoint node borrows; the scheduler
/// decides when each worker steps, waits, or exits.
///
/// This trait is `#[cfg(feature = "std")]` because it requires `std::thread::scope`.
/// [`GraphApi`] remains `no_std`-compatible.
///
/// # Edge and manager handles
///
/// Edges must implement [`ScopedEdge`](crate::edge::ScopedEdge) and managers
/// must implement [`ScopedManager`](crate::memory::ScopedManager) to produce
/// per-worker handles. Arc-based types (e.g. `ConcurrentEdge`,
/// `ConcurrentMemoryManager`) return clones; future lock-free types will
/// return split producer/consumer handles via
/// [`EdgeHandleKind`](crate::edge::EdgeHandleKind).
///
/// # Scheduling model
///
/// The codegen-generated implementation:
/// 1. Obtains per-worker edge and manager handles via `ScopedEdge::scoped_handle`
///    and `ScopedManager::scoped_handle` (before node borrows)
/// 2. Takes disjoint `&mut` borrows of each node (tuple field access)
/// 3. Spawns one scoped thread per node
/// 4. Each worker loop:
///    - Queries edge occupancy from concrete types (no dyn dispatch)
///    - Builds a [`WorkerState`] snapshot (readiness, backpressure, tick, last result)
///    - Calls `scheduler.decide(&state)` (static dispatch via `S`)
///    - Acts on the [`WorkerDecision`]: step, wait, or exit
///
/// The runtime controls execution policy by choosing which [`WorkerScheduler`]
/// to pass. Different runtimes can implement different strategies (backoff,
/// EDF, throughput, criticality-aware) without changing the graph.
///
/// [`WorkerScheduler`]: crate::scheduling::WorkerScheduler
/// [`WorkerState`]: crate::scheduling::WorkerState
/// [`WorkerDecision`]: crate::scheduling::WorkerDecision
#[cfg(feature = "std")]
pub trait ScopedGraphApi<const NODE_COUNT: usize, const EDGE_COUNT: usize>:
    GraphApi<NODE_COUNT, EDGE_COUNT>
{
    /// Run the graph concurrently with scheduler-controlled workers.
    ///
    /// Spawns one scoped thread per node. Each worker calls
    /// `scheduler.decide()` before every step. All threads join when
    /// all workers return (scope exit).
    ///
    /// Clock and telemetry are moved into the scope and distributed to workers
    /// via `Clone`. After this method returns, all threads have joined.
    fn run_scoped<C, T, S>(&mut self, clock: C, telemetry: T, scheduler: S)
    where
        C: PlatformClock + Clone + Send + Sync + 'static,
        T: Telemetry + Clone + Send + 'static,
        S: crate::scheduling::WorkerScheduler + 'static;
}
