#![cfg_attr(not(feature = "std"), no_std)]

pub mod bench;

use crate::{
    graph::{GraphApi, GraphNodeContextBuilder, GraphNodeTypes},
    node::StepContext,
    policy::EdgePolicy,
};

/// A single, uniform runtime trait that all Limen runtimes (P0, P1, P2, P2Concurrent)
/// can implement. The API is allocation- and threading-agnostic so it works in:
/// - `no_std` + `no_alloc`
/// - `no_std` + `alloc`
/// - `std` single-threaded
/// - `std` multi-threaded
///
/// The trait drives a graph that implements the unified graph API traits:
/// `GraphApi`, `GraphNodeAccess`, `GraphEdgeAccess`, `GraphNodeTypes`,
/// and `GraphNodeContextBuilder`.
///
/// Design notes:
/// - No heap, channels, or threading are exposed in the interface.
/// - `run()` has a default implementation that repeatedly calls `step()` until the
///   runtime reports it should stop (cooperative stop).
/// - Runtimes decide their own scheduling policy inside `step()`.
/// - `init()` and `reset()` allow lifecycle control without assuming allocation.
/// - `request_stop()` / `is_stopping()` provide a uniform cooperative stop signal.
/// - `with_step_context_for::<I, IN, OUT>()` is an ergonomic helper that builds the
///   compile-time-typed `StepContext` and hands it to a closure; this lets each
///   runtime call node code without the trait needing to name a node trait here.
///
/// The trait is generic over the concrete `Graph` type so a generated graph struct
/// can be used directly without trait objects or dynamic dispatch.
///
/// # Type Parameters
/// - `Graph`:
///   A concrete graph type that implements `GraphApi<NODE_COUNT, EDGE_COUNT>`
///   and per-node `GraphNodeTypes` / `GraphNodeContextBuilder`.
pub trait LimenRuntime<Graph, const NODE_COUNT: usize, const EDGE_COUNT: usize>
where
    Graph: GraphApi<NODE_COUNT, EDGE_COUNT>,
{
    /// Clock abstraction carried by the runtime. Choose `()` if you have none.
    type Clock;

    /// Telemetry collector carried by the runtime. Choose `()` if you have none.
    type Telemetry;

    /// Error type produced by the runtime. Use `core::convert::Infallible` if none.
    type Error;

    /// Initialize internal runtime state against the provided graph instance.
    ///
    /// Called exactly once before stepping or running.
    fn init(
        &mut self,
        graph: &mut Graph,
        clock: &Self::Clock,
        telemetry: &mut Self::Telemetry,
    ) -> Result<(), Self::Error>;

    /// Reset internal runtime state and (optionally) graph-local scheduling state.
    ///
    /// Implementations should leave the graph’s data buffers and node state intact
    /// unless their scheduling strategy requires clearing per-run bookkeeping.
    fn reset(
        &mut self,
        graph: &mut Graph,
        clock: &Self::Clock,
        telemetry: &mut Self::Telemetry,
    ) -> Result<(), Self::Error>;

    /// Request a cooperative stop. Implementations should set internal flags such
    /// that future calls to `step()` will eventually return `Ok(false)` or `run()`
    /// will return.
    fn request_stop(&mut self);

    /// Return `true` iff a stop has been requested and not yet fully observed.
    fn is_stopping(&self) -> bool;

    /// Execute one scheduler tick (one “unit of work”) and return whether more work
    /// remains (`Ok(true)`), or the runtime should stop (`Ok(false)`).
    ///
    /// Implementations typically:
    /// - choose a node according to their scheduling policy,
    /// - build a typed `StepContext` for that node using
    ///   `with_step_context_for::<I, IN, OUT>(...)`,
    /// - invoke the node’s step function,
    /// - update internal bookkeeping,
    /// - honor `is_stopping()`.
    fn step(
        &mut self,
        graph: &mut Graph,
        clock: &Self::Clock,
        telemetry: &mut Self::Telemetry,
    ) -> Result<bool, Self::Error>;

    /// Drive the runtime until it cooperatively indicates completion or a stop is
    /// requested. The default loop is allocation-free and portable across targets.
    fn run(
        &mut self,
        graph: &mut Graph,
        clock: &Self::Clock,
        telemetry: &mut Self::Telemetry,
    ) -> Result<(), Self::Error> {
        loop {
            if self.is_stopping() {
                return Ok(());
            }
            match self.step(graph, clock, telemetry)? {
                true => continue,
                false => return Ok(()),
            }
        }
    }

    /// Convenience: build a `StepContext` for node `I` and hand it to `f`.
    ///
    /// This keeps `LimenRuntime` independent of any node execution trait. Each
    /// concrete runtime can call the node’s step method inside `f`, or perform
    /// pre/post hooks around it, without this trait needing to name that node API.
    ///
    /// # Type Parameters
    /// - `I`:   Node index.
    /// - `IN`:  Node input arity.
    /// - `OUT`: Node output arity.
    ///
    /// The closure can return any value `R` and error via `E`. This allows runtimes
    /// to integrate node outcomes into their own error or control flow.
    fn with_step_context_for<const I: usize, const IN: usize, const OUT: usize, R, E>(
        graph: &mut Graph,
        clock: &Self::Clock,
        telemetry: &mut Self::Telemetry,
        mut f: impl FnMut(
            StepContext<
                IN,
                OUT,
                <Graph as GraphNodeTypes<I, IN, OUT>>::InP,
                <Graph as GraphNodeTypes<I, IN, OUT>>::OutP,
                <Graph as GraphNodeTypes<I, IN, OUT>>::InQ,
                <Graph as GraphNodeTypes<I, IN, OUT>>::OutQ,
                Self::Clock,
                Self::Telemetry,
            >,
        ) -> Result<R, E>,
    ) -> Result<R, E>
    where
        Graph: GraphNodeTypes<I, IN, OUT> + GraphNodeContextBuilder<I, IN, OUT>,
        EdgePolicy: Copy,
    {
        let ctx = <Graph as GraphNodeContextBuilder<I, IN, OUT>>::make_step_context(
            graph, clock, telemetry,
        );
        f(ctx)
    }
}
