//! P1 runtime: cooperative micro-batching with bounded heap.
//!
//! This runtime targets `no_std + alloc` devices (RTOS/embedded Linux/WASM).
use limen_core::node::{NodePolicy, StepResult};
use limen_core::errors::NodeError;
use limen_core::platform::PlatformClock;
use limen_core::telemetry::Telemetry;
use limen_core::types::Ticks;

/// Graph trait for P1 with policy introspection to drive micro-batching.
pub trait GraphP1<C: PlatformClock, T: Telemetry> {
    /// Number of nodes.
    const NODES: usize;

    /// Initialize resources.
    fn initialize(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError>;

    /// Optional warm-up.
    fn start(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> { Ok(()) }

    /// Return the policy bundle of a node.
    fn node_policy(&self, index: usize) -> NodePolicy;

    /// Step the node once.
    fn step_node(&mut self, index: usize, clock: &C, telemetry: &mut T) -> Result<StepResult, NodeError>;

    /// Optional stop.
    fn stop(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> { Ok(()) }
}

/// Runtime with micro-batching (N and/or Δt cap).
pub struct RuntimeP1<G, C, T>
where
    G: GraphP1<C, T>,
    C: PlatformClock,
    T: Telemetry,
{
    graph: G,
    clock: C,
    telemetry: T,
    next: usize,
}

impl<G, C, T> RuntimeP1<G, C, T>
where
    G: GraphP1<C, T>,
    C: PlatformClock,
    T: Telemetry,
{
    /// Create a new runtime.
    pub fn new(graph: G, clock: C, telemetry: T) -> Self {
        Self { graph, clock, telemetry, next: 0 }
    }

    /// Initialize and start.
    pub fn initialize_and_start(&mut self) -> Result<(), NodeError> {
        self.graph.initialize(&self.clock, &mut self.telemetry)?;
        self.graph.start(&self.clock, &mut self.telemetry)?;
        Ok(())
    }

    #[inline]
    fn next_index(&mut self) -> usize {
        let i = self.next;
        self.next = (self.next + 1) % G::NODES;
        i
    }

    /// Run one micro-batch for the next node.
    pub fn run_cycle(&mut self) -> Result<StepResult, NodeError> {
        let idx = self.next_index();
        let pol = self.graph.node_policy(idx);
        let fixed_n = pol.batching.fixed_n.unwrap_or(1);
        let delta_t = pol.batching.max_delta_t;

        let start = self.clock.now_ticks();
        let mut steps_done = 0usize;
        let mut last_res = StepResult::NoInput;

        loop {
            let res = self.graph.step_node(idx, &self.clock, &mut self.telemetry)?;
            last_res = res;

            match res {
                StepResult::MadeProgress => {
                    steps_done += 1;
                }
                StepResult::NoInput | StepResult::Backpressured | StepResult::WaitingOnExternal | StepResult::Terminal => {
                    break;
                }
                StepResult::YieldUntil(ticks) => {
                    // Cooperative yield: respect if within delta_t cap.
                    if let Some(cap) = delta_t {
                        if Self::elapsed(&self.clock, start) >= cap {
                            break;
                        }
                    }
                    // Otherwise continue loop; scheduler is cooperative.
                }
            }

            // Respect fixed-N and Δt caps.
            if steps_done >= fixed_n {
                break;
            }
            if let Some(cap) = delta_t {
                if Self::elapsed(&self.clock, start) >= cap {
                    break;
                }
            }
        }

        Ok(last_res)
    }

    #[inline]
    fn elapsed(clock: &C, start: Ticks) -> Ticks {
        let now = clock.now_ticks();
        Ticks(now.0.saturating_sub(start.0))
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

    /// Borrow the inner graph.
    pub fn graph(&self) -> &G { &self.graph }
    /// Borrow the inner telemetry sink.
    pub fn telemetry(&self) -> &T { &self.telemetry }
    /// Borrow the platform clock.
    pub fn clock(&self) -> &C { &self.clock }
}
