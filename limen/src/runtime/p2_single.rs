//! Single-thread P2 runtime: EDF or Throughput scheduling.
use std::time::Instant;

use limen_core::errors::NodeError;
use limen_core::platform::PlatformClock;
use limen_core::scheduling::{DequeuePolicy, NodeSummary};
use limen_core::telemetry::Telemetry;
use limen_core::node::StepResult;

/// Graph trait for P2 single-thread runtime.
pub trait GraphP2<C: PlatformClock, T: Telemetry> {
    /// Number of nodes in the graph.
    const NODES: usize;

    /// Initialize resources.
    fn initialize(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError>;

    /// Optional warm-up.
    fn start(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> { Ok(()) }

    /// Return scheduler summaries for all nodes.
    fn summaries(&self) -> [NodeSummary; Self::NODES];

    /// Step a node by index once.
    fn step_node(&mut self, index: usize, clock: &C, telemetry: &mut T) -> Result<StepResult, NodeError>;

    /// Optional stop.
    fn stop(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> { Ok(()) }
}

/// Runtime parameterized by a dequeue policy.
pub struct RuntimeP2<G, C, T, P>
where
    G: GraphP2<C, T>,
    C: PlatformClock,
    T: Telemetry,
    P: DequeuePolicy,
{
    graph: G,
    clock: C,
    telemetry: T,
    policy: P,
}

impl<G, C, T, P> RuntimeP2<G, C, T, P>
where
    G: GraphP2<C, T>,
    C: PlatformClock,
    T: Telemetry,
    P: DequeuePolicy,
{
    /// Build a new runtime.
    pub fn new(graph: G, clock: C, telemetry: T, policy: P) -> Self {
        Self { graph, clock, telemetry, policy }
    }

    /// Initialize and start the graph.
    pub fn initialize_and_start(&mut self) -> Result<(), NodeError> {
        self.graph.initialize(&self.clock, &mut self.telemetry)?;
        self.graph.start(&self.clock, &mut self.telemetry)?;
        Ok(())
    }

    /// Run a single scheduling cycle.
    pub fn run_cycle(&mut self) -> Result<Option<usize>, NodeError> {
        let summaries = self.graph.summaries();
        if let Some(next) = self.policy.select_next(&summaries) {
            let idx = next.0;
            let _res = self.graph.step_node(idx, &self.clock, &mut self.telemetry)?;
            Ok(Some(idx))
        } else {
            Ok(None)
        }
    }

    /// Run for `n` cycles.
    pub fn run_n_cycles(&mut self, n: usize) -> Result<(), NodeError> {
        for _ in 0..n {
            let _ = self.run_cycle()?;
        }
        Ok(())
    }

    /// Stop the graph.
    pub fn stop(mut self) -> Result<(), NodeError> {
        self.graph.stop(&self.clock, &mut self.telemetry)
    }

    /// Borrow the graph.
    pub fn graph(&self) -> &G { &self.graph }
    /// Borrow the telemetry sink.
    pub fn telemetry(&self) -> &T { &self.telemetry }
    /// Borrow the clock.
    pub fn clock(&self) -> &C { &self.clock }
}
