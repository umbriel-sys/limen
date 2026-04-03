//! limen-examples/build.rs
//!
//! Build script generating example Limen graphs (no_std and concurrent/std variants).
//!
//! This defines a simple 3-node pipeline (source → map → sink) with two edges,
//! emitting codegen’d graph definitions for both single-threaded (SPSC) and
//! concurrent runtimes.
//!
//! ⚠️ Memory manager sizing:
//! The memory manager capacity must be at least:
//!
//!     edge queue capacity + downstream node batch size
//!
//! For example, with a queue of 32 and a node batch size of 32, the manager must
//! be ≥ 64 (or slightly above to avoid edge cases). Undersizing will cause drops
//! or stalled pipelines due to insufficient buffer availability.

use limen_codegen::builder::{Edge, GraphBuilder, GraphVisibility, Node};

use limen_core::{
    edge::bench::TestSpscRingBuf,
    memory::static_manager::StaticMemoryManager,
    node::bench::{TestCounterSourceTensor, TestIdentityModelNodeTensor, TestSinkNodeTensor},
    policy::{AdmissionPolicy, EdgePolicy, OverBudgetAction, QueueCaps},
    prelude::{linux::NoStdLinuxMonotonicClock, TestTensor},
};

#[cfg(feature = "std")]
use limen_core::prelude::{ConcurrentEdge, ConcurrentMemoryManager};

fn main() {
    GraphBuilder::new("SimpleExampleNoStdGraph", GraphVisibility::Public)
        .node(
            Node::new(0)
                .ty::<TestCounterSourceTensor<NoStdLinuxMonotonicClock, 32>>()
                .in_ports(0)
                .out_ports(1)
                .in_payload::<()>()
                .out_payload::<TestTensor>()
                .name(Some("src"))
                .ingress_policy(EdgePolicy::new(
                    QueueCaps::new(32, 32, None, None),
                    AdmissionPolicy::DropNewest,
                    OverBudgetAction::Drop,
                )),
        )
        .node(
            Node::new(1)
                .ty::<TestIdentityModelNodeTensor<32>>()
                .in_ports(1)
                .out_ports(1)
                .in_payload::<TestTensor>()
                .out_payload::<TestTensor>()
                .name(Some("map")),
        )
        .node(
            Node::new(2)
                .ty::<TestSinkNodeTensor>()
                .in_ports(1)
                .out_ports(0)
                .in_payload::<TestTensor>()
                .out_payload::<()>()
                .name(Some("sink")),
        )
        .edge(
            Edge::new(0)
                .ty::<TestSpscRingBuf<32>>()
                .payload::<TestTensor>()
                .manager_ty::<StaticMemoryManager<TestTensor, 35>>()
                .from(0, 0)
                .to(1, 0)
                .policy(EdgePolicy::new(
                    QueueCaps::new(32, 32, None, None),
                    AdmissionPolicy::DropNewest,
                    OverBudgetAction::Drop,
                ))
                .name(Some("src->map")),
        )
        .edge(
            Edge::new(1)
                .ty::<TestSpscRingBuf<32>>()
                .payload::<TestTensor>()
                .manager_ty::<StaticMemoryManager<TestTensor, 35>>()
                .from(1, 0)
                .to(2, 0)
                .policy(EdgePolicy::new(
                    QueueCaps::new(32, 32, None, None),
                    AdmissionPolicy::DropNewest,
                    OverBudgetAction::Drop,
                ))
                .name(Some("map->sink")),
        )
        .finish()
        .write("simple_example_nostd_graph")
        .unwrap();

    #[cfg(feature = "std")]
    GraphBuilder::new("SimpleExampleConcurrentGraph", GraphVisibility::Public)
        .node(
            Node::new(0)
                .ty::<TestCounterSourceTensor<NoStdLinuxMonotonicClock, 32>>()
                .in_ports(0)
                .out_ports(1)
                .in_payload::<()>()
                .out_payload::<TestTensor>()
                .name(Some("src"))
                .ingress_policy(EdgePolicy::new(
                    QueueCaps::new(32, 32, None, None),
                    AdmissionPolicy::DropNewest,
                    OverBudgetAction::Drop,
                )),
        )
        .node(
            Node::new(1)
                .ty::<TestIdentityModelNodeTensor<32>>()
                .in_ports(1)
                .out_ports(1)
                .in_payload::<TestTensor>()
                .out_payload::<TestTensor>()
                .name(Some("map")),
        )
        .node(
            Node::new(2)
                .ty::<TestSinkNodeTensor>()
                .in_ports(1)
                .out_ports(0)
                .in_payload::<TestTensor>()
                .out_payload::<()>()
                .name(Some("sink")),
        )
        .edge(
            Edge::new(0)
                .ty::<ConcurrentEdge>()
                .payload::<TestTensor>()
                .manager_ty::<ConcurrentMemoryManager<TestTensor>>()
                .from(0, 0)
                .to(1, 0)
                .policy(EdgePolicy::new(
                    QueueCaps::new(32, 32, None, None),
                    AdmissionPolicy::DropNewest,
                    OverBudgetAction::Drop,
                ))
                .name(Some("src->map")),
        )
        .edge(
            Edge::new(1)
                .ty::<ConcurrentEdge>()
                .payload::<TestTensor>()
                .manager_ty::<ConcurrentMemoryManager<TestTensor>>()
                .from(1, 0)
                .to(2, 0)
                .policy(EdgePolicy::new(
                    QueueCaps::new(32, 32, None, None),
                    AdmissionPolicy::DropNewest,
                    OverBudgetAction::Drop,
                ))
                .name(Some("map->sink")),
        )
        .concurrent(true)
        .finish()
        .write("simple_example_concurrent_graph")
        .unwrap();
}
