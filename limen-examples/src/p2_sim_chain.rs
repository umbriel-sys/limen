//! P2 example: Simulated source → Normalize → Identity model → Threshold → Stdout sink.
use limen_core::message::Message;
use limen_core::policy::{EdgePolicy, QueueCaps, AdmissionPolicy, OverBudgetAction};
use limen_core::node::NodePolicy;
use limen_processing::payload::{Tensor1D, Label};
use limen_processing::math::NormalizeNode;
use limen_processing::logic::{ThresholdNode, DebounceNode};
use limen_processing::identity::IdentityNode;
use limen_models::identity::{IdentityBackend, IdentityModel};
use limen_models::nodes::ModelNode;
use limen_sensors::simulated::SimulatedSource1D;
use limen_output::stdout::StdoutSink;
use limen::spsc::lockfree_ring::LockFreeRing;
use limen::scheduler::edf::EdfPolicy;
use limen::runtime::p2_single::{RuntimeP2, GraphP2};
use limen::platform::StdClock;

// Assemble a typed 5-stage chain using the P2 helper
use limen::graph::simple_chain::{SimpleChain5, SimpleChain5P2};
use limen_core::node::StepResult;

const N: usize = 8;

// Payloads
type P1 = Tensor1D<f32, N>;
type P2p = Tensor1D<f32, N>;
type P3 = Tensor1D<f32, N>;
type P4 = Label;

// Queues
type Q1 = LockFreeRing<Message<P1>>;
type Q2 = LockFreeRing<Message<P2p>>;
type Q3 = LockFreeRing<Message<P3>>;
type Q4 = LockFreeRing<Message<P4>>;

fn main() {
    // Nodes
    let source = SimulatedSource1D::<N>::new();
    let pre = NormalizeNode::<N>::new(1.0, 0.0);
    let model = ModelNode::new(IdentityModel, NodePolicy {
        batching: limen_core::policy::BatchingPolicy::none(),
        budget: limen_core::policy::BudgetPolicy { tick_budget: None },
        deadline: limen_core::policy::DeadlinePolicy { require_absolute_deadline: false, slack_tolerance_ns: None },
    });
    let post = IdentityNode;
    let sink = StdoutSink;

    // Queues
    let qcap = 64usize.next_power_of_two();
    let q1 = Q1::with_capacity(qcap);
    let q2 = Q2::with_capacity(qcap);
    let q3 = Q3::with_capacity(qcap);
    let q4 = Q4::with_capacity(qcap);

    // Edge policies (simple same policy for all)
    let caps = QueueCaps { max_items: qcap - 1, soft_items: (qcap / 2), max_bytes: None, soft_bytes: None };
    let ep = EdgePolicy { caps, admission: AdmissionPolicy::DeadlineAndQoSAware, over_budget: OverBudgetAction::Drop };

    let mut chain = SimpleChain5 {
        source, pre, model, post, sink,
        q1, q2, q3, q4,
        pol_q1: ep, pol_q2: ep, pol_q3: ep, pol_q4: ep,
    };

    let mut graph = SimpleChain5P2(chain);

    let clock = StdClock::new();
    let telemetry = limen_core::telemetry::NoopTelemetry::default();
    let policy = EdfPolicy::default();
    let mut rt = RuntimeP2::new(graph, clock, telemetry, policy);

    rt.initialize_and_start().expect("init");

    for _ in 0..100 {
        let _ = rt.run_cycle().expect("cycle");
    }

    let _ = rt.stop();
}
