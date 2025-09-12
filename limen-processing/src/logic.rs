//! Logical processing nodes (threshold, debounce, classify).
use limen_core::message::{Message, Payload};
use limen_core::node::{Node, NodeCapabilities, NodePolicy, StepContext, StepResult};
use limen_core::queue::{SpscQueue, enqueue_with_admission};
use limen_core::errors::NodeError;
use limen_core::memory::PlacementAcceptance;
use crate::payload::{Tensor1D, Label};

/// Threshold a single element to produce a label (0/1).
#[derive(Debug, Clone, Copy)]
pub struct ThresholdNode<const N: usize> {
    /// Index to check.
    pub index: usize,
    /// Threshold value.
    pub threshold: f32,
}

impl<const N: usize> ThresholdNode<N> {
    /// Construct.
    pub const fn new(index: usize, threshold: f32) -> Self { Self { index, threshold } }
}

impl<const N: usize> Node<1, 1, Tensor1D<f32, N>, Label> for ThresholdNode<N> {
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
        ctx: &mut StepContext<1, 1, Tensor1D<f32, N>, Label, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<Tensor1D<f32, N>>>,
        OutQ: SpscQueue<Item = Message<Label>>,
    {
        let m = match ctx.inputs[0].try_pop() {
            Ok(m) => m,
            Err(_) => return Ok(StepResult::NoInput),
        };
        let label = if self.index < N && m.payload.data[self.index] >= self.threshold { 1 } else { 0 };
        let out_msg = Message::new(m.header, Label(label));
        let res = enqueue_with_admission(&mut ctx.outputs[0], &ctx.out_policies[0], out_msg);
        match res {
            limen_core::queue::EnqueueResult::Enqueued => Ok(StepResult::MadeProgress),
            _ => Ok(StepResult::Backpressured),
        }
    }
}

/// Debounce binary labels: require K consecutive 1's to emit 1; 0 resets.
#[derive(Debug, Clone, Copy)]
pub struct DebounceNode {
    k: usize,
    acc: usize,
}

impl DebounceNode {
    /// Construct with `k` consecutive threshold.
    pub const fn new(k: usize) -> Self { Self { k, acc: 0 } }
}

impl Node<1, 1, Label, Label> for DebounceNode {
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
        ctx: &mut StepContext<1, 1, Label, Label, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<Label>>,
        OutQ: SpscQueue<Item = Message<Label>>,
    {
        let m = match ctx.inputs[0].try_pop() {
            Ok(m) => m,
            Err(_) => return Ok(StepResult::NoInput),
        };
        if m.payload.0 == 1 {
            self.acc += 1;
        } else {
            self.acc = 0;
        }
        let out = if self.acc >= self.k { 1 } else { 0 };
        let msg = Message::new(m.header, Label(out));
        let res = enqueue_with_admission(&mut ctx.outputs[0], &ctx.out_policies[0], msg);
        match res {
            limen_core::queue::EnqueueResult::Enqueued => Ok(StepResult::MadeProgress),
            _ => Ok(StepResult::Backpressured),
        }
    }
}

/// Argmax classify: index of max element as label.
#[derive(Debug, Clone, Copy)]
pub struct ArgmaxClassifyNode<const N: usize>;

impl<const N: usize> Node<1, 1, Tensor1D<f32, N>, Label> for ArgmaxClassifyNode<N> {
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
    fn initialize<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), limen_core::errors::NodeError> { Ok(()) }

    fn step<InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<1, 1, Tensor1D<f32, N>, Label, InQ, OutQ, C, T>,
    ) -> Result<StepResult, limen_core::errors::NodeError>
    where
        InQ: SpscQueue<Item = Message<Tensor1D<f32, N>>>,
        OutQ: SpscQueue<Item = Message<Label>>,
    {
        let m = match ctx.inputs[0].try_pop() {
            Ok(m) => m,
            Err(_) => return Ok(StepResult::NoInput),
        };
        let mut best = 0usize;
        let mut bestv = m.payload.data[0];
        for i in 1..N {
            if m.payload.data[i] > bestv { bestv = m.payload.data[i]; best = i; }
        }
        let msg = Message::new(m.header, Label(best as u8));
        let res = enqueue_with_admission(&mut ctx.outputs[0], &ctx.out_policies[0], msg);
        match res {
            limen_core::queue::EnqueueResult::Enqueued => Ok(StepResult::MadeProgress),
            _ => Ok(StepResult::Backpressured),
        }
    }
}
