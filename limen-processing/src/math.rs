//! Numeric processing nodes (normalize, moving average).
use limen_core::message::{Message, Payload};
use limen_core::node::{Node, NodeCapabilities, NodePolicy, StepContext, StepResult};
use limen_core::queue::{SpscQueue, enqueue_with_admission};
use limen_core::errors::NodeError;
use limen_core::memory::PlacementAcceptance;
use crate::payload::Tensor1D;

/// Normalize each element by `y = (x * scale) + offset`.
#[derive(Debug, Clone, Copy)]
pub struct NormalizeNode<const N: usize> {
    /// Scale factor.
    pub scale: f32,
    /// Additive offset.
    pub offset: f32,
}

impl<const N: usize> NormalizeNode<N> {
    /// Construct a normalize node.
    pub const fn new(scale: f32, offset: f32) -> Self { Self { scale, offset } }
}

impl<const N: usize> Node<1, 1, Tensor1D<f32, N>, Tensor1D<f32, N>> for NormalizeNode<N> {
    fn describe_capabilities(&self) -> NodeCapabilities {
        NodeCapabilities { device_streams: false, degrade_tiers: false }
    }
    fn input_acceptance(&self) -> [PlacementAcceptance; 1] { [PlacementAcceptance::host_all()] }
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
        ctx: &mut StepContext<1, 1, Tensor1D<f32, N>, Tensor1D<f32, N>, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<Tensor1D<f32, N>>>,
        OutQ: SpscQueue<Item = Message<Tensor1D<f32, N>>>,
    {
        let m = match ctx.inputs[0].try_pop() {
            Ok(m) => m,
            Err(_) => return Ok(StepResult::NoInput),
        };
        let mut out = m.payload;
        for v in &mut out.data {
            *v = *v * self.scale + self.offset;
        }
        let header = m.header;
        let out_msg = Message::new(header, out);
        let res = enqueue_with_admission(&mut ctx.outputs[0], &ctx.out_policies[0], out_msg);
        match res {
            limen_core::queue::EnqueueResult::Enqueued => Ok(StepResult::MadeProgress),
            _ => Ok(StepResult::Backpressured),
        }
    }
}

/// Moving average over a fixed window W, element-wise.
#[derive(Debug)]
pub struct MovingAverageNode<const N: usize, const W: usize> {
    buf: [Tensor1D<f32, N>; W],
    count: usize,
    idx: usize,
}

impl<const N: usize, const W: usize> MovingAverageNode<N, W> {
    /// Create a new moving average node with zeroed buffer.
    pub fn new() -> Self {
        Self { buf: [Tensor1D { data: [0.0; N] }; W], count: 0, idx: 0 }
    }
}

impl<const N: usize, const W: usize> Node<1, 1, Tensor1D<f32, N>, Tensor1D<f32, N>> for MovingAverageNode<N, W> {
    fn describe_capabilities(&self) -> NodeCapabilities {
        NodeCapabilities { device_streams: false, degrade_tiers: false }
    }
    fn input_acceptance(&self) -> [PlacementAcceptance; 1] { [PlacementAcceptance::host_all()] }
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
        ctx: &mut StepContext<1, 1, Tensor1D<f32, N>, Tensor1D<f32, N>, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<Tensor1D<f32, N>>>,
        OutQ: SpscQueue<Item = Message<Tensor1D<f32, N>>>,
    {
        let m = match ctx.inputs[0].try_pop() {
            Ok(m) => m,
            Err(_) => return Ok(StepResult::NoInput),
        };
        // Update buffer
        self.buf[self.idx] = m.payload;
        self.idx = (self.idx + 1) % W;
        if self.count < W { self.count += 1; }

        // Compute average
        let mut out = Tensor1D { data: [0.0; N] };
        for i in 0..self.count {
            let x = &self.buf[i];
            for (j, v) in x.data.iter().enumerate() {
                out.data[j] += *v;
            }
        }
        let denom = self.count as f32;
        for v in &mut out.data { *v /= denom.max(1.0); }

        let out_msg = Message::new(m.header, out);
        let res = enqueue_with_admission(&mut ctx.outputs[0], &ctx.out_policies[0], out_msg);
        match res {
            limen_core::queue::EnqueueResult::Enqueued => Ok(StepResult::MadeProgress),
            _ => Ok(StepResult::Backpressured),
        }
    }
}
