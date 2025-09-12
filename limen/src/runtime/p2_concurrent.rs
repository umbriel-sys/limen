//! Concurrent P2 runtime (optional): requires a graph with interior mutability.
//!
//! **Why this exists:** the core Node contract is cooperative and takes `&mut self`.
//! To step nodes concurrently, a graph must encapsulate its nodes and queues with
//! interior mutability (e.g., `Mutex`/`RwLock`) and expose a **thread-safe** stepping
//! API. This trait models that requirement without imposing it universally.

use std::sync::mpsc::{sync_channel, SyncSender, Receiver};
use std::thread;

use limen_core::errors::NodeError;
use limen_core::platform::PlatformClock;
use limen_core::scheduling::{DequeuePolicy, NodeSummary};
use limen_core::telemetry::Telemetry;
use limen_core::node::StepResult;

/// Thread-safe graph trait for concurrent stepping.
pub trait GraphP2Concurrent<C: PlatformClock, T: Telemetry>: Send + 'static {
    /// Number of nodes.
    const NODES: usize;

    /// Initialize and warm-up on the calling thread.
    fn initialize_and_start(&self, clock: &C, telemetry: &T) -> Result<(), NodeError>;

    /// Return scheduler summaries for all nodes (read-only).
    fn summaries(&self) -> [NodeSummary; Self::NODES];

    /// Step a node by index **without** exclusive &mut access (uses interior mutability).
    fn step_node_threadsafe(&self, index: usize, clock: &C, telemetry: &T) -> Result<StepResult, NodeError>;

    /// Stop the graph.
    fn stop(&self, clock: &C, telemetry: &T) -> Result<(), NodeError>;
}

/// Concurrent runtime using a fixed worker pool.
pub struct RuntimeP2Concurrent<G, C, T, P, const WORKERS: usize>
where
    G: GraphP2Concurrent<C, T>,
    C: PlatformClock + Send + Sync + 'static,
    T: Telemetry + Send + Sync + 'static,
    P: DequeuePolicy + Send + 'static,
{
    graph: G,
    clock: C,
    telemetry: T,
    policy: P,
    tx: SyncSender<usize>,
    _handles: Vec<std::thread::JoinHandle<()>>,
}

impl<G, C, T, P, const WORKERS: usize> RuntimeP2Concurrent<G, C, T, P, WORKERS>
where
    G: GraphP2Concurrent<C, T>,
    C: PlatformClock + Send + Sync + 'static,
    T: Telemetry + Send + Sync + 'static,
    P: DequeuePolicy + Send + 'static,
{
    /// Create a new concurrent runtime with a fixed worker pool.
    pub fn new(graph: G, clock: C, telemetry: T, policy: P) -> Self {
        let (tx, rx) = sync_channel::<usize>(WORKERS * 2);
        let mut handles = Vec::new();

        // Spawn worker threads
        for _ in 0..WORKERS {
            let rx: Receiver<usize> = rx.clone();
            let g = &graph;
            let c = &clock;
            let t = &telemetry;
            handles.push(thread::spawn(move || {
                while let Ok(idx) = rx.recv() {
                    let _ = g.step_node_threadsafe(idx, c, t);
                }
            }));
        }

        Self { graph, clock, telemetry, policy, tx, _handles: handles }
    }

    /// Initialize and start the graph.
    pub fn initialize_and_start(&self) -> Result<(), NodeError> {
        self.graph.initialize_and_start(&self.clock, &self.telemetry)
    }

    /// Run one scheduler tick: select a node and dispatch to a worker.
    pub fn run_cycle(&mut self) -> Result<Option<usize>, NodeError> {
        let summaries = self.graph.summaries();
        if let Some(next) = self.policy.select_next(&summaries) {
            let idx = next.0;
            // Best-effort send; if the channel is full, we skip (backpressure).
            let _ = self.tx.try_send(idx);
            Ok(Some(idx))
        } else {
            Ok(None)
        }
    }

    /// Stop the graph.
    pub fn stop(&self) -> Result<(), NodeError> {
        self.graph.stop(&self.clock, &self.telemetry)
    }
}
