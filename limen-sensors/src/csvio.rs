//! CSV reader source (std-only).
use std::fs::File;
use std::io::{BufRead, BufReader};

use limen_core::message::{Message, Payload, MessageHeader, MessageFlags};
use limen_core::node::{Node, NodeCapabilities, NodePolicy, StepContext, StepResult};
use limen_core::queue::{SpscQueue, enqueue_with_admission};
use limen_core::errors::NodeError;
use limen_core::memory::{PlacementAcceptance, MemoryClass};
use limen_core::types::{TraceId, SequenceNumber, Ticks};
use limen_processing::payload::Tensor1D;

/// CSV source that reads one tensor per line with comma-separated floats.
pub struct CsvSource1D<const N: usize> {
    rdr: BufReader<File>,
    trace: u64,
}

impl<const N: usize> CsvSource1D<N> {
    /// Open a CSV file.
    pub fn open(path: &str) -> std::io::Result<Self> {
        let f = File::open(path)?;
        Ok(Self { rdr: BufReader::new(f), trace: 0 })
    }
}

impl<const N: usize> Node<0, 1, Tensor1D<f32, N>, Tensor1D<f32, N>> for CsvSource1D<N> {
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
        let mut line = String::new();
        let n = self.rdr.read_line(&mut line).map_err(|_| NodeError::new(limen_core::errors::NodeErrorKind::ExecutionFailed, 1))?;
        if n == 0 {
            return Ok(StepResult::Terminal);
        }
        let mut data = [0.0f32; N];
        for (i, v) in line.trim().split(',').enumerate().take(N) {
            if let Ok(f) = v.parse::<f32>() { data[i] = f; }
        }
        self.trace = self.trace.wrapping_add(1);
        let payload = Tensor1D { data };
        let header = MessageHeader::new(
            limen_core::types::TraceId(self.trace),
            limen_core::types::SequenceNumber(self.trace),
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
