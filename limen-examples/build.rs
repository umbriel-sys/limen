//! limen-examples/build.rs

use limen_codegen::builder::{Edge, GraphBuilder, GraphVisibility, Node};

use limen_core::{
    edge::bench::TestSpscRingBuf,
    node::bench::{TestCounterSourceU32_2, TestIdentityModelNodeU32_2, TestSinkNodeU32_2},
    policy::{AdmissionPolicy, EdgePolicy, OverBudgetAction, QueueCaps},
    prelude::{linux::NoStdLinuxMonotonicClock, Message},
};

fn main() {
    GraphBuilder::new("SimpleExampleGraph", GraphVisibility::Public)
        .node(
            Node::new(0)
                .ty::<TestCounterSourceU32_2<NoStdLinuxMonotonicClock>>()
                .in_ports(0)
                .out_ports(1)
                .in_payload::<()>()
                .out_payload::<u32>()
                .name(Some("src"))
                .ingress_policy(EdgePolicy::new(
                    QueueCaps::new(8, 6, None, None),
                    AdmissionPolicy::DropNewest,
                    OverBudgetAction::Drop,
                )),
        )
        .node(
            Node::new(1)
                .ty::<TestIdentityModelNodeU32_2<32>>()
                .in_ports(1)
                .out_ports(1)
                .in_payload::<u32>()
                .out_payload::<u32>()
                .name(Some("map")),
        )
        .node(
            Node::new(2)
                .ty::<TestSinkNodeU32_2>()
                .in_ports(1)
                .out_ports(0)
                .in_payload::<u32>()
                .out_payload::<()>()
                .name(Some("sink")),
        )
        .edge(
            Edge::new(0)
                .ty::<TestSpscRingBuf<Message<u32>, 8>>()
                .payload::<u32>()
                .from(0, 0)
                .to(1, 0)
                .policy(EdgePolicy::new(
                    QueueCaps::new(8, 6, None, None),
                    AdmissionPolicy::DropNewest,
                    OverBudgetAction::Drop,
                ))
                .name(Some("src->map")),
        )
        .edge(
            Edge::new(1)
                .ty::<TestSpscRingBuf<Message<u32>, 8>>()
                .payload::<u32>()
                .from(1, 0)
                .to(2, 0)
                .policy(EdgePolicy::new(
                    QueueCaps::new(8, 6, None, None),
                    AdmissionPolicy::DropNewest,
                    OverBudgetAction::Drop,
                ))
                .name(Some("map->sink")),
        )
        .finish()
        .write("simple_example_graph")
        .unwrap();
}
