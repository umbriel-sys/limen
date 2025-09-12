//! Model node wrapper: converts any `ComputeModel` into a `Node`.
use limen_core::message::{Message, Payload};
use limen_core::node::{Node, NodeCapabilities, NodePolicy, StepContext, StepResult};
use limen_core::queue::{SpscQueue, enqueue_with_admission};
use limen_core::errors::NodeError;
use limen_core::memory::PlacementAcceptance;
use limen_core::compute::{ComputeModel, ModelMetadata};

/// A node that executes a compute model synchronously.
#[derive(Debug)]
pub struct ModelNode<M> {
    model: M,
    policy: NodePolicy,
}

impl<M> ModelNode<M> {
    /// Construct with a model and a node policy.
    pub const fn new(model: M, policy: NodePolicy) -> Self {
        Self { model, policy }
    }
}

impl<M, InP: Payload, OutP: Payload> Node<1, 1, InP, OutP> for ModelNode<M>
where
    M: ComputeModel<InP, OutP>,
{
    fn describe_capabilities(&self) -> NodeCapabilities {
        let meta: ModelMetadata = self.model.metadata();
        let _ = meta; // mapping to NodeCapabilities keeps flat for now
        NodeCapabilities { device_streams: false, degrade_tiers: false }
    }

    fn input_acceptance(&self) -> [PlacementAcceptance; 1] {
        [PlacementAcceptance::host_all()]
    }

    fn output_acceptance(&self) -> [PlacementAcceptance; 1] {
        [PlacementAcceptance::host_all()]
    }

    fn policy(&self) -> NodePolicy {
        self.policy
    }

    fn initialize<C, T>(&mut self, _clock: &C, _telemetry: &mut T) -> Result<(), NodeError> { Ok(()) }

    fn step<InQ, OutQ, C, T>(
        &mut self,
        ctx: &mut StepContext<1, 1, InP, OutP, InQ, OutQ, C, T>,
    ) -> Result<StepResult, NodeError>
    where
        InQ: SpscQueue<Item = Message<InP>>,
        OutQ: SpscQueue<Item = Message<OutP>>,
    {
        let m = match ctx.inputs[0].try_pop() {
            Ok(m) => m,
            Err(_) => return Ok(StepResult::NoInput),
        };

        // Prepare output payload and run the model.
        // NOTE: without allocation, we need an OutP instance. For PoC we reuse Default if available,
        // otherwise we attempt to transmute via assignment; here we require OutP: Default.
        // To keep generic and safe, we bound OutP: Default.
        // However, rather than forcing at trait level, we can require M::run to fill an out param;
        // we create a zeroed default and pass mutable ref.

        // Workaround: require OutP: Default at impl site (see where clause).
        let mut out_payload: OutP = crate::default_out::<OutP>();

        // Execute compute
        self.model.run(&m.payload, &mut out_payload).map_err(|_e| NodeError::new(limen_core::errors::NodeErrorKind::ExecutionFailed, 0))?;

        let header = m.header;
        let out_msg = Message::new(header, out_payload);
        let res = enqueue_with_admission(&mut ctx.outputs[0], &ctx.out_policies[0], out_msg);
        match res {
            limen_core::queue::EnqueueResult::Enqueued => Ok(StepResult::MadeProgress),
            _ => Ok(StepResult::Backpressured),
        }
    }
}

// Helper to get a default OutP without introducing trait bounds on Node itself.
pub(crate) fn default_out<T: Default>() -> T {
    T::default()
}
