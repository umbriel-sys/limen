//! Identity node: forwards input messages to output unchanged.
use limen_core::message::{Message, Payload};
use limen_core::node::{Node, NodeCapabilities, NodePolicy, StepContext, StepResult};
use limen_core::queue::{SpscQueue, enqueue_with_admission};
use limen_core::errors::NodeError;
use limen_core::memory::PlacementAcceptance;

/// A simple identity node that forwards input to output.
#[derive(Debug, Default, Clone)]
pub struct IdentityNode;

impl<InP: Payload> Node<1, 1, InP, InP> for IdentityNode {
    fn describe_capabilities(&self) -> NodeCapabilities {
        NodeCapabilities { device_streams: false, degrade_tiers: false }
    }

    fn input_acceptance(&self) -> [PlacementAcceptance; 1] {
        [PlacementAcceptance::host_all()]
    }

    fn output_acceptance(&self) -> [PlacementAcceptance; 1] {
        [PlacementAcceptance::host_all()]
    }

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
        ctx: &mut StepContext<1, 1, InP, InP, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<InP>>,
        OutQ: SpscQueue<Item = Message<InP>>,
    {
        let m = match ctx.inputs[0].try_pop() {
            Ok(m) => m,
            Err(_) => return Ok(StepResult::NoInput),
        };
        let res = enqueue_with_admission(&mut ctx.outputs[0], &ctx.out_policies[0], m);
        match res {
            limen_core::queue::EnqueueResult::Enqueued => Ok(StepResult::MadeProgress),
            _ => Ok(StepResult::Backpressured),
        }
    }
}
