//! Stdout sink node (std).
use limen_core::message::{Message, Payload};
use limen_core::node::{Node, NodeCapabilities, NodePolicy, StepContext, StepResult};
use limen_core::queue::SpscQueue;
use limen_core::errors::NodeError;
use limen_core::memory::PlacementAcceptance;

/// A simple stdout sink that prints Debug of the payload.
pub struct StdoutSink;

impl<P: core::fmt::Debug + Payload> Node<1, 0, P, super::super::limen_core::message::MessageFlags> for StdoutSink {
    fn describe_capabilities(&self) -> NodeCapabilities {
        NodeCapabilities { device_streams: false, degrade_tiers: false }
    }
    fn input_acceptance(&self) -> [PlacementAcceptance; 1] { [PlacementAcceptance::host_all()] }
    fn output_acceptance(&self) -> [PlacementAcceptance; 0] { [] }
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
        ctx: &mut StepContext<1, 0, P, super::super::limen_core::message::MessageFlags, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<P>>,
    {
        match ctx.inputs[0].try_pop() {
            Ok(m) => {
                #[cfg(feature = "std")]
                {
                    println!("[StdoutSink] payload: {:?}", m.payload);
                }
                Ok(StepResult::MadeProgress)
            }
            Err(_) => Ok(StepResult::NoInput),
        }
    }
}
