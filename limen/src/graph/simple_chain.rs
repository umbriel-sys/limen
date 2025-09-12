//! Simple 5-stage linear pipeline helper (Source→Pre→Model→Post→Sink) for P2.
use limen_core::node::{Node, NodePolicy, StepContext, StepResult};
use limen_core::message::{Message, Payload};
use limen_core::policy::EdgePolicy;
use limen_core::queue::SpscQueue;
use limen_core::telemetry::Telemetry;
use limen_core::platform::PlatformClock;
use limen_core::scheduling::{NodeSummary, Readiness};
use limen_core::types::{NodeIndex, PortIndex, DeadlineNs};

use crate::runtime::p2_single::GraphP2;

/// A zero-size payload to satisfy generics for 0-port nodes.
#[derive(Debug, Clone, Copy)]
pub struct EmptyPayload;
impl Payload for EmptyPayload {
    fn buffer_descriptor(&self) -> limen_core::memory::BufferDescriptor {
        limen_core::memory::BufferDescriptor { bytes: 0, class: limen_core::memory::MemoryClass::Host }
    }
}

/// A simple linear chain for P2 as in limen-light, with 1-in/1-out mid-stages.
pub struct SimpleChain5<
    S, P, M, O, K, // nodes
    P1, P2p, P3, P4, // payloads
    Q1, Q2, Q3, Q4,  // queues
> where
    P1: Payload, P2p: Payload, P3: Payload, P4: Payload,
    Q1: SpscQueue<Item = Message<P1>>, Q2: SpscQueue<Item = Message<P2p>>,
    Q3: SpscQueue<Item = Message<P3>>, Q4: SpscQueue<Item = Message<P4>>,
    S: Node<0, 1, EmptyPayload, P1>,
    P: Node<1, 1, P1, P2p>,
    M: Node<1, 1, P2p, P3>,
    O: Node<1, 1, P3, P4>,
    K: Node<1, 0, P4, EmptyPayload>,
{
    pub source: S,
    pub pre: P,
    pub model: M,
    pub post: O,
    pub sink: K,

    pub q1: Q1, pub pol_q1: EdgePolicy,
    pub q2: Q2, pub pol_q2: EdgePolicy,
    pub q3: Q3, pub pol_q3: EdgePolicy,
    pub q4: Q4, pub pol_q4: EdgePolicy,
}

impl<S, P, M, O, K, P1, P2p, P3, P4, Q1, Q2, Q3, Q4>
    SimpleChain5<S, P, M, O, K, P1, P2p, P3, P4, Q1, Q2, Q3, Q4>
where
    P1: Payload, P2p: Payload, P3: Payload, P4: Payload,
    Q1: SpscQueue<Item = Message<P1>>, Q2: SpscQueue<Item = Message<P2p>>,
    Q3: SpscQueue<Item = Message<P3>>, Q4: SpscQueue<Item = Message<P4>>,
    S: Node<0, 1, EmptyPayload, P1>,
    P: Node<1, 1, P1, P2p>,
    M: Node<1, 1, P2p, P3>,
    O: Node<1, 1, P3, P4>,
    K: Node<1, 0, P4, EmptyPayload>,
{
    /// Compute scheduler summaries (simple variant without deep introspection).
    fn summaries_internal(&self) -> [NodeSummary; 5] {
        // In a real implementation, you'd inspect queue watermarks and deadlines.
        // This helper provides a basic "Ready" summary for all nodes.
        [
            NodeSummary { index: NodeIndex(0), earliest_deadline: None, readiness: Readiness::Ready, backpressure: limen_core::policy::WatermarkState::BelowSoft },
            NodeSummary { index: NodeIndex(1), earliest_deadline: None, readiness: Readiness::Ready, backpressure: limen_core::policy::WatermarkState::BelowSoft },
            NodeSummary { index: NodeIndex(2), earliest_deadline: None, readiness: Readiness::Ready, backpressure: limen_core::policy::WatermarkState::BelowSoft },
            NodeSummary { index: NodeIndex(3), earliest_deadline: None, readiness: Readiness::Ready, backpressure: limen_core::policy::WatermarkState::BelowSoft },
            NodeSummary { index: NodeIndex(4), earliest_deadline: None, readiness: Readiness::Ready, backpressure: limen_core::policy::WatermarkState::BelowSoft },
        ]
    }

    fn step_node_internal<C: PlatformClock, T: Telemetry>(
        &mut self, index: usize, clock: &C, telemetry: &mut T
    ) -> Result<StepResult, limen_core::errors::NodeError> {
        match index {
            0 => {
                let mut ctx = StepContext::<'_, 0, 1, EmptyPayload, P1, Q1, Q1, C, T>::new(
                    [], [&mut self.q1], [], [self.pol_q1], clock, telemetry
                );
                self.source.step(&mut ctx)
            }
            1 => {
                let mut ctx = StepContext::<'_, 1, 1, P1, P2p, Q1, Q2, C, T>::new(
                    [&mut self.q1], [&mut self.q2], [self.pol_q1], [self.pol_q2], clock, telemetry
                );
                self.pre.step(&mut ctx)
            }
            2 => {
                let mut ctx = StepContext::<'_, 1, 1, P2p, P3, Q2, Q3, C, T>::new(
                    [&mut self.q2], [&mut self.q3], [self.pol_q2], [self.pol_q3], clock, telemetry
                );
                self.model.step(&mut ctx)
            }
            3 => {
                let mut ctx = StepContext::<'_, 1, 1, P3, P4, Q3, Q4, C, T>::new(
                    [&mut self.q3], [&mut self.q4], [self.pol_q3], [self.pol_q4], clock, telemetry
                );
                self.post.step(&mut ctx)
            }
            4 => {
                let mut ctx = StepContext::<'_, 1, 0, P4, EmptyPayload, Q4, Q4, C, T>::new(
                    [&mut self.q4], [], [self.pol_q4], [], clock, telemetry
                );
                self.sink.step(&mut ctx)
            }
            _ => Ok(StepResult::NoInput),
        }
    }
}

/// Wrapper that implements GraphP2 for SimpleChain5.
pub struct SimpleChain5P2<G>(pub G);

impl<S, P, M, O, K, P1, P2p, P3, P4, Q1, Q2, Q3, Q4, C, T>
    crate::runtime::p2_single::GraphP2<C, T> for SimpleChain5P2<
        SimpleChain5<S, P, M, O, K, P1, P2p, P3, P4, Q1, Q2, Q3, Q4>
    >
where
    P1: Payload, P2p: Payload, P3: Payload, P4: Payload,
    Q1: SpscQueue<Item = Message<P1>>, Q2: SpscQueue<Item = Message<P2p>>,
    Q3: SpscQueue<Item = Message<P3>>, Q4: SpscQueue<Item = Message<P4>>,
    S: Node<0, 1, EmptyPayload, P1>,
    P: Node<1, 1, P1, P2p>,
    M: Node<1, 1, P2p, P3>,
    O: Node<1, 1, P3, P4>,
    K: Node<1, 0, P4, EmptyPayload>,
    C: PlatformClock,
    T: Telemetry,
{
    const NODES: usize = 5;

    fn initialize(&mut self, clock: &C, telemetry: &mut T) -> Result<(), limen_core::errors::NodeError> {
        self.0.source.initialize(clock, telemetry)?;
        self.0.pre.initialize(clock, telemetry)?;
        self.0.model.initialize(clock, telemetry)?;
        self.0.post.initialize(clock, telemetry)?;
        self.0.sink.initialize(clock, telemetry)?;
        Ok(())
    }

    fn start(&mut self, clock: &C, telemetry: &mut T) -> Result<(), limen_core::errors::NodeError> {
        self.0.source.start(clock, telemetry)?;
        self.0.pre.start(clock, telemetry)?;
        self.0.model.start(clock, telemetry)?;
        self.0.post.start(clock, telemetry)?;
        self.0.sink.start(clock, telemetry)?;
        Ok(())
    }

    fn summaries(&self) -> [NodeSummary; Self::NODES] {
        self.0.summaries_internal()
    }

    fn step_node(&mut self, index: usize, clock: &C, telemetry: &mut T) -> Result<StepResult, limen_core::errors::NodeError> {
        self.0.step_node_internal(index, clock, telemetry)
    }

    fn stop(&mut self, clock: &C, telemetry: &mut T) -> Result<(), limen_core::errors::NodeError> {
        self.0.source.stop(clock, telemetry)?;
        self.0.pre.stop(clock, telemetry)?;
        self.0.model.stop(clock, telemetry)?;
        self.0.post.stop(clock, telemetry)?;
        self.0.sink.stop(clock, telemetry)?;
        Ok(())
    }
}
