//! Simple 5-stage linear pipeline helper (Source→Pre→Model→Post→Sink).
//!
//! This showcases how to wire nodes and queues without dynamic dispatch while
//! satisfying the P0/P1 runtime `Graph` traits. It assumes 1-in/1-out ports
//! for the middle stages, and 0-in/1-out for Source, 1-in/0-out for Sink.

use limen_core::node::{Node, NodePolicy, StepContext, StepResult};
use limen_core::message::{Message, Payload};
use limen_core::policy::EdgePolicy;
use limen_core::queue::SpscQueue;
use limen_core::telemetry::Telemetry;
use limen_core::platform::PlatformClock;
use limen_core::errors::NodeError;

/// A zero-size payload to satisfy `Node` generics when a stage has 0 inputs or outputs.
#[derive(Debug, Clone, Copy)]
pub struct EmptyPayload;
impl Payload for EmptyPayload {
    fn buffer_descriptor(&self) -> limen_core::memory::BufferDescriptor {
        limen_core::memory::BufferDescriptor { bytes: 0, class: limen_core::memory::MemoryClass::Host }
    }
}

/// A simple linear chain:
/// Source(0->1) -> Pre(1->1) -> Model(1->1) -> Post(1->1) -> Sink(1->0)
pub struct SimpleChain5<
    // Node types
    S, P, M, O, K,
    // Payload types between nodes
    P1, P2, P3, P4,
    // Queue types between nodes
    Q1, Q2, Q3, Q4,
> where
    P1: Payload, P2: Payload, P3: Payload, P4: Payload,
    Q1: SpscQueue<Item = Message<P1>>, Q2: SpscQueue<Item = Message<P2>>,
    Q3: SpscQueue<Item = Message<P3>>, Q4: SpscQueue<Item = Message<P4>>,
    S: Node<0, 1, EmptyPayload, P1>,
    P: Node<1, 1, P1, P2>,
    M: Node<1, 1, P2, P3>,
    O: Node<1, 1, P3, P4>,
    K: Node<1, 0, P4, EmptyPayload>,
{
    /// Nodes (Source, Pre, Model, Post, Sink).
    pub source: S,
    pub pre: P,
    pub model: M,
    pub post: O,
    pub sink: K,
    /// Queues between stages.
    pub q1: Q1,
    pub q2: Q2,
    pub q3: Q3,
    pub q4: Q4,
    /// Edge policies per queue.
    pub pol_q1: EdgePolicy,
    pub pol_q2: EdgePolicy,
    pub pol_q3: EdgePolicy,
    pub pol_q4: EdgePolicy,
}

impl<
        S, P, M, O, K,
        P1, P2, P3, P4,
        Q1, Q2, Q3, Q4,
    > SimpleChain5<S, P, M, O, K, P1, P2, P3, P4, Q1, Q2, Q3, Q4>
where
    P1: Payload, P2: Payload, P3: Payload, P4: Payload,
    Q1: SpscQueue<Item = Message<P1>>, Q2: SpscQueue<Item = Message<P2>>,
    Q3: SpscQueue<Item = Message<P3>>, Q4: SpscQueue<Item = Message<P4>>,
    S: Node<0, 1, EmptyPayload, P1>,
    P: Node<1, 1, P1, P2>,
    M: Node<1, 1, P2, P3>,
    O: Node<1, 1, P3, P4>,
    K: Node<1, 0, P4, EmptyPayload>,
{
    /// Initialize all nodes.
    pub fn initialize<C: PlatformClock, T: Telemetry>(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError> {
        self.source.initialize(clock, telemetry)?;
        self.pre.initialize(clock, telemetry)?;
        self.model.initialize(clock, telemetry)?;
        self.post.initialize(clock, telemetry)?;
        self.sink.initialize(clock, telemetry)?;
        Ok(())
    }

    /// Optional warm-up.
    pub fn start<C: PlatformClock, T: Telemetry>(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError> {
        self.source.start(clock, telemetry)?;
        self.pre.start(clock, telemetry)?;
        self.model.start(clock, telemetry)?;
        self.post.start(clock, telemetry)?;
        self.sink.start(clock, telemetry)?;
        Ok(())
    }

    /// Stop and release resources.
    pub fn stop<C: PlatformClock, T: Telemetry>(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError> {
        self.source.stop(clock, telemetry)?;
        self.pre.stop(clock, telemetry)?;
        self.model.stop(clock, telemetry)?;
        self.post.stop(clock, telemetry)?;
        self.sink.stop(clock, telemetry)?;
        Ok(())
    }

    /// Return a node policy by index 0..4.
    pub fn node_policy(&self, index: usize) -> NodePolicy {
        match index {
            0 => self.source.policy(),
            1 => self.pre.policy(),
            2 => self.model.policy(),
            3 => self.post.policy(),
            4 => self.sink.policy(),
            _ => self.sink.policy(),
        }
    }

    /// Step node by index using round-robin order.
    pub fn step_node<C: PlatformClock, T: Telemetry>(
        &mut self,
        index: usize,
        clock: &C,
        telemetry: &mut T,
    ) -> Result<StepResult, NodeError> {
        match index {
            0 => {
                // Source: outputs -> q1
                let mut ctx = StepContext::<'_, 0, 1, EmptyPayload, P1, Q1, Q1, C, T>::new(
                    [],
                    [&mut self.q1],
                    [],
                    [self.pol_q1],
                    clock,
                    telemetry,
                );
                self.source.step(&mut ctx)
            }
            1 => {
                // Pre: inputs <- q1, outputs -> q2
                let mut ctx = StepContext::<'_, 1, 1, P1, P2, Q1, Q2, C, T>::new(
                    [&mut self.q1],
                    [&mut self.q2],
                    [self.pol_q1],
                    [self.pol_q2],
                    clock,
                    telemetry,
                );
                self.pre.step(&mut ctx)
            }
            2 => {
                let mut ctx = StepContext::<'_, 1, 1, P2, P3, Q2, Q3, C, T>::new(
                    [&mut self.q2],
                    [&mut self.q3],
                    [self.pol_q2],
                    [self.pol_q3],
                    clock,
                    telemetry,
                );
                self.model.step(&mut ctx)
            }
            3 => {
                let mut ctx = StepContext::<'_, 1, 1, P3, P4, Q3, Q4, C, T>::new(
                    [&mut self.q3],
                    [&mut self.q4],
                    [self.pol_q3],
                    [self.pol_q4],
                    clock,
                    telemetry,
                );
                self.post.step(&mut ctx)
            }
            4 => {
                let mut ctx = StepContext::<'_, 1, 0, P4, EmptyPayload, Q4, Q4, C, T>::new(
                    [&mut self.q4],
                    [],
                    [self.pol_q4],
                    [],
                    clock,
                    telemetry,
                );
                self.sink.step(&mut ctx)
            }
            _ => Ok(StepResult::NoInput),
        }
    }
}

/// A P0 wrapper that implements the runtime trait for `SimpleChain5`.
pub struct SimpleChain5P0<G>(pub G);

impl<
        S, P, M, O, K, P1t, P2t, P3t, P4t, Q1t, Q2t, Q3t, Q4t, C, T
    > crate::runtime::p0::GraphP0<C, T> for SimpleChain5P0<
        SimpleChain5<S, P, M, O, K, P1t, P2t, P3t, P4t, Q1t, Q2t, Q3t, Q4t>
    >
where
    P1t: Payload, P2t: Payload, P3t: Payload, P4t: Payload,
    Q1t: SpscQueue<Item = Message<P1t>>, Q2t: SpscQueue<Item = Message<P2t>>,
    Q3t: SpscQueue<Item = Message<P3t>>, Q4t: SpscQueue<Item = Message<P4t>>,
    S: Node<0, 1, EmptyPayload, P1t>,
    P: Node<1, 1, P1t, P2t>,
    M: Node<1, 1, P2t, P3t>,
    O: Node<1, 1, P3t, P4t>,
    K: Node<1, 0, P4t, EmptyPayload>,
    C: PlatformClock,
    T: Telemetry,
{
    const NODES: usize = 5;

    fn initialize(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError> {
        self.0.initialize(clock, telemetry)
    }
    fn start(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError> {
        self.0.start(clock, telemetry)
    }
    fn step_node(&mut self, index: usize, clock: &C, telemetry: &mut T) -> Result<StepResult, NodeError> {
        self.0.step_node(index, clock, telemetry)
    }
    fn stop(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError> {
        self.0.stop(clock, telemetry)
    }
}

/// A P1 wrapper that implements the runtime trait for `SimpleChain5`.
pub struct SimpleChain5P1<G>(pub G);

#[cfg(feature = "alloc")]
impl<
        S, P, M, O, K, P1t, P2t, P3t, P4t, Q1t, Q2t, Q3t, Q4t, C, T
    > crate::runtime::p1::GraphP1<C, T> for SimpleChain5P1<
        SimpleChain5<S, P, M, O, K, P1t, P2t, P3t, P4t, Q1t, Q2t, Q3t, Q4t>
    >
where
    P1t: Payload, P2t: Payload, P3t: Payload, P4t: Payload,
    Q1t: SpscQueue<Item = Message<P1t>>, Q2t: SpscQueue<Item = Message<P2t>>,
    Q3t: SpscQueue<Item = Message<P3t>>, Q4t: SpscQueue<Item = Message<P4t>>,
    S: Node<0, 1, EmptyPayload, P1t>,
    P: Node<1, 1, P1t, P2t>,
    M: Node<1, 1, P2t, P3t>,
    O: Node<1, 1, P3t, P4t>,
    K: Node<1, 0, P4t, EmptyPayload>,
    C: PlatformClock,
    T: Telemetry,
{
    const NODES: usize = 5;

    fn initialize(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError> {
        self.0.initialize(clock, telemetry)
    }
    fn start(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError> {
        self.0.start(clock, telemetry)
    }
    fn node_policy(&self, index: usize) -> NodePolicy {
        self.0.node_policy(index)
    }
    fn step_node(&mut self, index: usize, clock: &C, telemetry: &mut T) -> Result<StepResult, NodeError> {
        self.0.step_node(index, clock, telemetry)
    }
    fn stop(&mut self, clock: &C, telemetry: &mut T) -> Result<(), NodeError> {
        self.0.stop(clock, telemetry)
    }
}
