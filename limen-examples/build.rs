//! limen-examples/build.rs

use limen_codegen::builder::{Edge, GraphBuilder, Node};

use syn::parse_quote;

fn main() {
    // Per-graph policy (tune as needed)
    let edge_policy: syn::Expr = parse_quote! {
        limen_core::policy::EdgePolicy {
            caps: limen_core::policy::QueueCaps {
                max_items: 8,
                soft_items: 6,
                max_bytes: None,
                soft_bytes: None,
            },
            admission: limen_core::policy::AdmissionPolicy::DropNewest,
            over_budget: limen_core::policy::OverBudgetAction::Drop,
        }
    };

    // Build a typed AST for the graph
    let ast = GraphBuilder::new(parse_quote!(pub), parse_quote!(SimpleExampleGraph))
        .node(
            Node::new(0)
                .ty(parse_quote!(
                    limen_core::node::bench::TestCounterSourceU32_2<
                        limen_core::prelude::linux::NoStdLinuxMonotonicClock,
                    >
                ))
                .in_ports(0)
                .out_ports(1)
                .in_payload(parse_quote!(()))
                .out_payload(parse_quote!(u32))
                .name(parse_quote!(Some("src")))
                .ingress_policy(edge_policy.clone()),
        )
        .node(
            Node::new(1)
                .ty(parse_quote!(
                    limen_core::node::bench::TestIdentityModelNodeU32_2<32>
                ))
                .in_ports(1)
                .out_ports(1)
                .in_payload(parse_quote!(u32))
                .out_payload(parse_quote!(u32))
                .name(parse_quote!(Some("map"))),
        )
        .node(
            Node::new(2)
                .ty(parse_quote!(limen_core::node::bench::TestSinkNodeU32_2))
                .in_ports(1)
                .out_ports(0)
                .in_payload(parse_quote!(u32))
                .out_payload(parse_quote!(()))
                .name(parse_quote!(Some("sink"))),
        )
        .edge(
            Edge::new(0)
                .ty(parse_quote!(
                    limen_core::edge::bench::TestSpscRingBuf<limen_core::message::Message<u32>, 8>
                ))
                .payload(parse_quote!(u32))
                .from(0, 0)
                .to(1, 0)
                .policy(edge_policy.clone())
                .name(parse_quote!(Some("src->map"))),
        )
        .edge(
            Edge::new(1)
                .ty(parse_quote!(
                    limen_core::edge::bench::TestSpscRingBuf<limen_core::message::Message<u32>, 8>
                ))
                .payload(parse_quote!(u32))
                .from(1, 0)
                .to(2, 0)
                .policy(edge_policy.clone())
                .name(parse_quote!(Some("map->sink"))),
        )
        .finish();

    // Expand and write (pretty-or-raw fallback)
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest_dir = std::path::Path::new(&out_dir).join("generated");
    std::fs::create_dir_all(&dest_dir).expect("create $OUT_DIR/generated");
    let dest = dest_dir.join("simple_example_graph.rs");
    limen_codegen::expand_ast_to_file(ast, &dest).expect("codegen failed");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:warning=generated graph A -> {}", dest.display());
}
