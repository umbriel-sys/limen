//! Shared helpers for limen-examples tests.

use limen_core::memory::PlacementAcceptance;
use limen_core::message::Message;
use limen_core::message::MessageFlags;
use limen_core::node::bench::{
    TestCounterSourceU32_2, TestIdentityModelNodeU32_2, TestSinkNodeU32_2, TestU32Backend,
};
use limen_core::node::NodeCapabilities;
use limen_core::policy::{BatchingPolicy, BudgetPolicy, DeadlinePolicy, EdgePolicy, NodePolicy};
use limen_core::types::{QoSClass, SequenceNumber, Ticks, TraceId};

pub const TEST_MAX_BATCH: usize = 32;
pub type MapNode = TestIdentityModelNodeU32_2<TEST_MAX_BATCH>;

// Queue type used by limen-examples graphs (matches build.rs)
pub type Q = limen_core::edge::spsc_ringbuf::SpscRingbuf<Message<u32>>;

#[inline]
pub fn example_node_policy() -> NodePolicy {
    NodePolicy {
        batching: BatchingPolicy {
            fixed_n: None,
            max_delta_t: None,
        },
        budget: BudgetPolicy {
            tick_budget: None,
            watchdog_ticks: None,
        },
        deadline: DeadlinePolicy {
            require_absolute_deadline: false,
            slack_tolerance_ns: None,
            default_deadline_ns: None,
        },
    }
}

#[inline]
pub fn example_edge_policy() -> EdgePolicy {
    EdgePolicy {
        caps: limen_core::policy::QueueCaps {
            max_items: 8,
            soft_items: 6,
            max_bytes: None,
            soft_bytes: None,
        },
        admission: limen_core::policy::AdmissionPolicy::DropNewest,
        over_budget: limen_core::policy::OverBudgetAction::Drop,
    }
}

#[cfg(feature = "std")]
pub fn sink_printer(s: &str) {
    println!("--- [***Sink Output***] --- {}", s);
}
#[cfg(not(feature = "std"))]
pub fn sink_printer(_: &str) {}

pub fn make_nodes() -> (TestCounterSourceU32_2, MapNode, TestSinkNodeU32_2) {
    let node_policy = example_node_policy();

    let src = TestCounterSourceU32_2::new(
        0,
        TraceId(0u64),
        SequenceNumber(0u64),
        Ticks(0u64),
        None,
        QoSClass::BestEffort,
        MessageFlags::empty(),
        NodeCapabilities::default(),
        node_policy,
        [PlacementAcceptance::default()],
    );

    let map = MapNode::new(
        TestU32Backend,
        (),
        example_node_policy(),
        NodeCapabilities::default(),
        [PlacementAcceptance::default()],
        [PlacementAcceptance::default()],
    )
    .expect("MapNode::new failed");

    let snk = TestSinkNodeU32_2::new(
        NodeCapabilities::default(),
        example_node_policy(),
        [PlacementAcceptance::default()],
        sink_printer,
    );

    (src, map, snk)
}
