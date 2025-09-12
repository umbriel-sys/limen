//! Simulated 1D source generating synthetic samples.
use limen_core::message::{Message, Payload, MessageHeader, MessageFlags};
use limen_core::node::{Node, NodeCapabilities, NodePolicy, StepContext, StepResult};
use limen_core::queue::{SpscQueue, enqueue_with_admission};
use limen_core::errors::NodeError;
use limen_core::memory::{PlacementAcceptance, MemoryClass};
use limen_core::types::{TraceId, SequenceNumber, Ticks};
use limen_processing::payload::Tensor1D;

/// A simple synthetic source: fills the tensor with an increasing counter (as f32).
#[derive(Debug, Clone, Copy)]
pub struct SimulatedSource1D<const N: usize> {
    counter: u64,
}

impl<const N: usize> SimulatedSource1D<N> {
    /// Create with counter starting at zero.
    pub const fn new() -> Self { Self { counter: 0 } }
}

impl<const N: usize> Node<0, 1, Tensor1D<f32, N>, Tensor1D<f32, N>> for SimulatedSource1D<N> {
    fn describe_capabilities(&self) -> NodeCapabilities {
        NodeCapabilities { device_streams: false, degrade_tiers: false }
    }
    fn input_acceptance(&self) -> [PlacementAcceptance; 0] { [] }
    fn output_acceptance(&self) -> [PlacementAcceptance; 1] { [PlacementAcceptance::host_all()] }
    fn policy(&self) -> NodePolicy {
        NodePolicy {
            batching: limen_core::policy::BatchingPolicy::none(),
            budget: limen_core::policy::BudgetPolicy { tick_budget: None },
            deadline: limen_core::policy::DeadlinePolicy { require_absolute_deadline: false, slack_tolerance_ns: None },
        }
    }
    fn initialize<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> { Ok(()) }

    fn step<InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<0, 1, Tensor1D<f32, N>, Tensor1D<f32, N>, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        OutQ: SpscQueue<Item = Message<Tensor1D<f32, N>>>,
    {
        let mut data = [0.0f32; N];
        let base = self.counter as f32;
        for i in 0..N { data[i] = base + (i as f32); }
        self.counter = self.counter.wrapping_add(1);

        let payload = Tensor1D { data };
        let header = MessageHeader::new(
            TraceId(self.counter),
            SequenceNumber(self.counter),
            Ticks(0),
            None,
            limen_core::types::QoSClass::BestEffort,
            0,
            MessageFlags::empty(),
            MemoryClass::Host,
        );
        let msg = Message::new(header, payload);

        let res = enqueue_with_admission(&mut ctx.outputs[0], &ctx.out_policies[0], msg);
        match res {
            limen_core::queue::EnqueueResult::Enqueued => Ok(StepResult::MadeProgress),
            _ => Ok(StepResult::Backpressured),
        }
    }
}
