//! Code generation for Limen graphs.
//!
//! This module turns a validated parsed [`GraphDef`] into a `TokenStream`
//! containing a concrete graph type that implements `limen_core::graph::GraphApi`.
//!
//! # What this generator emits
//! - A concrete `struct <GraphName>` containing:
//!   - `nodes`: a tuple of `NodeLink<...>` (one entry per declared node).
//!   - `edges`: a tuple of `EdgeLink<...>` (one entry per declared *real* edge).
//! - An inherent `new(..)` constructor with arguments:
//!   - One `impl Into<NodeType>` parameter per node: `node_<idx>`.
//!   - One queue parameter per *real* edge: `q_<edge_id>`, where `edge_id` is
//!     offset by the ingress count (see below).
//! - A full `impl GraphApi<NODES, EDGES>` block:
//!   - Node and edge descriptors.
//!   - Edge occupancy queries for both ingress edges and real edges.
//!   - A `step_node_by_index(..)` dispatch.
//! - `impl GraphNodeAccess<I>` and `impl GraphEdgeAccess<E>` for ergonomic,
//!   index-based access to nodes/edges.
//! - `impl GraphNodeTypes<I, IN, OUT>` per node, resolving payload and queue
//!   types per port counts.
//! - `impl GraphNodeContextBuilder<I, IN, OUT>` per node to build a
//!   `StepContext` and run a node step with correct lifetimes and types.
//!
//! # Additionally, when the `std` feature is enabled
//! - `pub mod concurrent_graph` containing:
//!   - `struct <GraphName>Std`:
//!     - `nodes`: a tuple of `Option<NodeLink<...>>` (nodes can be moved out temporarily).
//!     - `edges`: a tuple of `ConcurrentEdgeLink<...>` (lock-free SPSC queue wrapper).
//!     - `endpoints`: per-edge `(ConsumerEndpoint, ProducerEndpoint)` pairs.
//!     - `ingress_edges`: probe-based ingress links for source nodes.
//!     - `ingress_updaters`: per-source `SourceIngressUpdater` handles.
//!     - `node_descs`: a cached `[NodeDescriptor; NODES]` array.
//!   - `enum <GraphName>StdOwnedBundle`: per-node owned-bundle for safe handoff.
//!   - `impl GraphApi<NODES, EDGES>` with the same surface as no-std **plus**:
//!     - `type OwnedBundle = <GraphName>StdOwnedBundle`
//!     - `take_owned_bundle_by_index`, `put_owned_bundle_by_index`,
//!       and `step_owned_bundle` (owned execution with endpoints detached).
//!
//! # Ingress edges
//! Nodes with `ingress_policy` in the DSL are treated as having an *implicit*
//! external ingress edge. These edges are numbered `0..ingress_count-1` and are
//! not present in the user-declared `edges { ... }` block. Real edges are then
//! numbered `ingress_count..(ingress_count + real_edge_count - 1)`.

use crate::ast::{EdgeDef, GraphDef, NodeDef};

use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::Index;

/// Emit the final `TokenStream` for a given validated [`GraphDef`].
///
///
/// This is the public entry to the generator. It emits **both** graph flavors:
/// - a non-`std` graph (no owned-bundle transfer) for `no_std` targets, and
/// - a concurrent `std` graph behind the `std` feature flag, which supports
///   owned-bundle handoff and SPSC endpoints for each edge.
///
/// # Returns
/// A `TokenStream2` containing the concrete graph types and trait impls (the
/// `std` flavor gated by `#[cfg(feature = "std")]`).
pub fn emit(g: &GraphDef) -> TokenStream2 {
    let vis = &g.vis;
    let name = &g.name;

    let ns = NonStd::new(g);
    let nonstd_tokens = ns.emit_nonstd_graph(vis, name);

    // Concurrent/std graph (feature-gated)
    let std_tokens = {
        let st = Std::new(g);
        st.emit_std_graph(vis, name)
    };

    quote! {
        #nonstd_tokens
        #std_tokens
    }
}

/* =========================
===== Non-std graph =====
========================= */

/// Internal generator for the non-`std` graph flavor.
///
/// This flavor does not support owned bundle transfer for edges. It focuses on
/// in-place stepping and occupancy queries suitable for constrained targets.
struct NonStd<'a> {
    /// Original parsed and validated graph definition.
    g: &'a GraphDef,
    /// Node indices that declared an `ingress_policy` (one implicit ingress edge each).
    ingress_nodes: Vec<usize>,
    /// The `EdgePolicy` tokens for ingress edges, ordered to match `ingress_nodes`.
    ingress_policies: Vec<TokenStream2>,
    /// For each node, the list of inbound *real* edge indices, sorted by `to_port`.
    in_edges_by_node: Vec<Vec<usize>>,
    /// For each node, the list of outbound *real* edge indices, sorted by `from_port`.
    out_edges_by_node: Vec<Vec<usize>>,
}

impl<'a> NonStd<'a> {
    /// Build the non-std generator view from a [`GraphDef`].
    ///
    /// - Collect ingress nodes and their policies.
    /// - Build per-node inbound and outbound edge index tables, sorted by port id.
    fn new(g: &'a GraphDef) -> Self {
        let mut ingress_nodes = Vec::new();
        let mut ingress_policies = Vec::new();
        for n in &g.nodes {
            if let Some(pol) = &n.ingress_policy_opt {
                ingress_nodes.push(n.idx);
                ingress_policies.push(quote! { #pol });
            }
        }

        let mut in_edges_by_node = vec![Vec::<usize>::new(); g.nodes.len()];
        let mut out_edges_by_node = vec![Vec::<usize>::new(); g.nodes.len()];
        let mut tmp_in: Vec<Vec<(usize, usize)>> = vec![Vec::new(); g.nodes.len()];
        let mut tmp_out: Vec<Vec<(usize, usize)>> = vec![Vec::new(); g.nodes.len()];

        for (eidx, e) in g.edges.iter().enumerate() {
            tmp_out[e.from_node].push((e.from_port, eidx));
            tmp_in[e.to_node].push((e.to_port, eidx));
        }
        for ni in 0..g.nodes.len() {
            tmp_in[ni].sort_by_key(|p| p.0);
            tmp_out[ni].sort_by_key(|p| p.0);
            in_edges_by_node[ni] = tmp_in[ni].iter().map(|p| p.1).collect();
            out_edges_by_node[ni] = tmp_out[ni].iter().map(|p| p.1).collect();
        }

        Self {
            g,
            ingress_nodes,
            ingress_policies,
            in_edges_by_node,
            out_edges_by_node,
        }
    }

    /// The number of implicit ingress edges.
    fn ingress_count(&self) -> usize {
        self.ingress_nodes.len()
    }

    /// Compute the *effective* node type:
    ///
    /// - If the node has `in_ports == 0` and `out_ports > 0`, it is a `SourceNode<..>`.
    /// - If the node has `out_ports == 0` and `in_ports > 0`, it is a `SinkNode<..>`.
    /// - Otherwise, use the node's declared type as-is.
    fn node_n_type(&self, n: &NodeDef) -> TokenStream2 {
        let ty = &n.ty;
        let in_p = &n.in_payload;
        let out_p = &n.out_payload;
        let in_ports = n.in_ports;
        let out_ports = n.out_ports;

        if in_ports == 0 && out_ports > 0 {
            quote! { limen_core::node::source::SourceNode<#ty, #out_p, #out_ports> }
        } else if out_ports == 0 && in_ports > 0 {
            quote! { limen_core::node::sink::SinkNode<#ty, #in_p, #in_ports> }
        } else {
            quote! { #ty }
        }
    }

    /// The `NodeLink` wrapper type for this node instance.
    fn node_link_type(&self, n: &NodeDef) -> TokenStream2 {
        let ntype = self.node_n_type(n);
        let in_p = &n.in_payload;
        let out_p = &n.out_payload;
        let in_ports = n.in_ports;
        let out_ports = n.out_ports;
        quote! { limen_core::node::link::NodeLink<#ntype, #in_ports, #out_ports, #in_p, #out_p> }
    }

    /// The `EdgeLink` wrapper type for this edge instance.
    fn edge_link_type(&self, e: &EdgeDef) -> TokenStream2 {
        let q = &e.ty;
        let p = &e.payload;
        quote! { limen_core::edge::link::EdgeLink<#q, #p> }
    }

    /// The tuple type that holds all node links.
    fn node_tuple_type(&self) -> TokenStream2 {
        let parts = self.g.nodes.iter().map(|n| self.node_link_type(n));
        quote! { ( #( #parts ),* ) }
    }

    /// The tuple type that holds all *real* edge links (ingress edges are implicit).
    fn edge_tuple_type(&self) -> TokenStream2 {
        let parts = self.g.edges.iter().map(|e| self.edge_link_type(e));
        quote! { ( #( #parts ),* ) }
    }

    /// Construct the node tuple, calling `NodeLink::new(..)` for every node.
    ///
    /// Each node constructor takes:
    /// - The node instance (`node_<idx>.into()` in the outer constructor).
    /// - A `NodeIndex`.
    /// - An optional display name.
    fn node_tuple_init(&self) -> TokenStream2 {
        let parts = self.g.nodes.iter().map(|n| {
            let id = n.idx;
            let node_ident = format_ident!("node_{}", id);
            let name_opt = n
                .name_opt
                .as_ref()
                .map(|e| quote! { #e })
                .unwrap_or(quote! { None });
            let ntype = self.node_n_type(n);
            let in_ports = n.in_ports;
            let out_ports = n.out_ports;
            let in_p = &n.in_payload;
            let out_p = &n.out_payload;
            quote! {
                limen_core::node::link::NodeLink::<#ntype, #in_ports, #out_ports, #in_p, #out_p>
                    ::new(
                        #node_ident.into(),
                        limen_core::types::NodeIndex::from(#id as usize),
                        #name_opt
                    )
            }
        });
        quote! { ( #( #parts ),* ) }
    }

    /// Construct the edge tuple, calling `EdgeLink::new(..)` for every *real* edge.
    ///
    /// Real edges are indexed after ingress, so `edge_id = ingress_count + e.idx`.
    /// Each edge constructor takes:
    /// - The queue instance `q_<edge_id>`.
    /// - An `EdgeIndex`.
    /// - Upstream and downstream `PortId`s.
    /// - The `EdgePolicy`.
    /// - An optional display name.
    fn edge_tuple_init(&self) -> TokenStream2 {
        let ingress_count = self.ingress_count();
        let parts = self.g.edges.iter().map(|e| {
            let id = e.idx + ingress_count;
            let up = e.from_node;
            let up_p = e.from_port;
            let dn = e.to_node;
            let dn_p = e.to_port;
            let pol = &e.policy;
            let name_opt = e
                .name_opt
                .as_ref()
                .map(|x| quote! { #x })
                .unwrap_or(quote! { None });

            let ety = &e.ty;
            let epayload = &e.payload;
            let q_ident = format_ident!("q_{}", id);

            quote! {
                limen_core::edge::link::EdgeLink::<#ety, #epayload>::new(
                    #q_ident,
                    limen_core::types::EdgeIndex::from(#id as usize),
                    limen_core::types::PortId {
                        node: limen_core::types::NodeIndex::from(#up as usize),
                        port: limen_core::types::PortIndex(#up_p),
                    },
                    limen_core::types::PortId {
                        node: limen_core::types::NodeIndex::from(#dn as usize),
                        port: limen_core::types::PortIndex(#dn_p),
                    },
                    #pol,
                    #name_opt
                )
            }
        });
        quote! { ( #( #parts ),* ) }
    }

    /// Build the argument list for the public `new(..)` constructor.
    ///
    /// - One node arg: `node_<idx> : impl Into<EffectiveNodeType>`
    /// - One queue arg per *real* edge (offset by ingress): `q_<edge_id> : QueueType`.
    fn ctor_args(&self) -> TokenStream2 {
        let node_args = self.g.nodes.iter().map(|n| {
            let id = n.idx;
            let ntype = self.node_n_type(n);
            let node_ident = format_ident!("node_{}", id);
            quote! { #node_ident : impl Into<#ntype> }
        });
        let ingress_count = self.ingress_count();
        let edge_args = self.g.edges.iter().map(|e| {
            let id = e.idx + ingress_count;
            let q = &e.ty;
            let q_ident = format_ident!("q_{}", id);
            quote! { #q_ident : #q }
        });
        let args: Vec<TokenStream2> = node_args.chain(edge_args).collect();
        quote! { #( #args ),* }
    }

    /// Create the node descriptor array (`[NodeDescriptor; NODES]`).
    fn node_desc_array(&self) -> TokenStream2 {
        let elems = self.g.nodes.iter().enumerate().map(|(i, _)| {
            let idx = Index::from(i);
            quote! { self.nodes.#idx.descriptor() }
        });
        quote! { [ #( #elems ),* ] }
    }

    /// Create the edge descriptor array (`[EdgeDescriptor; EDGES]`).
    ///
    /// Includes synthetic ingress edges first, followed by real edges from the tuple.
    fn edge_desc_array(&self) -> TokenStream2 {
        let ingress = self.ingress_nodes.iter().enumerate().map(|(k, &node_idx)| {
            let dn = node_idx;
            let name = format!("ingress{}", k);
            quote! {
                limen_core::edge::link::EdgeDescriptor {
                    id: limen_core::types::EdgeIndex::from(#k as usize),
                    upstream: limen_core::types::PortId {
                        node: limen_core::node::source::EXTERNAL_INGRESS_NODE,
                        port: limen_core::types::PortIndex(0),
                    },
                    downstream: limen_core::types::PortId {
                        node: limen_core::types::NodeIndex::from(#dn as usize),
                        port: limen_core::types::PortIndex(0),
                    },
                    name: Some(#name),
                }
            }
        });

        let reals = self.g.edges.iter().enumerate().map(|(j, _)| {
            let jidx = Index::from(j);
            quote! { self.edges.#jidx.descriptor() }
        });

        quote! { [ #( #ingress ),*, #( #reals ),* ] }
    }

    /// Emit a constant with ingress policies (if any).
    ///
    /// This creates:
    /// `const INGRESS_POLICIES: [EdgePolicy; INGRESS_COUNT] = [ ... ];`
    fn ingress_policies_const(&self) -> TokenStream2 {
        let cnt = self.ingress_count();
        if cnt == 0 {
            quote! {}
        } else {
            let elems = self.ingress_policies.iter();
            quote! {
                const INGRESS_POLICIES: [limen_core::policy::EdgePolicy; #cnt] = [ #( #elems ),* ];
            }
        }
    }

    /// Match on an edge id `E` and return its `EdgeOccupancy`.
    ///
    /// - For ingress edges: ask the owning source node via `ingress_occupancy(..)`.
    /// - For real edges: read the policy from the `EdgeLink` and call `occupancy(..)`.
    fn edge_occupancy_match(&self) -> TokenStream2 {
        let ingress_count = self.ingress_count();

        let ingress_arms = self.ingress_nodes.iter().enumerate().map(|(k, nidx)| {
            let npos = Index::from(*nidx);
            quote! {
                #k => {
                    let src = self.nodes.#npos.node().source_ref();
                    Ok(src.ingress_occupancy(&INGRESS_POLICIES[#k]))
                }
            }
        });

        let real_arms = self.g.edges.iter().enumerate().map(|(j, _)| {
            let eid = j + ingress_count;
            let jidx = Index::from(j);
            quote! {
                #eid => {
                    let e = &self.edges.#jidx;
                    Ok(e.occupancy(&e.policy()))
                }
            }
        });

        quote! {
            let occ = match E {
                #( #ingress_arms )*
                #( #real_arms )*
                _ => Err(limen_core::errors::GraphError::InvalidEdgeIndex),
            }?;
            Ok(occ)
        }
    }

    /// Write all edge occupancies (ingress first, then real edges) into `out`.
    fn write_all_occupancies(&self) -> TokenStream2 {
        let total = self.ingress_count() + self.g.edges.len();
        let assigns = (0..total).map(|k| {
            quote! { out[#k] = self.edge_occupancy_for::<#k>()?; }
        });
        quote! { #( #assigns )* Ok(()) }
    }

    /// Refresh the occupancies for all edges touching a specific node `I`.
    fn refresh_for_node(&self) -> TokenStream2 {
        let total = self.ingress_count() + self.g.edges.len();
        let arms = (0..total).map(|k| {
            let kk = syn::Index::from(k);
            quote! { #k => { out[#k] = self.edge_occupancy_for::<#kk>()?; } }
        });
        quote! {
            let node_idx = limen_core::types::NodeIndex::from(I);
            for ed in self.get_edge_descriptors().iter() {
                if ed.upstream.node == node_idx || ed.downstream.node == node_idx {
                    let k = (ed.id).as_usize();
                    match k {
                        #( #arms )*,
                        _ => unreachable!("invalid edge index"),
                    }
                }
            }
            Ok(())
        }
    }

    /// Dispatch a step call by node index.
    ///
    /// Expands a `match` with one arm per node index, delegating to
    /// `GraphNodeContextBuilder::with_node_and_step_context(..)` to construct
    /// the `StepContext` and then invoke `node.step(..)`.
    fn step_by_index(&self) -> TokenStream2 {
        let arms = self.g.nodes.iter().map(|n| {
            let i = n.idx;
            let in_ports = n.in_ports;
            let out_ports = n.out_ports;
            quote! {
                #i => <Self as limen_core::graph::GraphNodeContextBuilder<#i, #in_ports, #out_ports>>::with_node_and_step_context::<
                    C, T, limen_core::node::StepResult, limen_core::errors::NodeError
                >(self, clock, telemetry, |node, ctx| node.step(ctx)),
            }
        });
        quote! {
            match index {
                #( #arms )*
                _ => unreachable!("invalid node index"),
            }
        }
    }

    /// Emit `GraphNodeAccess<I>` impls for all nodes.
    fn graph_node_access_impls(&self, name: &Ident) -> Vec<TokenStream2> {
        self.g
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let idx = Index::from(i);
                let const_i = n.idx;
                let nlink = self.node_link_type(n);
                quote! {
                    impl limen_core::graph::GraphNodeAccess<#const_i> for #name {
                        type Node = #nlink;
                        #[inline] fn node_ref(&self) -> &Self::Node { &self.nodes.#idx }
                        #[inline] fn node_mut(&mut self) -> &mut Self::Node { &mut self.nodes.#idx }
                    }
                }
            })
            .collect()
    }

    /// Emit `GraphEdgeAccess<E>` impls for all *real* edges.
    fn graph_edge_access_impls(&self, name: &Ident) -> Vec<TokenStream2> {
        let ingress_count = self.ingress_count();
        self.g
            .edges
            .iter()
            .enumerate()
            .map(|(j, e)| {
                let eid = j + ingress_count;
                let ety = self.edge_link_type(e);
                let jidx = Index::from(j);
                quote! {
                    impl limen_core::graph::GraphEdgeAccess<#eid> for #name {
                        type Edge = #ety;
                        #[inline] fn edge_ref(&self) -> &Self::Edge { &self.edges.#jidx }
                        #[inline] fn edge_mut(&mut self) -> &mut Self::Edge { &mut self.edges.#jidx }
                    }
                }
            })
            .collect()
    }

    /// Emit `GraphNodeTypes<I, IN, OUT>` impls for all nodes.
    ///
    /// Determines queue types per side:
    /// - If a side has zero ports, uses `NoQueue<Payload>`.
    /// - Otherwise, the queue type is taken from the first edge (uniformity is
    ///   validated earlier).
    fn graph_node_types_impls(&self, name: &Ident) -> Vec<TokenStream2> {
        self.g
            .nodes
            .iter()
            .map(|n| {
                let i = n.idx;
                let in_p = &n.in_payload;
                let out_p = &n.out_payload;
                let in_ports = n.in_ports;
                let out_ports = n.out_ports;

                let inq_ty = if in_ports == 0 {
                    quote! { limen_core::edge::NoQueue<#in_p> }
                } else {
                    let e0 = self.in_edges_by_node[i][0];
                    let ety = &self.g.edges[e0].ty;
                    quote! { #ety }
                };
                let outq_ty = if out_ports == 0 {
                    quote! { limen_core::edge::NoQueue<#out_p> }
                } else {
                    let e0 = self.out_edges_by_node[i][0];
                    let ety = &self.g.edges[e0].ty;
                    quote! { #ety }
                };

                quote! {
                    impl limen_core::graph::GraphNodeTypes<#i, #in_ports, #out_ports> for #name {
                        type InP = #in_p;
                        type OutP = #out_p;
                        type InQ = #inq_ty;
                        type OutQ = #outq_ty;
                    }
                }
            })
            .collect()
    }

    /// Emit `GraphNodeContextBuilder<I, IN, OUT>` impls for all nodes.
    ///
    /// Each impl builds the arrays of input/output queues and policies in port
    /// order, provides in/out edge ids, and constructs a `StepContext`. It also
    /// supplies the `with_node_and_step_context(..)` helper to borrow the node
    /// and pass the context to a closure.
    fn graph_node_ctx_impls(&self, name: &Ident) -> Vec<TokenStream2> {
        self.g
            .nodes
            .iter()
            .enumerate()
            .map(|(tuple_pos, n)| {
                let tuple_idx = Index::from(tuple_pos);
                let i = n.idx;
                let in_ports = n.in_ports;
                let out_ports = n.out_ports;

                let input_qs: Vec<TokenStream2> = if in_ports == 0 {
                    vec![]
                } else {
                    self.in_edges_by_node[i]
                    .iter()
                    .map(|&eidx| {
                            let pos = Index::from(eidx);
                            quote! { self.edges.#pos.queue_mut() }
                        })
                        .collect()
                };

                let output_qs: Vec<TokenStream2> = if out_ports == 0 {
                    vec![]
                } else {
                    self.out_edges_by_node[i]
                    .iter()
                    .map(|&eidx| {
                            let pos = Index::from(eidx);
                            quote! { self.edges.#pos.queue_mut() }
                        })
                        .collect()
                };

                let in_pols: Vec<TokenStream2> = if in_ports == 0 {
                    vec![]
                } else {
                    self.in_edges_by_node[i]
                        .iter()
                        .map(|&eidx| {
                            let pos = Index::from(eidx);
                            quote! { self.edges.#pos.policy() }
                        })
                        .collect()
                };
                let out_pols: Vec<TokenStream2> = if out_ports == 0 {
                    vec![]
                } else {
                    self.out_edges_by_node[i]
                        .iter()
                        .map(|&eidx| {
                            let pos = Index::from(eidx);
                            quote! { self.edges.#pos.policy() }
                        })
                        .collect()
                };

                let ingress_count = self.ingress_count();
                let in_ids: Vec<usize> = if in_ports == 0 {
                    vec![]
                } else {
                    self.in_edges_by_node[i]
                        .iter()
                        .map(|&eidx| eidx + ingress_count)
                        .collect()
                };
                let out_ids: Vec<usize> = if out_ports == 0 {
                    vec![]
                } else {
                    self.out_edges_by_node[i]
                        .iter()
                        .map(|&eidx| eidx + ingress_count)
                        .collect()
                };

                let i_const = i;

                quote! {
                    impl limen_core::graph::GraphNodeContextBuilder<#i_const, #in_ports, #out_ports> for #name {
                        #[inline]
                        fn make_step_context<'graph, 'telemetry, 'clock, C, T>(
                            &'graph mut self,
                            clock: &'clock C,
                            telemetry: &'telemetry mut T,
                        ) -> limen_core::node::StepContext<
                            'graph, 'telemetry, 'clock,
                            #in_ports, #out_ports,
                            <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InP,
                            <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutP,
                            <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InQ,
                            <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutQ,
                            C, T
                        >
                        where
                            limen_core::policy::EdgePolicy: Copy,
                            C: limen_core::prelude::PlatformClock + Sized,
                            T: limen_core::prelude::Telemetry + Sized,
                        {
                            let in_policies: [limen_core::policy::EdgePolicy; #in_ports] = [ #( #in_pols ),* ];
                            let out_policies: [limen_core::policy::EdgePolicy; #out_ports] = [ #( #out_pols ),* ];

                            let inputs: [&'graph mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InQ; #in_ports] = [ #( #input_qs ),* ];
                            let outputs: [&'graph mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutQ; #out_ports] = [ #( #output_qs ),* ];

                            let node_id: u32 = #i_const as u32;
                            let in_edge_ids: [u32; #in_ports] = [ #( #in_ids as u32 ),* ];
                            let out_edge_ids: [u32; #out_ports] = [ #( #out_ids as u32 ),* ];

                            limen_core::node::StepContext::new(
                                inputs, outputs,
                                in_policies, out_policies,
                                node_id, in_edge_ids, out_edge_ids,
                                clock, telemetry
                            )
                        }

                        #[inline]
                        fn with_node_and_step_context<'telemetry, 'clock, C, T, R, E>(
                            &mut self,
                            clock: &'clock C,
                            telemetry: &'telemetry mut T,
                            f: impl FnOnce(
                                &mut <Self as limen_core::graph::GraphNodeAccess<#i_const>>::Node,
                                &mut limen_core::node::StepContext<
                                    '_,
                                    'telemetry,
                                    'clock,
                                    #in_ports, #out_ports,
                                    <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InP,
                                    <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutP,
                                    <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InQ,
                                    <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutQ,
                                    C, T
                                >,
                            ) -> Result<R, E>,
                        ) -> Result<R, E>
                        where
                            Self: limen_core::graph::GraphNodeAccess<#i_const>,
                            limen_core::policy::EdgePolicy: Copy,
                            C: limen_core::prelude::PlatformClock + Sized,
                            T: limen_core::prelude::Telemetry + Sized,
                        {
                            let in_policies: [limen_core::policy::EdgePolicy; #in_ports] = [ #( #in_pols ),* ];
                            let out_policies: [limen_core::policy::EdgePolicy; #out_ports] = [ #( #out_pols ),* ];

                            let inputs: [&mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InQ; #in_ports] = [ #( #input_qs ),* ];
                            let outputs: [&mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutQ; #out_ports] = [ #( #output_qs ),* ];

                            let node_id: u32 = #i_const as u32;
                            let in_edge_ids: [u32; #in_ports] = [ #( #in_ids as u32 ),* ];
                            let out_edge_ids: [u32; #out_ports] = [ #( #out_ids as u32 ),* ];

                            let mut ctx = limen_core::node::StepContext::new(
                                inputs, outputs,
                                in_policies, out_policies,
                                node_id, in_edge_ids, out_edge_ids,
                                clock, telemetry
                            );
                            f(&mut self.nodes.#tuple_idx, &mut ctx)
                        }
                    }
                }
            })
            .collect()
    }

    /// Emit the full non-`std` graph type and its trait impls.
    ///
    /// This includes:
    /// - The graph struct definition and `new(..)` constructor.
    /// - `GraphApi` implementation (descriptors, occupancy, stepping).
    /// - `GraphNodeAccess`, `GraphEdgeAccess`, `GraphNodeTypes`,
    ///   `GraphNodeContextBuilder` implementations for all indices.
    fn emit_nonstd_graph(&self, vis: &syn::Visibility, name: &Ident) -> TokenStream2 {
        let node_tuple_ty = self.node_tuple_type();
        let edge_tuple_ty = self.edge_tuple_type();
        let ctor_args = self.ctor_args();
        let node_tuple_init = self.node_tuple_init();
        let edge_tuple_init = self.edge_tuple_init();

        let node_count = self.g.nodes.len();
        let edge_count = self.ingress_count() + self.g.edges.len();

        let node_descs = self.node_desc_array();
        let edge_descs = self.edge_desc_array();
        let edge_occ_match = self.edge_occupancy_match();
        let write_all = self.write_all_occupancies();
        let refresh_one = self.refresh_for_node();
        let step_match = self.step_by_index();

        let node_access = self.graph_node_access_impls(name);
        let edge_access = self.graph_edge_access_impls(name);
        let node_types = self.graph_node_types_impls(name);
        let node_ctx = self.graph_node_ctx_impls(name);

        let ingress_pols_const = self.ingress_policies_const();

        quote! {
            #ingress_pols_const

            /// Non-std (embedded) graph flavor for this pipeline.
            ///
            /// This variant stores edges as plain `EdgeLink<Q, P>` using the queue
            /// types specified in the DSL, avoids allocating concurrent endpoints,
            /// and is suitable for `no_std` targets. Ingress edges are implicit
            /// (one per source node) and are not stored; their occupancy is obtained
            /// from the owning source node at runtime.
            #[allow(clippy::complexity)]
            #vis struct #name {
                /// Node links for all nodes in declaration order.
                nodes: #node_tuple_ty,
                /// Edge links for all *real* edges in declaration order.
                ///
                /// Note: implicit ingress edges are **not** stored here; they are
                /// synthesized for descriptors and occupancy queries.
                edges: #edge_tuple_ty,
            }

            impl #name {
                /// Construct a new **non-std** graph instance.
                ///
                /// # Parameters
                /// - One node parameter per node: `node_<idx> : impl Into<EffectiveNodeType>`.
                /// - One queue parameter per *real* edge (offset by ingress):
                ///   `q_<edge_id> : QueueType`.
                ///
                /// This builds `NodeLink` and `EdgeLink` values. Implicit ingress
                /// edges (for sources) are not stored; their occupancy is computed
                /// on demand via the source node.
                #[inline]
                pub fn new( #ctor_args ) -> Self {
                    let nodes = #node_tuple_init;
                    let edges = #edge_tuple_init;
                    Self { nodes, edges }
                }
            }

            impl limen_core::graph::GraphApi<#node_count, #edge_count> for #name {
                #[inline]
                fn get_node_descriptors(&self) -> [limen_core::node::link::NodeDescriptor; #node_count] {
                    #node_descs
                }
                #[inline]
                fn get_edge_descriptors(&self) -> [limen_core::edge::link::EdgeDescriptor; #edge_count] {
                    #edge_descs
                }
                #[inline]
                fn edge_occupancy_for<const E: usize>(&self) -> Result<limen_core::edge::EdgeOccupancy, limen_core::errors::GraphError> {
                    #edge_occ_match
                }
                #[inline]
                fn write_all_edge_occupancies(&self, out: &mut [limen_core::edge::EdgeOccupancy; #edge_count]) -> Result<(), limen_core::errors::GraphError> {
                    #write_all
                }
                #[inline]
                fn refresh_occupancies_for_node<const I: usize, const IN: usize, const OUT: usize>(
                    &self,
                    out: &mut [limen_core::edge::EdgeOccupancy; #edge_count],
                ) -> Result<(), limen_core::errors::GraphError> {
                    #refresh_one
                }
                #[inline]
                fn step_node_by_index<C, T>(
                    &mut self,
                    index: usize,
                    clock: &C,
                    telemetry: &mut T,
                ) -> Result<limen_core::node::StepResult, limen_core::errors::NodeError>
                where
                    limen_core::policy::EdgePolicy: Copy,
                    C: limen_core::prelude::PlatformClock + Sized,
                    T: limen_core::prelude::Telemetry + Sized,
                {
                    #step_match
                }

                // Provide the OwnedBundle API unconditionally so GraphApi is fully implemented.
                #[cfg(feature = "std")]
                type OwnedBundle = ();

                #[cfg(feature = "std")]
                #[inline]
                fn take_owned_bundle_by_index(&mut self, _index: usize)
                    -> Result<Self::OwnedBundle, limen_core::errors::GraphError>
                {
                    Err(limen_core::errors::GraphError::InvalidEdgeIndex)
                }

                #[cfg(feature = "std")]
                #[inline]
                fn put_owned_bundle_by_index(&mut self, _bundle: Self::OwnedBundle)
                    -> Result<(), limen_core::errors::GraphError>
                {
                    Err(limen_core::errors::GraphError::InvalidEdgeIndex)
                }

                #[cfg(feature = "std")]
                #[inline]
                fn step_owned_bundle<C, T>(
                    _bundle: &mut Self::OwnedBundle,
                    _clock: &C,
                    _telemetry: &mut T,
                ) -> Result<limen_core::node::StepResult, limen_core::errors::NodeError>
                where
                    limen_core::policy::EdgePolicy: Copy,
                    C: limen_core::prelude::PlatformClock + Sized,
                    T: limen_core::prelude::Telemetry + Sized,
                {
                    Err(limen_core::errors::NodeError::execution_failed().with_code(1))
                }
            }

            #(#node_access)*
            #(#edge_access)*
            #(#node_types)*
            #(#node_ctx)*
        }
    }
}

/* ======================
===== Std graph  =====
====================== */

/// Internal generator for the `std` (concurrent) graph flavor.
///
/// This flavor:
/// - uses lock-free SPSC queues via `ConcurrentEdgeLink` and per-edge
///   `ConsumerEndpoint`/`ProducerEndpoint`,
/// - exposes external ingress to sources via probe-based ingress edges,
/// - and implements the owned-bundle handoff API to move a node and its
///   endpoints out of the graph safely (and restore them later).
struct Std<'a> {
    /// Original parsed and validated graph definition.
    g: &'a GraphDef,
    /// Indices of nodes that declared an implicit ingress edge (i.e. sources).
    ingress_nodes: Vec<usize>,
    /// The `EdgePolicy` tokens for implicit ingress edges, ordered to match `ingress_nodes`.
    ingress_policies: Vec<TokenStream2>,
    /// For each node, the list of inbound *real* edge indices, sorted by `to_port`.
    in_edges_by_node: Vec<Vec<usize>>,
    /// For each node, the list of outbound *real* edge indices, sorted by `from_port`.
    out_edges_by_node: Vec<Vec<usize>>,
    /// Map from node index to its position in `ingress_nodes` (if the node is a source).
    source_pos_by_node: Vec<Option<usize>>,
}

impl<'a> Std<'a> {
    /// Build the `std` generator view from a [`GraphDef`].
    ///
    /// This reuses the indexing and per-node edge tables from the non-`std`
    /// generator and computes `source_pos_by_node` to quickly locate a node's
    /// ingress slot (if any).
    fn new(g: &'a GraphDef) -> Self {
        // Reuse same indexing as NonStd
        let ns = NonStd::new(g);
        let mut source_pos_by_node = vec![None; g.nodes.len()];
        for (k, &nidx) in ns.ingress_nodes.iter().enumerate() {
            source_pos_by_node[nidx] = Some(k);
        }
        Self {
            g,
            ingress_nodes: ns.ingress_nodes,
            ingress_policies: ns.ingress_policies,
            in_edges_by_node: ns.in_edges_by_node,
            out_edges_by_node: ns.out_edges_by_node,
            source_pos_by_node,
        }
    }

    /// The number of implicit ingress edges (one per source node).
    fn ingress_count(&self) -> usize {
        self.ingress_nodes.len()
    }

    /// Compute the effective node type (same inference as non-`std`):
    /// `SourceNode` for zero-input nodes, `SinkNode` for zero-output nodes,
    /// otherwise the declared node type.
    fn node_n_type(&self, n: &NodeDef) -> TokenStream2 {
        NonStd {
            g: self.g,
            ingress_nodes: self.ingress_nodes.clone(),
            ingress_policies: self.ingress_policies.clone(),
            in_edges_by_node: self.in_edges_by_node.clone(),
            out_edges_by_node: self.out_edges_by_node.clone(),
        }
        .node_n_type(n)
    }

    /// The `NodeLink` wrapper type for this node instance.
    fn node_link_type(&self, n: &NodeDef) -> TokenStream2 {
        let ntype = self.node_n_type(n);
        let in_p = &n.in_payload;
        let out_p = &n.out_payload;
        let in_ports = n.in_ports;
        let out_ports = n.out_ports;
        quote! { limen_core::node::link::NodeLink<#ntype, #in_ports, #out_ports, #in_p, #out_p> }
    }

    /// The concurrent `EdgeLink` wrapper type (`ConcurrentEdgeLink`) for a real edge.
    fn std_edge_link_type(&self, e: &EdgeDef) -> TokenStream2 {
        let q = &e.ty;
        let p = &e.payload;
        quote! { limen_core::edge::link::ConcurrentEdgeLink<#q, #p> }
    }

    /// The per-edge `(ConsumerEndpoint, ProducerEndpoint)` tuple type.
    fn endpoints_pair_ty(&self, e: &EdgeDef) -> TokenStream2 {
        let q = &e.ty;
        let p = &e.payload;
        quote! {
            (
                limen_core::edge::spsc_concurrent::ConsumerEndpoint<#p, limen_core::edge::spsc_concurrent::ConcurrentQueue<#q>>,
                limen_core::edge::spsc_concurrent::ProducerEndpoint<#p, limen_core::edge::spsc_concurrent::ConcurrentQueue<#q>>
            )
        }
    }

    /// The tuple type that holds all node links, wrapped in `Option` so nodes
    /// can be moved out for owned-bundle execution.
    fn nodes_tuple_ty_with_option(&self) -> TokenStream2 {
        let parts = self.g.nodes.iter().map(|n| {
            let nlt = self.node_link_type(n);
            quote! { core::option::Option<#nlt> }
        });
        quote! { ( #( #parts ),* ) }
    }

    /// The tuple type that holds all *real* concurrent edge links.
    fn edges_tuple_ty_std(&self) -> TokenStream2 {
        let parts = self.g.edges.iter().map(|e| self.std_edge_link_type(e));
        quote! { ( #( #parts ),* ) }
    }

    /// The tuple type that holds all per-edge endpoint pairs.
    fn endpoints_tuple_ty(&self) -> TokenStream2 {
        let parts = self.g.edges.iter().map(|e| self.endpoints_pair_ty(e));
        quote! { ( #( #parts ),* ) }
    }

    /// The tuple type that holds probe-based ingress edge links, one per source.
    /// This tuple is heterogeneous because each source has its own payload type.
    fn ingress_edges_tuple_ty(&self) -> TokenStream2 {
        let count = self.ingress_nodes.len();
        if count == 0 {
            quote! { () }
        } else if count == 1 {
            let nidx = self.ingress_nodes[0];
            let out_p = &self.g.nodes[nidx].out_payload;
            quote! {
                (
                    limen_core::node::source::probe::ConcurrentIngressEdgeLink<#out_p>,
                )
            }
        } else {
            // Heterogeneous tuple: one per source node, payload typed on the source's out_payload.
            let parts = self.ingress_nodes.iter().map(|&nidx| {
                let out_p = &self.g.nodes[nidx].out_payload;
                quote! { limen_core::node::source::probe::ConcurrentIngressEdgeLink<#out_p> }
            });
            quote! { ( #( #parts ),* ) }
        }
    }

    /// The tuple type that holds `Option<SourceIngressUpdater>` for each source.
    fn ingress_updaters_tuple_ty(&self) -> TokenStream2 {
        let count = self.ingress_nodes.len();
        if count == 0 {
            quote! { () }
        } else if count == 1 {
            quote! {
                (
                    core::option::Option<
                        limen_core::node::source::probe::SourceIngressUpdater
                    >,
                )
            }
        } else {
            // Homogeneous updater type; tuple of Options, one per source.
            let t = quote! {
                core::option::Option<
                    limen_core::node::source::probe::SourceIngressUpdater
                >
            };
            let parts = self.ingress_nodes.iter().map(|_| quote! { #t });
            quote! { ( #( #parts ),* ) }
        }
    }

    /// Build the argument list for the public `new(..)` constructor of the
    /// generated concurrent graph:
    /// - One `impl Into<EffectiveNodeType>` parameter per node: `node_<idx>`.
    /// - One queue parameter per *real* edge (offset by ingress): `q_<edge_id>`.
    fn ctor_args(&self) -> TokenStream2 {
        let node_args = self.g.nodes.iter().map(|n| {
            let id = n.idx;
            let ntype = self.node_n_type(n);
            let node_ident = format_ident!("node_{}", id);
            quote! { #node_ident : impl Into<#ntype> }
        });
        let ingress_count = self.ingress_count();
        let edge_args = self.g.edges.iter().map(|e| {
            let id = e.idx + ingress_count;
            let q = &e.ty;
            let q_ident = format_ident!("q_{}", id);
            quote! { #q_ident : #q }
        });
        let args: Vec<TokenStream2> = node_args.chain(edge_args).collect();
        quote! { #( #args ),* }
    }

    /// Construct the nodes tuple for the concurrent graph, wrapping each
    /// `NodeLink` in `Some(..)` so it can later be moved out as part of an
    /// owned bundle.
    fn nodes_tuple_init_with_option(&self) -> TokenStream2 {
        let parts = self.g.nodes.iter().map(|n| {
            let id = n.idx;
            let node_ident = format_ident!("node_{}", id);
            let name_opt = n
                .name_opt
                .as_ref()
                .map(|e| quote! { #e })
                .unwrap_or(quote! { None });
            let ntype = self.node_n_type(n);
            let in_ports = n.in_ports;
            let out_ports = n.out_ports;
            let in_p = &n.in_payload;
            let out_p = &n.out_payload;
            quote! {
                core::option::Option::Some(
                    limen_core::node::link::NodeLink::<#ntype, #in_ports, #out_ports, #in_p, #out_p>
                        ::new(#node_ident.into(), limen_core::types::NodeIndex::from(#id as usize), #name_opt)
                )
            }
        });
        quote! { ( #( #parts ),* ) }
    }

    /// Produce a series of `let e_j = ConcurrentEdgeLink::<..>::new(..);` declarations
    /// for each real edge, and return (`decls`, `tuple`) tokens where `tuple` is
    /// `( e_0, e_1, ... )`.
    fn edges_let_bindings_init(&self) -> (TokenStream2, TokenStream2) {
        // Produce `let e_j = ConcurrentEdgeLink::<..>::new(...);` for each edge,
        // then a tuple `( e_0, e_1, ... )`.
        let ingress_count = self.ingress_count();

        let decls = self.g.edges.iter().map(|e| {
            let id = e.idx + ingress_count;
            let up = e.from_node;
            let up_p = e.from_port;
            let dn = e.to_node;
            let dn_p = e.to_port;
            let pol = &e.policy;
            let name_opt = e
                .name_opt
                .as_ref()
                .map(|x| quote! { #x })
                .unwrap_or(quote! { None });
            let ety = &e.ty;
            let epayload = &e.payload;
            let q_ident = format_ident!("q_{}", id);
            let e_var = format_ident!("e_{}", e.idx);
            quote! {
                let #e_var = limen_core::edge::link::ConcurrentEdgeLink::<#ety, #epayload>::new(
                    #q_ident,
                    limen_core::types::EdgeIndex::from(#id as usize),
                    limen_core::types::PortId {
                        node: limen_core::types::NodeIndex::from(#up as usize),
                        port: limen_core::types::PortIndex(#up_p),
                    },
                    limen_core::types::PortId {
                        node: limen_core::types::NodeIndex::from(#dn as usize),
                        port: limen_core::types::PortIndex(#dn_p),
                    },
                    #pol,
                    #name_opt
                );
            }
        });

        let tuple = {
            let elems = self.g.edges.iter().map(|e| {
                let e_var = format_ident!("e_{}", e.idx);
                quote! { #e_var }
            });
            quote! { ( #( #elems ),* ) }
        };

        (quote! { #( #decls )* }, tuple)
    }

    /// Construct the per-edge endpoint tuple by cloning from each edge's shared
    /// queue `Arc`.
    fn endpoints_tuple_init(&self) -> TokenStream2 {
        let parts = self.g.edges.iter().map(|e| {
            let e_var = format_ident!("e_{}", e.idx);
            quote! {
                {
                    let c = limen_core::edge::spsc_concurrent::ConcurrentQueue::from_arc(#e_var.arc());
                    let p = limen_core::edge::spsc_concurrent::ConcurrentQueue::from_arc(#e_var.arc());
                    (
                        limen_core::edge::spsc_concurrent::ConsumerEndpoint::new(c),
                        limen_core::edge::spsc_concurrent::ProducerEndpoint::new(p)
                    )
                }
            }
        });
        if self.g.edges.is_empty() {
            quote! { () }
        } else {
            quote! { ( #( #parts ),* ) }
        }
    }

    /// Emit a constant with ingress policies (if any) for the concurrent graph.
    fn ingress_policies_const(&self) -> TokenStream2 {
        let cnt = self.ingress_count();
        if cnt == 0 {
            quote! {}
        } else {
            let elems = self.ingress_policies.iter();
            quote! {
                const INGRESS_POLICIES: [limen_core::policy::EdgePolicy; #cnt] = [ #( #elems ),* ];
            }
        }
    }

    /// Build probe-based ingress edges and keep their `SourceIngressUpdater`s.
    ///
    /// Returns (`decls`, `(edges_tuple, updaters_tuple)`) tokens.
    fn ingress_edges_tuple_init(&self) -> (TokenStream2, TokenStream2) {
        // Build probe edges and keep updaters (Some(..)) for each source.
        let decls = self.ingress_nodes.iter().enumerate().map(|(k, &nidx)| {
            let out_p = &self.g.nodes[nidx].out_payload;
            let e_var = format_ident!("ing_e_{}", k);
            let u_var = format_ident!("ing_u_{}", k);
            quote! {
                let (#e_var, #u_var) = limen_core::node::source::probe::new_probe_edge_pair::<#out_p>();
            }
        });

        let edges_tuple = {
            let count = self.ingress_nodes.len();
            if count == 0 {
                quote! { () }
            } else if count == 1 {
                let k = 0usize;
                let nidx = self.ingress_nodes[0];
                let e_var = format_ident!("ing_e_{}", k);
                let dn = nidx;
                let name = format!("ingress{}", k);
                quote! {
                    (
                        limen_core::node::source::probe::ConcurrentIngressEdgeLink::from_probe(
                            #e_var,
                            limen_core::types::EdgeIndex::from(#k as usize),
                            limen_core::types::PortId {
                                node: limen_core::node::source::EXTERNAL_INGRESS_NODE,
                                port: limen_core::types::PortIndex(0),
                            },
                            limen_core::types::PortId {
                                node: limen_core::types::NodeIndex::from(#dn as usize),
                                port: limen_core::types::PortIndex(0),
                            },
                            INGRESS_POLICIES[#k],
                            Some(#name),
                        ),
                    )
                }
            } else {
                let elems = self.ingress_nodes.iter().enumerate().map(|(k, &nidx)| {
                    let e_var = format_ident!("ing_e_{}", k);
                    let dn = nidx;
                    let name = format!("ingress{}", k);
                    quote! {
                        limen_core::node::source::probe::ConcurrentIngressEdgeLink::from_probe(
                            #e_var,
                            limen_core::types::EdgeIndex::from(#k as usize),
                            limen_core::types::PortId {
                                node: limen_core::node::source::EXTERNAL_INGRESS_NODE,
                                port: limen_core::types::PortIndex(0),
                            },
                            limen_core::types::PortId {
                                node: limen_core::types::NodeIndex::from(#dn as usize),
                                port: limen_core::types::PortIndex(0),
                            },
                            INGRESS_POLICIES[#k],
                            Some(#name),
                        )
                    }
                });
                quote! { ( #( #elems ),* ) }
            }
        };

        let updaters_tuple = {
            let count = self.ingress_nodes.len();
            if count == 0 {
                quote! { () }
            } else if count == 1 {
                let k = 0usize;
                let u_var = format_ident!("ing_u_{}", k);
                quote! { ( core::option::Option::Some(#u_var), ) }
            } else {
                let elems = self.ingress_nodes.iter().enumerate().map(|(k, _)| {
                    let u_var = format_ident!("ing_u_{}", k);
                    quote! { core::option::Option::Some(#u_var) }
                });
                quote! { ( #( #elems ),* ) }
            }
        };

        (
            quote! { #( #decls )* },
            quote! { (#edges_tuple, #updaters_tuple) },
        )
    }

    /// Cache `[NodeDescriptor; NODES]` at construction time so descriptors
    /// remain available even if nodes are temporarily moved out.
    fn node_descs_array_cache_init(&self) -> TokenStream2 {
        let parts = self.g.nodes.iter().enumerate().map(|(i, _)| {
            let idx = Index::from(i);
            quote! { nodes.#idx.as_ref().unwrap().descriptor() }
        });
        let n = self.g.nodes.len();
        if n == 0 {
            quote! { [] }
        } else {
            quote! { [ #( #parts ),* ] }
        }
    }

    /// Compute the concrete concurrent graph type name: `<GraphName>Std`.
    fn std_graph_name(&self, base: &Ident) -> Ident {
        format_ident!("{}Std", base)
    }

    /// Compute the owned-bundle enum type name: `<GraphName>StdOwnedBundle`.
    fn std_owned_bundle_name(&self, base: &Ident) -> Ident {
        format_ident!("{}StdOwnedBundle", base)
    }

    /// Emit `GraphNodeAccess<I>` impls for all nodes (using `Option` storage).
    fn graph_node_access_impls(&self, gname: &Ident) -> Vec<TokenStream2> {
        self.g.nodes.iter().enumerate().map(|(i, n)| {
            let idx = Index::from(i);
            let const_i = n.idx;
            let nlink = self.node_link_type(n);
            quote! {
                impl limen_core::graph::GraphNodeAccess<#const_i> for #gname {
                    type Node = #nlink;
                    #[inline] fn node_ref(&self) -> &Self::Node { self.nodes.#idx.as_ref().expect("node moved") }
                    #[inline] fn node_mut(&mut self) -> &mut Self::Node { self.nodes.#idx.as_mut().expect("node moved") }
                }
            }
        }).collect()
    }

    /// Emit `GraphEdgeAccess<E>` impls for all *real* edges.
    fn graph_edge_access_impls(&self, gname: &Ident) -> Vec<TokenStream2> {
        let ingress_count = self.ingress_count();
        self.g.edges.iter().enumerate().map(|(j, e)| {
            let eid = j + ingress_count;
            let ety = self.std_edge_link_type(e);
            let jidx = Index::from(j);
            quote! {
                impl limen_core::graph::GraphEdgeAccess<#eid> for #gname {
                    type Edge = #ety;
                    #[inline] fn edge_ref(&self) -> &Self::Edge { &self.edges.#jidx }
                    #[inline] fn edge_mut(&mut self) -> &mut Self::Edge { &mut self.edges.#jidx }
                }
            }
        }).collect()
    }

    /// Select the input endpoint queue type for node `i` (or `NoQueue` if no inputs).
    fn std_inq_ty_for_node(&self, i: usize) -> TokenStream2 {
        let in_p = &self.g.nodes[i].in_payload;
        if self.g.nodes[i].in_ports == 0 {
            quote! { limen_core::edge::NoQueue<#in_p> }
        } else {
            let e0 = self.in_edges_by_node[i][0];
            let ety = &self.g.edges[e0].ty;
            quote! { limen_core::edge::spsc_concurrent::ConsumerEndpoint<#in_p, limen_core::edge::spsc_concurrent::ConcurrentQueue<#ety>> }
        }
    }

    /// Select the output endpoint queue type for node `i` (or `NoQueue` if no outputs).
    fn std_outq_ty_for_node(&self, i: usize) -> TokenStream2 {
        let out_p = &self.g.nodes[i].out_payload;
        if self.g.nodes[i].out_ports == 0 {
            quote! { limen_core::edge::NoQueue<#out_p> }
        } else {
            let e0 = self.out_edges_by_node[i][0];
            let ety = &self.g.edges[e0].ty;
            quote! { limen_core::edge::spsc_concurrent::ProducerEndpoint<#out_p, limen_core::edge::spsc_concurrent::ConcurrentQueue<#ety>> }
        }
    }

    /// Emit `GraphNodeTypes<I, IN, OUT>` impls for all nodes (concurrent flavor).
    fn graph_node_types_impls_std(&self, gname: &Ident) -> Vec<TokenStream2> {
        self.g
            .nodes
            .iter()
            .map(|n| {
                let i = n.idx;
                let in_p = &n.in_payload;
                let out_p = &n.out_payload;
                let in_ports = n.in_ports;
                let out_ports = n.out_ports;
                let inq_ty = self.std_inq_ty_for_node(i);
                let outq_ty = self.std_outq_ty_for_node(i);
                quote! {
                    impl limen_core::graph::GraphNodeTypes<#i, #in_ports, #out_ports> for #gname {
                        type InP = #in_p;
                        type OutP = #out_p;
                        type InQ = #inq_ty;
                        type OutQ = #outq_ty;
                    }
                }
            })
            .collect()
    }

    /// Emit `GraphNodeContextBuilder<I, IN, OUT>` impls for all nodes using
    /// persistent endpoints and policies.
    fn graph_node_ctx_impls_std(&self, gname: &Ident) -> Vec<TokenStream2> {
        self.g.nodes.iter().enumerate().map(|(tuple_pos, n)| {
            let tuple_idx = Index::from(tuple_pos);
            let i = n.idx;
            let in_ports = n.in_ports;
            let out_ports = n.out_ports;

            let input_eps: Vec<TokenStream2> = if in_ports == 0 {
                vec![]
            } else {
                self.in_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { &mut (self.endpoints.#pos).0 }
                }).collect()
            };

            let output_eps: Vec<TokenStream2> = if out_ports == 0 {
                vec![]
            } else {
                self.out_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { &mut (self.endpoints.#pos).1 }
                }).collect()
            };

            let in_pols: Vec<TokenStream2> = if in_ports == 0 {
                vec![]
            } else {
                self.in_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { self.edges.#pos.policy() }
                }).collect()
            };
            let out_pols: Vec<TokenStream2> = if out_ports == 0 {
                vec![]
            } else {
                self.out_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { self.edges.#pos.policy() }
                }).collect()
            };

            let ingress_count = self.ingress_count();
            let in_ids: Vec<usize> = if in_ports == 0 { vec![] } else {
                self.in_edges_by_node[i].iter().map(|&eidx| eidx + ingress_count).collect()
            };
            let out_ids: Vec<usize> = if out_ports == 0 { vec![] } else {
                self.out_edges_by_node[i].iter().map(|&eidx| eidx + ingress_count).collect()
            };

            let i_const = i;

            quote! {
                impl limen_core::graph::GraphNodeContextBuilder<#i_const, #in_ports, #out_ports> for #gname {
                    #[inline]
                    fn make_step_context<'graph, 'telemetry, 'clock, C, T>(
                        &'graph mut self,
                        clock: &'clock C,
                        telemetry: &'telemetry mut T,
                    ) -> limen_core::node::StepContext<
                        'graph, 'telemetry, 'clock,
                        #in_ports, #out_ports,
                        <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InP,
                        <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutP,
                        <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InQ,
                        <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutQ,
                        C, T
                    >
                    where
                        limen_core::policy::EdgePolicy: Copy,
                        C: limen_core::prelude::PlatformClock + Sized,
                        T: limen_core::prelude::Telemetry + Sized,
                    {
                        let in_policies: [limen_core::policy::EdgePolicy; #in_ports] = [ #( #in_pols ),* ];
                        let out_policies: [limen_core::policy::EdgePolicy; #out_ports] = [ #( #out_pols ),* ];

                        let inputs: [&'graph mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InQ; #in_ports] = [ #( #input_eps ),* ];
                        let outputs: [&'graph mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutQ; #out_ports] = [ #( #output_eps ),* ];

                        let node_id: u32 = #i_const as u32;
                        let in_edge_ids: [u32; #in_ports] = [ #( #in_ids as u32 ),* ];
                        let out_edge_ids: [u32; #out_ports] = [ #( #out_ids as u32 ),* ];

                        limen_core::node::StepContext::new(
                            inputs, outputs,
                            in_policies, out_policies,
                            node_id, in_edge_ids, out_edge_ids,
                            clock, telemetry
                        )
                    }

                    #[inline]
                    fn with_node_and_step_context<'telemetry, 'clock, C, T, R, E>(
                        &mut self,
                        clock: &'clock C,
                        telemetry: &'telemetry mut T,
                        f: impl FnOnce(
                            &mut <Self as limen_core::graph::GraphNodeAccess<#i_const>>::Node,
                            &mut limen_core::node::StepContext<
                                '_,
                                'telemetry,
                                'clock,
                                #in_ports, #out_ports,
                                <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InP,
                                <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutP,
                                <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InQ,
                                <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutQ,
                                C, T
                            >,
                        ) -> Result<R, E>,
                    ) -> Result<R, E>
                    where
                        Self: limen_core::graph::GraphNodeAccess<#i_const>,
                        limen_core::policy::EdgePolicy: Copy,
                        C: limen_core::prelude::PlatformClock + Sized,
                        T: limen_core::prelude::Telemetry + Sized,
                    {
                        let in_policies: [limen_core::policy::EdgePolicy; #in_ports] = [ #( #in_pols ),* ];
                        let out_policies: [limen_core::policy::EdgePolicy; #out_ports] = [ #( #out_pols ),* ];

                        let inputs: [&mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InQ; #in_ports] = [ #( #input_eps ),* ];
                        let outputs: [&mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutQ; #out_ports] = [ #( #output_eps ),* ];

                        let node_id: u32 = #i_const as u32;
                        let in_edge_ids: [u32; #in_ports] = [ #( #in_ids as u32 ),* ];
                        let out_edge_ids: [u32; #out_ports] = [ #( #out_ids as u32 ),* ];

                        let mut ctx = limen_core::node::StepContext::new(
                            inputs, outputs,
                            in_policies, out_policies,
                            node_id, in_edge_ids, out_edge_ids,
                            clock, telemetry
                        );
                        f(self.nodes.#tuple_idx.as_mut().expect("node moved"), &mut ctx)
                    }
                }
            }
        }).collect()
    }

    /// Create the edge descriptor array for the concurrent graph:
    /// ingress edges first, then real edges.
    fn edge_descriptors_array_std(&self) -> TokenStream2 {
        // [ ingress..., edges... ]
        let ingress = self.ingress_nodes.iter().enumerate().map(|(k, _)| {
            let kidx = Index::from(k);
            quote! { self.ingress_edges.#kidx.descriptor() }
        });
        let reals = self.g.edges.iter().enumerate().map(|(j, _)| {
            let jidx = Index::from(j);
            quote! { self.edges.#jidx.descriptor() }
        });
        let total = self.ingress_count() + self.g.edges.len();
        if total == 0 {
            quote! { [] }
        } else {
            quote! { [ #( #ingress ),* , #( #reals ),* ] }
        }
    }

    /// Match on an edge id `E` and return its `EdgeOccupancy` using the
    /// concurrent edge links (ingress and real edges).
    fn edge_occupancy_match_std(&self) -> TokenStream2 {
        let ingress_count = self.ingress_count();

        let ingress_arms = self.ingress_nodes.iter().enumerate().map(|(k, _)| {
            let kidx = Index::from(k);
            quote! {
                #k => {
                    let e = &self.ingress_edges.#kidx;
                    Ok(e.occupancy(&e.policy()))
                }
            }
        });

        let real_arms = self.g.edges.iter().enumerate().map(|(j, _)| {
            let eid = j + ingress_count;
            let jidx = Index::from(j);
            quote! {
                #eid => {
                    let e = &self.edges.#jidx;
                    Ok(e.occupancy(&e.policy()))
                }
            }
        });

        quote! {
            let occ = match E {
                #( #ingress_arms )*
                #( #real_arms )*
                _ => Err(limen_core::errors::GraphError::InvalidEdgeIndex),
            }?;
            Ok(occ)
        }
    }

    /// Write all edge occupancies (ingress first, then real edges) into `out`.
    fn write_all_occupancies_std(&self) -> TokenStream2 {
        let total = self.ingress_count() + self.g.edges.len();
        let assigns = (0..total).map(|k| {
            quote! { out[#k] = self.edge_occupancy_for::<#k>()?; }
        });
        quote! { #( #assigns )* Ok(()) }
    }

    /// Refresh the occupancies for all edges touching a specific node `I`.
    fn refresh_for_node_std(&self) -> TokenStream2 {
        let total = self.ingress_count() + self.g.edges.len();
        let arms = (0..total).map(|k| {
            let kk = syn::Index::from(k);
            quote! { #k => { out[#k] = self.edge_occupancy_for::<#kk>()?; } }
        });
        quote! {
            let node_idx = limen_core::types::NodeIndex::from(I);
            for ed in self.get_edge_descriptors().iter() {
                if ed.upstream.node == node_idx || ed.downstream.node == node_idx {
                    let k = (ed.id).as_usize();
                    match k {
                        #( #arms )*,
                        _ => unreachable!("invalid edge index"),
                    }
                }
            }
            Ok(())
        }
    }

    /// Dispatch a step call by node index using the concurrent endpoints and
    /// `GraphNodeContextBuilder`.
    fn step_by_index_std(&self) -> TokenStream2 {
        let arms = self.g.nodes.iter().map(|n| {
            let i = n.idx;
            let in_ports = n.in_ports;
            let out_ports = n.out_ports;
            quote! {
                #i => <Self as limen_core::graph::GraphNodeContextBuilder<#i, #in_ports, #out_ports>>::with_node_and_step_context::<
                    C, T, limen_core::node::StepResult, limen_core::errors::NodeError
                >(self, clock, telemetry, |node, ctx| node.step(ctx)),
            }
        });
        quote! {
            match index {
                #( #arms )*
                _ => unreachable!("invalid node index"),
            }
        }
    }

    /// Define the `OwnedBundle` enum used by the concurrent graph.
    ///
    /// Each variant corresponds to a node index and contains the node, its
    /// input/output endpoints, and the relevant policies. Source nodes also
    /// carry a `SourceIngressUpdater` to maintain ingress occupancy.
    fn owned_bundle_enum(&self, base: &Ident) -> TokenStream2 {
        let enum_name = self.std_owned_bundle_name(base);

        let variants = self.g.nodes.iter().map(|n| {
            let i = n.idx;
            let variant = format_ident!("N{}", i);

            let in_ports = n.in_ports;
            let out_ports = n.out_ports;

            let nlink = self.node_link_type(n);
            let inq_ty = self.std_inq_ty_for_node(i);
            let outq_ty = self.std_outq_ty_for_node(i);

            let maybe_ing_field = if let Some(_k) = self.source_pos_by_node[i] {
                quote! { , ingress_updater: limen_core::node::source::probe::SourceIngressUpdater }
            } else {
                quote! {}
            };

            quote! {
                #variant {
                    node: #nlink,
                    ins: [#inq_ty; #in_ports],
                    outs: [#outq_ty; #out_ports],
                    in_policies: [limen_core::policy::EdgePolicy; #in_ports],
                    out_policies: [limen_core::policy::EdgePolicy; #out_ports]
                    #maybe_ing_field
                }
            }
        });

        quote! {
            pub enum #enum_name {
                #( #variants ),*
            }
        }
    }

    /// Implement the `GraphApi` owned-bundle operations for the concurrent
    /// graph: `take_owned_bundle_by_index`, `put_owned_bundle_by_index`, and
    /// `step_owned_bundle`. These move nodes and their endpoints out of the
    /// graph safely, run a step, then restore them (if desired) without
    /// rebuilding queue arcs.
    fn graphapi_owned_impls(&self, base: &Ident) -> TokenStream2 {
        let enum_name = self.std_owned_bundle_name(base);

        // take_owned_bundle_by_index arms
        let take_arms = self.g.nodes.iter().map(|n| {
            let i = n.idx;
            let iidx = Index::from(i);
            let variant = format_ident!("N{}", i);

            let in_ports = n.in_ports;
            let out_ports = n.out_ports;

            let in_endpoints_collect: Vec<TokenStream2> = if in_ports == 0 {
                vec![]
            } else {
                self.in_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { (self.endpoints.#pos).0.clone() }
                }).collect()
            };
            let out_endpoints_collect: Vec<TokenStream2> = if out_ports == 0 {
                vec![]
            } else {
                self.out_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { (self.endpoints.#pos).1.clone() }
                }).collect()
            };
            let in_policies_collect: Vec<TokenStream2> = if in_ports == 0 {
                vec![]
            } else {
                self.in_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { self.edges.#pos.policy() }
                }).collect()
            };
            let out_policies_collect: Vec<TokenStream2> = if out_ports == 0 {
                vec![]
            } else {
                self.out_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { self.edges.#pos.policy() }
                }).collect()
            };

            let maybe_ing_take = if let Some(k) = self.source_pos_by_node[i] {
                let kidx = Index::from(k);
                quote! {
                    , ingress_updater: self.ingress_updaters.#kidx.take().expect("ingress updater already taken")
                }
            } else {
                quote! {}
            };

            quote! {
                #i => {
                    let node = self.nodes.#iidx.take().ok_or(limen_core::errors::GraphError::InvalidEdgeIndex)?;
                    Ok(#enum_name::#variant {
                        node,
                        ins: [ #( #in_endpoints_collect ),* ],
                        outs: [ #( #out_endpoints_collect ),* ],
                        in_policies: [ #( #in_policies_collect ),* ],
                        out_policies: [ #( #out_policies_collect ),* ]
                        #maybe_ing_take
                    })
                }
            }
        });

        // put_owned_bundle_by_index arms
        let put_arms = self.g.nodes.iter().map(|n| {
            let i = n.idx;
            let iidx = Index::from(i);
            let variant = format_ident!("N{}", i);

            let in_ports = n.in_ports;
            let out_ports = n.out_ports;

            let in_assigns: Vec<TokenStream2> = if in_ports == 0 {
                vec![]
            } else {
                self.in_edges_by_node[i].iter().enumerate().map(|(k, &eidx)| {
                    let pos = Index::from(eidx);
                    let kk = Index::from(k);
                    quote! { (self.endpoints.#pos).0 = ins[#kk].clone(); }
                }).collect()
            };

            let out_assigns: Vec<TokenStream2> = if out_ports == 0 {
                vec![]
            } else {
                self.out_edges_by_node[i].iter().enumerate().map(|(k, &eidx)| {
                    let pos = Index::from(eidx);
                    let kk = Index::from(k);
                    quote! { (self.endpoints.#pos).1 = outs[#kk].clone(); }
                }).collect()
            };

            let maybe_ing_restore = if let Some(k) = self.source_pos_by_node[i] {
                let kidx = Index::from(k);
                quote! { self.ingress_updaters.#kidx = core::option::Option::Some(ingress_updater); }
            } else {
                quote! {}
            };

            // Variant pattern with optional ingress_updater
            let pat = if self.source_pos_by_node[i].is_some() {
                quote! { #enum_name::#variant { node, ins, outs, in_policies: _, out_policies: _, ingress_updater } }
            } else {
                quote! { #enum_name::#variant { node, ins, outs, in_policies: _, out_policies: _ } }
            };

            quote! {
                #pat => {
                    assert!(self.nodes.#iidx.is_none(), "node already present");
                    self.nodes.#iidx = core::option::Option::Some(node);
                    #( #in_assigns )*
                    #( #out_assigns )*
                    #maybe_ing_restore
                    Ok(())
                }
            }
        });

        // step_owned_bundle arms
        let step_arms = self.g.nodes.iter().map(|n| {
            let i = n.idx;
            let in_ports = n.in_ports;
            let out_ports = n.out_ports;
            let variant = format_ident!("N{}", i);

            let inq_ty = self.std_inq_ty_for_node(i);
            let outq_ty = self.std_outq_ty_for_node(i);

            let mut in_refs: Vec<TokenStream2> = Vec::new();
            for k in 0..in_ports {
                let kk = Index::from(k);
                in_refs.push(quote! { &mut ins[#kk] });
            }
            let mut out_refs: Vec<TokenStream2> = Vec::new();
            for k in 0..out_ports {
                let kk = Index::from(k);
                out_refs.push(quote! { &mut outs[#kk] });
            }

            let ingress_count = self.ingress_count();
            let in_ids: Vec<usize> = if in_ports == 0 { vec![] } else {
                self.in_edges_by_node[i].iter().map(|&eidx| eidx + ingress_count).collect()
            };
            let out_ids: Vec<usize> = if out_ports == 0 { vec![] } else {
                self.out_edges_by_node[i].iter().map(|&eidx| eidx + ingress_count).collect()
            };

            let maybe_ing_update = if let Some(k) = self.source_pos_by_node[i] {
                quote! {
                    let occ = node.node().source_ref().ingress_occupancy(&INGRESS_POLICIES[#k]);
                    ingress_updater.update(occ.items, occ.bytes);
                }
            } else { quote! {} };

            let pat = if self.source_pos_by_node[i].is_some() {
                quote! { #enum_name::#variant { node, ins, outs, in_policies, out_policies, ingress_updater } }
            } else {
                quote! { #enum_name::#variant { node, ins, outs, in_policies, out_policies } }
            };

            quote! {
                #pat => {
                    #maybe_ing_update
                    let inputs: [&mut #inq_ty; #in_ports] = [ #( #in_refs ),* ];
                    let outputs: [&mut #outq_ty; #out_ports] = [ #( #out_refs ),* ];
                    let node_id: u32 = #i as u32;
                    let in_edge_ids: [u32; #in_ports] = [ #( #in_ids as u32 ),* ];
                    let out_edge_ids: [u32; #out_ports] = [ #( #out_ids as u32 ),* ];
                    let mut ctx = limen_core::node::StepContext::new(
                        inputs,
                        outputs,
                        *in_policies,
                        *out_policies,
                        node_id,
                        in_edge_ids,
                        out_edge_ids,
                        clock,
                        telemetry,
                    );
                    node.step(&mut ctx)
                }
            }
        });

        quote! {
            type OwnedBundle = #enum_name;

            #[cfg(feature = "std")]
            fn take_owned_bundle_by_index(&mut self, index: usize) -> Result<Self::OwnedBundle, limen_core::errors::GraphError> {
                match index {
                    #( #take_arms ),*,
                    _ => Err(limen_core::errors::GraphError::InvalidEdgeIndex),
                }
            }

            #[cfg(feature = "std")]
            fn put_owned_bundle_by_index(&mut self, bundle: Self::OwnedBundle) -> Result<(), limen_core::errors::GraphError> {
                match bundle {
                    #( #put_arms ),*
                }
            }

            #[cfg(feature = "std")]
            #[inline]
            fn step_owned_bundle<C, T>(
                bundle: &mut Self::OwnedBundle,
                clock: &C,
                telemetry: &mut T,
            ) -> Result<limen_core::node::StepResult, limen_core::errors::NodeError>
            where
                limen_core::policy::EdgePolicy: Copy,
                C: limen_core::prelude::PlatformClock + Sized,
                T: limen_core::prelude::Telemetry + Sized,
            {
                match bundle {
                    #( #step_arms ),*
                }
            }
        }
    }

    /// Implement `GraphNodeOwnedEndpointHandoff<I, IN, OUT>` using the
    /// persistent endpoints stored in the concurrent graph. Endpoints are
    /// cloned on take and restored on put, preserving the underlying queue
    /// `Arc`s.
    fn owned_handoff_impls(&self, base: &Ident) -> Vec<TokenStream2> {
        // Implement GraphNodeOwnedEndpointHandoff<I, IN, OUT> using the persistent endpoints.
        let gname = self.std_graph_name(base);
        self.g.nodes.iter().map(|n| {
            let i = n.idx;
            let in_ports = n.in_ports;
            let out_ports = n.out_ports;

            // Values to return in take_node_and_endpoints
            let in_ret: Vec<TokenStream2> = if in_ports == 0 { vec![] } else {
                self.in_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { (self.endpoints.#pos).0.clone() }
                }).collect()
            };
            let out_ret: Vec<TokenStream2> = if out_ports == 0 { vec![] } else {
                self.out_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { (self.endpoints.#pos).1.clone() }
                }).collect()
            };
            let in_pols: Vec<TokenStream2> = if in_ports == 0 { vec![] } else {
                self.in_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { self.edges.#pos.policy() }
                }).collect()
            };
            let out_pols: Vec<TokenStream2> = if out_ports == 0 { vec![] } else {
                self.out_edges_by_node[i].iter().map(|&eidx| {
                    let pos = Index::from(eidx);
                    quote! { self.edges.#pos.policy() }
                }).collect()
            };

            // Assignments for put_node_and_endpoints (tuple slots must be addressed statically)
            let in_assigns: Vec<TokenStream2> = if in_ports == 0 { vec![] } else {
                self.in_edges_by_node[i].iter().enumerate().map(|(k, &eidx)| {
                    let pos = Index::from(eidx);
                    let kk = Index::from(k);
                    quote! { (self.endpoints.#pos).0 = inputs[#kk].clone(); }
                }).collect()
            };
            let out_assigns: Vec<TokenStream2> = if out_ports == 0 { vec![] } else {
                self.out_edges_by_node[i].iter().enumerate().map(|(k, &eidx)| {
                    let pos = Index::from(eidx);
                    let kk = Index::from(k);
                    quote! { (self.endpoints.#pos).1 = outputs[#kk].clone(); }
                }).collect()
            };

            let iidx = Index::from(i);
            let inputs_ident = if in_ports == 0 {
                format_ident!("_inputs")
            } else {
                format_ident!("inputs")
            };
            let outputs_ident = if out_ports == 0 {
                format_ident!("_outputs")
            } else {
                format_ident!("outputs")
            };

            quote! {
                impl limen_core::graph::GraphNodeOwnedEndpointHandoff<#i, #in_ports, #out_ports> for #gname {
                    type NodeOwned = <Self as limen_core::graph::GraphNodeAccess<#i>>::Node;
                    fn take_node_and_endpoints(
                        &mut self,
                    ) -> (
                        Self::NodeOwned,
                        [<Self as limen_core::graph::GraphNodeTypes<#i, #in_ports, #out_ports>>::InQ; #in_ports],
                        [<Self as limen_core::graph::GraphNodeTypes<#i, #in_ports, #out_ports>>::OutQ; #out_ports],
                        [limen_core::policy::EdgePolicy; #in_ports],
                        [limen_core::policy::EdgePolicy; #out_ports],
                    ) {
                        let node = self.nodes.#iidx.take().expect("node already taken");
                        ( node, [ #( #in_ret ),* ], [ #( #out_ret ),* ], [ #( #in_pols ),* ], [ #( #out_pols ),* ] )
                    }
                    fn put_node_and_endpoints(
                        &mut self,
                        node: Self::NodeOwned,
                        #inputs_ident: [<Self as limen_core::graph::GraphNodeTypes<#i, #in_ports, #out_ports>>::InQ; #in_ports],
                        #outputs_ident: [<Self as limen_core::graph::GraphNodeTypes<#i, #in_ports, #out_ports>>::OutQ; #out_ports],
                    ) {
                        assert!(self.nodes.#iidx.is_none(), "node already present");
                        self.nodes.#iidx = core::option::Option::Some(node);
                        #( #in_assigns )*
                        #( #out_assigns )*
                    }
                }
            }
        }).collect()
    }

    /// Emit the full **concurrent** graph module and type.
    ///
    /// Inside `#[cfg(feature = "std")] pub mod concurrent_graph` this generates:
    /// - `struct <GraphName>Std` with node storage, concurrent edges, endpoints,
    ///   ingress edges and updaters, and cached node descriptors.
    /// - `impl GraphApi<NODES, EDGES>` including owned-bundle methods.
    /// - `GraphNodeAccess`, `GraphEdgeAccess`, `GraphNodeTypes`,
    ///   `GraphNodeContextBuilder`, and `GraphNodeOwnedEndpointHandoff` impls.
    fn emit_std_graph(&self, vis: &syn::Visibility, base: &Ident) -> TokenStream2 {
        let gname = self.std_graph_name(base);
        let owned_enum = self.owned_bundle_enum(base);

        let nodes_ty = self.nodes_tuple_ty_with_option();
        let edges_ty = self.edges_tuple_ty_std();
        let endpoints_ty = self.endpoints_tuple_ty();
        let ingress_edges_ty = self.ingress_edges_tuple_ty();
        let ingress_updaters_ty = self.ingress_updaters_tuple_ty();

        let ctor_args = self.ctor_args();
        let nodes_init = self.nodes_tuple_init_with_option();
        let (edges_let_decls, edges_tuple) = self.edges_let_bindings_init();
        let endpoints_init = self.endpoints_tuple_init();
        let (ingress_decl, ingress_both) = self.ingress_edges_tuple_init();
        let node_descs_init = self.node_descs_array_cache_init();

        let node_count = self.g.nodes.len();
        let edge_count = self.ingress_count() + self.g.edges.len();

        let ingress_pols_const = self.ingress_policies_const();
        let edge_descs = self.edge_descriptors_array_std();
        let edge_occ_match = self.edge_occupancy_match_std();
        let write_all = self.write_all_occupancies_std();
        let refresh_one = self.refresh_for_node_std();
        let step_match = self.step_by_index_std();

        let node_access = self.graph_node_access_impls(&gname);
        let edge_access = self.graph_edge_access_impls(&gname);
        let node_types = self.graph_node_types_impls_std(&gname);
        let node_ctx = self.graph_node_ctx_impls_std(&gname);

        // GraphNodeOwnedEndpointHandoff impls
        let handoffs = self.owned_handoff_impls(base);

        // GraphApi owned-bundle impls (take/put/step)
        let owned_impls = self.graphapi_owned_impls(base);

        quote! {
            #[cfg(feature = "std")]
            pub mod concurrent_graph {
                use super::*;

                #ingress_pols_const

                /// Concurrent (std) graph flavor for this pipeline.
                ///
                /// This variant stores real edges as lock-free SPSC queues,
                /// exposes external ingress to source nodes via probe links,
                /// and supports moving nodes and their endpoints out as an
                /// owned bundle for concurrent execution.
                #[allow(clippy::complexity)]
                #vis struct #gname {
                    /// Node storage as `Option<NodeLink<..>>` to allow owned-bundle handoff.
                    nodes: #nodes_ty,
                    /// Concurrent SPSC edge links for all *real* edges.
                    edges: #edges_ty,
                    /// Persistent `(ConsumerEndpoint, ProducerEndpoint)` pairs cloned from each edge.
                    endpoints: #endpoints_ty,
                    /// Probe-based ingress links for source nodes (heterogeneous payloads).
                    ingress_edges: #ingress_edges_ty,
                    /// Updaters that push ingress occupancy into the probe links.
                    ingress_updaters: #ingress_updaters_ty,
                    /// Cached node descriptors (valid even when nodes are moved out).
                    node_descs: [limen_core::node::link::NodeDescriptor; #node_count],
                }

                impl #gname {
                    /// Construct a new concurrent graph instance.
                    ///
                    /// # Parameters
                    /// - One node parameter per node: `node_<idx> : impl Into<EffectiveNodeType>`.
                    /// - One queue parameter per *real* edge (offset by ingress):
                    ///   `q_<edge_id> : QueueType`.
                    ///
                    /// This builds concurrent edges, per-edge endpoints, probe-based
                    /// ingress edges for sources, and caches node descriptors.
                    #[inline]
                    pub fn new( #ctor_args ) -> Self {
                        // build nodes
                        let nodes = #nodes_init;

                        // build edges (let-bindings), tuple, endpoints from arc
                        #edges_let_decls
                        let endpoints: #endpoints_ty = #endpoints_init;
                        let edges: #edges_ty = #edges_tuple;

                        // ingress probes and updaters
                        #ingress_decl
                        let (ingress_edges, ingress_updaters) = #ingress_both;

                        // cache descriptors (works even if nodes are moved out later)
                        let node_descs: [limen_core::node::link::NodeDescriptor; #node_count] = #node_descs_init;

                        Self {
                            nodes,
                            edges,
                            endpoints,
                            ingress_edges,
                            ingress_updaters,
                            node_descs,
                        }
                    }
                }

                /// Owned-bundle for concurrent execution: one variant per node,
                /// including its endpoints and policies (plus ingress updater for sources).
                #owned_enum

                impl limen_core::graph::GraphApi<#node_count, #edge_count> for #gname {
                    #[inline]
                    fn get_node_descriptors(&self) -> [limen_core::node::link::NodeDescriptor; #node_count] {
                        self.node_descs.clone()
                    }
                    #[inline]
                    fn get_edge_descriptors(&self) -> [limen_core::edge::link::EdgeDescriptor; #edge_count] {
                        #edge_descs
                    }
                    #[inline]
                    fn edge_occupancy_for<const E: usize>(&self) -> Result<limen_core::edge::EdgeOccupancy, limen_core::errors::GraphError> {
                        #edge_occ_match
                    }
                    #[inline]
                    fn write_all_edge_occupancies(&self, out: &mut [limen_core::edge::EdgeOccupancy; #edge_count]) -> Result<(), limen_core::errors::GraphError> {
                        #write_all
                    }
                    #[inline]
                    fn refresh_occupancies_for_node<const I: usize, const IN: usize, const OUT: usize>(
                        &self,
                        out: &mut [limen_core::edge::EdgeOccupancy; #edge_count],
                    ) -> Result<(), limen_core::errors::GraphError> {
                        #refresh_one
                    }
                    #[inline]
                    fn step_node_by_index<C, T>(
                        &mut self,
                        index: usize,
                        clock: &C,
                        telemetry: &mut T,
                    ) -> Result<limen_core::node::StepResult, limen_core::errors::NodeError>
                    where
                        limen_core::policy::EdgePolicy: Copy,
                        C: limen_core::prelude::PlatformClock + Sized,
                        T: limen_core::prelude::Telemetry + Sized,
                    {
                        #step_match
                    }

                    #owned_impls
                }

                #(#node_access)*
                #(#edge_access)*
                #(#node_types)*
                #(#node_ctx)*
                #(#handoffs)*
            }
        }
    }
}
