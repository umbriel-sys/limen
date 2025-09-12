//! P0 runtime: deterministic single-thread cooperative loop.
//!
//! This runtime targets `no_std + no_alloc` devices. It assumes a statically-wired
//! graph and uses round-robin fairness across nodes.

use limen_core::node::StepResult;
use limen_core::errors::NodeError;
use limen_core::platform::PlatformClock;
use limen_core::telemetry::Telemetry;

/// Graph trait that a statically-wired pipeline must implement to run on P0.
pub trait GraphP0<C: PlatformClock, T: Telemetry> {
    /// Number of nodes in the graph (used for round-robin fairness).
    const NODES: usize;

    /// Initialize all nodes and acquire resources.
    fn initialize(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError>;

    /// Optional warm-up before stepping begins.
    fn start(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> { Ok(()) }

    /// Step a specific node by its index. The implementation should build the
    /// appropriate `StepContext` and call the node's `step` method.
    fn step_node(&mut self, index: usize, clock: &C, telemetry: &mut T) -> Result<StepResult, NodeError>;

    /// Optional stop hook to flush and release resources.
    fn stop(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> { Ok(()) }
}

/// A simple round-robin scheduler state.
#[derive(Debug, Default)]
pub struct RoundRobin {
    next: usize,
}

impl RoundRobin {
    fn next_index(&mut self, n: usize) -> usize {
        let i = self.next;
        self.next = (self.next + 1) % n;
        i
    }
}

/// P0 runtime runner.
pub struct RuntimeP0<G, C, T>
where
    G: GraphP0<C, T>,
    C: PlatformClock,
    T: Telemetry,
{
    graph: G,
    clock: C,
    telemetry: T,
    rr: RoundRobin,
}

impl<G, C, T> RuntimeP0<G, C, T>
where
    G: GraphP0<C, T>,
    C: PlatformClock,
    T: Telemetry,
{
    /// Create a new runtime from a graph, platform clock, and telemetry sink.
    pub fn new(graph: G, clock: C, telemetry: T) -> Self {
        Self { graph, clock, telemetry, rr: RoundRobin::default() }
    }

    /// Initialize and start the graph.
    pub fn initialize_and_start(&mut self) -> Result<(), NodeError> {
        self.graph.initialize(&self.clock, &mut self.telemetry)?;
        self.graph.start(&self.clock, &mut self.telemetry)?;
        Ok(())
    }

    /// Run a single scheduling cycle (one step of one node).
    pub fn run_cycle(&mut self) -> Result<StepResult, NodeError> {
        let idx = self.rr.next_index(G::NODES);
        self.graph.step_node(idx, &self.clock, &mut self.telemetry)
    }

    /// Run for a fixed number of cycles.
    pub fn run_n_cycles(&mut self, n: usize) -> Result<(), NodeError> {
        for _ in 0..n {
            let _ = self.run_cycle()?;
        }
        Ok(())
    }

    /// Run until the provided predicate returns `true`. The predicate is
    /// checked each cycle.
    pub fn run_until<F: Fn() -> bool>(&mut self, stop: F) -> Result<(), NodeError> {
        while !stop() {
            let _ = self.run_cycle()?;
        }
        Ok(())
    }

    /// Stop the graph and release resources.
    pub fn stop(mut self) -> Result<(), NodeError> {
        self.graph.stop(&self.clock, &mut self.telemetry)
    }

    /// Borrow the inner graph.
    pub fn graph(&self) -> &G { &self.graph }
    /// Borrow the inner telemetry sink.
    pub fn telemetry(&self) -> &T { &self.telemetry }
    /// Borrow the platform clock.
    pub fn clock(&self) -> &C { &self.clock }
}
