//! Code generation for Limen graphs.
//!
//! This module turns a validated parsed [`GraphDef`] into a `TokenStream`
//! containing a concrete graph type that implements `limen_core::graph::GraphApi`.
//!
//! # What this generator emits
//!
//! Exactly one concrete graph type per invocation.
//!
//! ## When `emit_concurrent = false` (default) — non-`std` graph
//!
//! - A concrete `struct <GraphName>` containing:
//!   - `nodes`: a tuple of `NodeLink<...>` (one entry per declared node).
//!   - `edges`: a tuple of `EdgeLink<...>` (one entry per declared *real* edge).
//!   - `managers`: a tuple of memory manager instances (one per declared *real* edge).
//! - An inherent `new(..)` constructor with arguments:
//!   - One `impl Into<NodeType>` parameter per node: `node_<idx>`.
//!   - One queue parameter per *real* edge: `q_<edge_id>`, where `edge_id` is
//!     offset by the ingress count (see below).
//!   - One memory manager per *real* edge: `mgr_<edge_id>`, with the same offset.
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
//! When `GraphDef.emit_concurrent` is `true`, codegen additionally emits
//! a `#[cfg(feature = "std")] impl ScopedGraphApi<...> for <GraphName>`,
//! allowing the same graph type to run via scoped worker threads.
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
/// Emits a single graph struct named `<Name>` at module root.
///
/// When `g.emit_concurrent` is `true`, the generated code also includes
/// a `#[cfg(feature = "std")]` `ScopedGraphApi` implementation with
/// `run_scoped()`. Edge and memory manager types then need to satisfy
/// `ScopedEdge` and `ScopedManager<_>` respectively.
pub fn emit(g: &GraphDef) -> TokenStream2 {
    let vis = &g.vis;
    let name = &g.name;
    let ns = NonStd::new(g);
    ns.emit_nonstd_graph(vis, name)
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
        for n in &g.nodes {
            if let Some(_pol) = &n.ingress_policy_opt {
                ingress_nodes.push(n.idx);
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
        quote! { limen_core::edge::link::EdgeLink<#q> }
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
            let q_ident = format_ident!("q_{}", id);

            quote! {
                limen_core::edge::link::EdgeLink::<#ety>::new(
                    #q_ident,
                    limen_core::types::EdgeIndex::from(#id as usize),
                    limen_core::types::PortId::new(
                        limen_core::types::NodeIndex::from(#up as usize),
                        limen_core::types::PortIndex::from(#up_p),
                    ),
                    limen_core::types::PortId::new(
                        limen_core::types::NodeIndex::from(#dn as usize),
                        limen_core::types::PortIndex::from(#dn_p),
                    ),
                    #pol,
                    #name_opt
                )
            }
        });
        quote! { ( #( #parts ),* ) }
    }

    /// The tuple type that holds all memory managers (one per *real* edge).
    fn manager_tuple_type(&self) -> TokenStream2 {
        let parts = self.g.edges.iter().map(|e| {
            let m = &e.manager_ty;
            quote! { #m }
        });
        quote! { ( #( #parts ),* ) }
    }

    /// Construct the manager tuple from constructor arguments.
    fn manager_tuple_init(&self) -> TokenStream2 {
        let ingress_count = self.ingress_count();
        let parts = self.g.edges.iter().map(|e| {
            let id = e.idx + ingress_count;
            let mgr_ident = format_ident!("mgr_{}", id);
            quote! { #mgr_ident }
        });
        quote! { ( #( #parts ),* ) }
    }

    /// Resolve the memory manager type for a node's input side.
    ///
    /// - If the node has zero input ports, returns a dummy
    ///   `StaticMemoryManager<InP, 1>` (never instantiated).
    /// - Otherwise, returns the `manager_ty` from the first inbound edge
    ///   (uniformity is validated earlier).
    fn in_manager_ty(&self, n: &NodeDef) -> TokenStream2 {
        let in_p = &n.in_payload;
        if n.in_ports == 0 {
            quote! { limen_core::memory::static_manager::StaticMemoryManager<#in_p, 1> }
        } else {
            let e0 = self.in_edges_by_node[n.idx][0];
            let m = &self.g.edges[e0].manager_ty;
            quote! { #m }
        }
    }

    /// Resolve the memory manager type for a node's output side.
    ///
    /// - If the node has zero output ports, returns a dummy
    ///   `StaticMemoryManager<OutP, 1>` (never instantiated).
    /// - Otherwise, returns the `manager_ty` from the first outbound edge
    ///   (uniformity is validated earlier).
    fn out_manager_ty(&self, n: &NodeDef) -> TokenStream2 {
        let out_p = &n.out_payload;
        if n.out_ports == 0 {
            quote! { limen_core::memory::static_manager::StaticMemoryManager<#out_p, 1> }
        } else {
            let e0 = self.out_edges_by_node[n.idx][0];
            let m = &self.g.edges[e0].manager_ty;
            quote! { #m }
        }
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
        let mgr_args = self.g.edges.iter().map(|e| {
            let id = e.idx + ingress_count;
            let m = &e.manager_ty;
            let mgr_ident = format_ident!("mgr_{}", id);
            quote! { #mgr_ident : #m }
        });
        let args: Vec<TokenStream2> = node_args.chain(edge_args).chain(mgr_args).collect();
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
                limen_core::edge::link::EdgeDescriptor::new(
                    limen_core::types::EdgeIndex::from(#k as usize),
                    limen_core::types::PortId::new(
                        limen_core::node::source::EXTERNAL_INGRESS_NODE,
                        limen_core::types::PortIndex::from(0),
                    ),
                    limen_core::types::PortId::new(
                        limen_core::types::NodeIndex::from(#dn as usize),
                        limen_core::types::PortIndex::from(0),
                    ),
                    Some(#name),
                )
            }
        });

        let reals = self.g.edges.iter().enumerate().map(|(j, _)| {
            let jidx = Index::from(j);
            quote! { self.edges.#jidx.descriptor() }
        });

        quote! { [ #( #ingress ),*, #( #reals ),* ] }
    }

    /// Create the node policy array (`[NodePolicy; NODES]`).
    fn node_policies_array(&self) -> TokenStream2 {
        let elems = self.g.nodes.iter().enumerate().map(|(i, _)| {
            let idx = Index::from(i);
            quote! { self.nodes.#idx.policy() }
        });
        quote! { [ #( #elems ),* ] }
    }

    /// Create the edge policy array (`[EdgePolicy; EDGES]`), with ingress
    /// policies first followed by real edge policies.
    fn edge_policies_array(&self) -> TokenStream2 {
        let ingress = self.ingress_nodes.iter().enumerate().map(|(k, &node_idx)| {
            let _kidx = Index::from(k);
            let npos = Index::from(node_idx);
            // Query the source's ingress_policy at runtime.
            quote! { self.nodes.#npos.node().source_ref().ingress_policy() }
        });

        let reals = self.g.edges.iter().enumerate().map(|(j, _)| {
            let jidx = Index::from(j);
            quote! { *self.edges.#jidx.policy() }
        });

        let total = self.ingress_count() + self.g.edges.len();
        if total == 0 {
            quote! { [] }
        } else {
            quote! { [ #( #ingress ),*, #( #reals ),* ] }
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
                    Ok(src.ingress_occupancy())
                }
            }
        });

        let real_arms = self.g.edges.iter().enumerate().map(|(j, _)| {
            let eid = j + ingress_count;
            let jidx = Index::from(j);
            quote! {
                #eid => {
                    let e = &self.edges.#jidx;
                    let pol = *e.policy();
                    Ok(e.occupancy(&pol))
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
                if *ed.upstream().node() == node_idx || *ed.downstream().node() == node_idx {
                    let k = ed.id().as_usize();
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
                    quote! { limen_core::edge::NoQueue }
                } else {
                    let e0 = self.in_edges_by_node[i][0];
                    let ety = &self.g.edges[e0].ty;
                    quote! { #ety }
                };
                let outq_ty = if out_ports == 0 {
                    quote! { limen_core::edge::NoQueue }
                } else {
                    let e0 = self.out_edges_by_node[i][0];
                    let ety = &self.g.edges[e0].ty;
                    quote! { #ety }
                };

                let in_m_ty = self.in_manager_ty(n);
                let out_m_ty = self.out_manager_ty(n);

                quote! {
                    impl limen_core::graph::GraphNodeTypes<#i, #in_ports, #out_ports> for #name {
                        type InP = #in_p;
                        type OutP = #out_p;
                        type InQ = #inq_ty;
                        type OutQ = #outq_ty;
                        type InM = #in_m_ty;
                        type OutM = #out_m_ty;
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

                let in_mgrs: Vec<TokenStream2> = if in_ports == 0 {
                    vec![]
                } else {
                    self.in_edges_by_node[i]
                        .iter()
                        .map(|&eidx| {
                            let pos = Index::from(eidx);
                            quote! { &mut self.managers.#pos }
                        })
                        .collect()
                    };

                let out_mgrs: Vec<TokenStream2> = if out_ports == 0 {
                    vec![]
                } else {
                    self.out_edges_by_node[i]
                        .iter()
                        .map(|&eidx| {
                            let pos = Index::from(eidx);
                            quote! { &mut self.managers.#pos }
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
                            quote! { *self.edges.#pos.policy() }
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
                            quote! { *self.edges.#pos.policy() }
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
                            <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InM,
                            <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutM,
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

                            let in_managers: [&'graph mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InM; #in_ports] = [ #( #in_mgrs ),* ];
                            let out_managers: [&'graph mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutM; #out_ports] = [ #( #out_mgrs ),* ];

                            let node_id: u32 = #i_const as u32;
                            let in_edge_ids: [u32; #in_ports] = [ #( #in_ids as u32 ),* ];
                            let out_edge_ids: [u32; #out_ports] = [ #( #out_ids as u32 ),* ];

                            limen_core::node::StepContext::new(
                                inputs, outputs,
                                in_managers, out_managers,
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
                                    <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InM,
                                    <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutM,
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

                            let in_managers: [&mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::InM; #in_ports] = [ #( #in_mgrs ),* ];
                            let out_managers: [&mut <Self as limen_core::graph::GraphNodeTypes<#i_const, #in_ports, #out_ports>>::OutM; #out_ports] = [ #( #out_mgrs ),* ];

                            let node_id: u32 = #i_const as u32;
                            let in_edge_ids: [u32; #in_ports] = [ #( #in_ids as u32 ),* ];
                            let out_edge_ids: [u32; #out_ports] = [ #( #out_ids as u32 ),* ];

                            let mut ctx = limen_core::node::StepContext::new(
                                inputs, outputs,
                                in_managers, out_managers,
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

    /// Emit a `#[cfg(feature = "std")]` `run_scoped()` method on the graph.
    ///
    /// This method executes every node in a separate `std::thread::scope` worker
    /// (for Send nodes) or inline (for non-Send nodes). Workers share edge and
    /// manager handles via `Clone` (Arc-based), and each gets its own telemetry
    /// clone. The `should_stop` callback is polled once per step.
    ///
    /// Compile-time `Send` assertions are emitted for each spawned node; if a
    /// node type is not `Send`, the user gets a clear error pointing to that type.
    fn emit_run_scoped(&self, name: &Ident) -> TokenStream2 {
        let ingress_count = self.ingress_count();
        let node_count = self.g.nodes.len();
        let edge_count = ingress_count + self.g.edges.len();

        // --- Step 1: edge policy copies (before node borrows; EdgePolicy: Copy)
        let pol_decls: Vec<TokenStream2> = self
            .g
            .edges
            .iter()
            .enumerate()
            .map(|(j, _)| {
                let j_idx = Index::from(j);
                let pol_var = format_ident!("pol_{}", j);
                quote! { let #pol_var = *self.edges.#j_idx.policy(); }
            })
            .collect();

        // --- Step 2: per-node scoped edge handles + manager handles (before
        //     node borrows). Uses ScopedEdge / ScopedManager instead of Clone
        //     so future lock-free, non-Clone types work transparently.
        let mut handle_decls: Vec<TokenStream2> = Vec::new();
        for n in &self.g.nodes {
            let i = n.idx;
            for (port, &eidx) in self.in_edges_by_node[i].iter().enumerate() {
                let j_idx = Index::from(eidx);
                let q_var = format_ident!("in_e_{}_{}", i, port);
                let m_var = format_ident!("in_m_{}_{}", i, port);
                handle_decls.push(quote! {
                    let #q_var = limen_core::edge::ScopedEdge::scoped_handle(
                        self.edges.#j_idx.queue(),
                        limen_core::edge::EdgeHandleKind::Consumer,
                    );
                    let #m_var = limen_core::memory::manager::ScopedManager::scoped_handle(
                        &self.managers.#j_idx,
                    );
                });
            }
            for (port, &eidx) in self.out_edges_by_node[i].iter().enumerate() {
                let j_idx = Index::from(eidx);
                let q_var = format_ident!("out_e_{}_{}", i, port);
                let m_var = format_ident!("out_m_{}_{}", i, port);
                handle_decls.push(quote! {
                    let #q_var = limen_core::edge::ScopedEdge::scoped_handle(
                        self.edges.#j_idx.queue(),
                        limen_core::edge::EdgeHandleKind::Producer,
                    );
                    let #m_var = limen_core::memory::manager::ScopedManager::scoped_handle(
                        &self.managers.#j_idx,
                    );
                });
            }
        }

        // --- Step 3: per-worker telemetry clones
        let telem_decls: Vec<TokenStream2> = (0..node_count)
            .map(|i| {
                let tvar = format_ident!("telem_{}", i);
                if i + 1 < node_count {
                    quote! { let #tvar = telemetry.clone(); }
                } else {
                    quote! { let #tvar = telemetry; }
                }
            })
            .collect();

        // --- Step 4: disjoint node borrows
        let node_borrows: Vec<TokenStream2> = self
            .g
            .nodes
            .iter()
            .map(|n| {
                let i = n.idx;
                let i_idx = Index::from(i);
                let nvar = format_ident!("n{}", i);
                quote! { let #nvar = &mut self.nodes.#i_idx; }
            })
            .collect();

        // --- Step 5: per-node worker closures (scheduler-driven)
        let workers: Vec<TokenStream2> = self
            .g
            .nodes
            .iter()
            .map(|n| {
                let i = n.idx;
                let in_ports = n.in_ports;
                let out_ports = n.out_ports;
                let in_edges = &self.in_edges_by_node[i];
                let out_edges = &self.out_edges_by_node[i];
                let node_ty = &n.ty;
                let nvar = format_ident!("n{}", i);
                let tvar = format_ident!("telem_{}", i);
                let node_id = i as u32;

                // Local bindings moved into closure
                let in_q_vars: Vec<_> = (0..in_ports)
                    .map(|p| format_ident!("in_e_{}_{}", i, p))
                    .collect();
                let in_m_vars: Vec<_> = (0..in_ports)
                    .map(|p| format_ident!("in_m_{}_{}", i, p))
                    .collect();
                let out_q_vars: Vec<_> = (0..out_ports)
                    .map(|p| format_ident!("out_e_{}_{}", i, p))
                    .collect();
                let out_m_vars: Vec<_> = (0..out_ports)
                    .map(|p| format_ident!("out_m_{}_{}", i, p))
                    .collect();

                // Mutable rebindings inside closure (for &mut refs)
                let in_mut_binds: Vec<_> = in_q_vars
                    .iter()
                    .zip(&in_m_vars)
                    .map(|(q, m)| quote! { let mut #q = #q; let mut #m = #m; })
                    .collect();
                let out_mut_binds: Vec<_> = out_q_vars
                    .iter()
                    .zip(&out_m_vars)
                    .map(|(q, m)| quote! { let mut #q = #q; let mut #m = #m; })
                    .collect();

                // References for StepContext arrays
                let in_q_refs: Vec<_> = in_q_vars.iter().map(|v| quote! { &mut #v }).collect();
                let out_q_refs: Vec<_> = out_q_vars.iter().map(|v| quote! { &mut #v }).collect();
                let in_m_refs: Vec<_> = in_m_vars.iter().map(|v| quote! { &mut #v }).collect();
                let out_m_refs: Vec<_> = out_m_vars.iter().map(|v| quote! { &mut #v }).collect();

                // Policy variables (Copy — captured by copy in move closure)
                let in_pol_vars: Vec<_> = in_edges
                    .iter()
                    .map(|&eidx| format_ident!("pol_{}", eidx))
                    .collect();
                let out_pol_vars: Vec<_> = out_edges
                    .iter()
                    .map(|&eidx| format_ident!("pol_{}", eidx))
                    .collect();

                // Edge global IDs
                let in_ids: Vec<u32> = in_edges
                    .iter()
                    .map(|&eidx| (eidx + ingress_count) as u32)
                    .collect();
                let out_ids: Vec<u32> = out_edges
                    .iter()
                    .map(|&eidx| (eidx + ingress_count) as u32)
                    .collect();

                // Typed empty arrays for input/output sides with zero ports.
                // When IN=0 or OUT=0 the compiler can't infer the queue/manager
                // type from an empty `[]`, so we emit explicit type annotations.
                let inq_ty = if in_ports == 0 {
                    quote! { limen_core::edge::NoQueue }
                } else {
                    let e0 = self.in_edges_by_node[i][0];
                    let ety = &self.g.edges[e0].ty;
                    quote! { <#ety as limen_core::edge::ScopedEdge>::Handle<'_> }
                };
                let outq_ty = if out_ports == 0 {
                    quote! { limen_core::edge::NoQueue }
                } else {
                    let e0 = self.out_edges_by_node[i][0];
                    let ety = &self.g.edges[e0].ty;
                    quote! { <#ety as limen_core::edge::ScopedEdge>::Handle<'_> }
                };
                let in_m_ty = if in_ports == 0 {
                    self.in_manager_ty(n)
                } else {
                    let e0 = self.in_edges_by_node[i][0];
                    let mty = &self.g.edges[e0].manager_ty;
                    let payload = &self.g.edges[e0].payload;
                    quote! { <#mty as limen_core::memory::manager::ScopedManager<#payload>>::Handle<'_> }
                };
                let out_m_ty = if out_ports == 0 {
                    self.out_manager_ty(n)
                } else {
                    let e0 = self.out_edges_by_node[i][0];
                    let mty = &self.g.edges[e0].manager_ty;
                    let payload = &self.g.edges[e0].payload;
                    quote! { <#mty as limen_core::memory::manager::ScopedManager<#payload>>::Handle<'_> }
                };

                let in_q_array = if in_ports == 0 {
                    quote! { [] as [&mut #inq_ty; 0] }
                } else {
                    quote! { [ #( #in_q_refs ),* ] }
                };
                let out_q_array = if out_ports == 0 {
                    quote! { [] as [&mut #outq_ty; 0] }
                } else {
                    quote! { [ #( #out_q_refs ),* ] }
                };
                let in_m_array = if in_ports == 0 {
                    quote! { [] as [&mut #in_m_ty; 0] }
                } else {
                    quote! { [ #( #in_m_refs ),* ] }
                };
                let out_m_array = if out_ports == 0 {
                    quote! { [] as [&mut #out_m_ty; 0] }
                } else {
                    quote! { [ #( #out_m_refs ),* ] }
                };

                // --- Occupancy query expressions (concrete types, no dyn) ---
                let in_occ_items: Vec<TokenStream2> = in_q_vars
                    .iter()
                    .zip(&in_pol_vars)
                    .map(|(v, p)| quote! { *limen_core::edge::Edge::occupancy(&#v, &#p).items() })
                    .collect();
                // Output occupancy + watermark queries
                let out_occ_exprs: Vec<TokenStream2> = out_q_vars
                    .iter()
                    .zip(&out_pol_vars)
                    .map(|(v, p)| quote! { limen_core::edge::Edge::occupancy(&#v, &#p) })
                    .collect();

                // Source nodes (with ingress_policy) use ingress_occupancy()
                // for readiness instead of input edge occupancy (they have none).
                let is_ingress_node = self.ingress_nodes.contains(&i);
                let input_avail_expr = if is_ingress_node {
                    quote! {
                        let _ingress_occ = #nvar.node().source_ref().ingress_occupancy();
                        let _any_input = *_ingress_occ.items() > 0;
                    }
                } else {
                    match in_occ_items.len() {
                        0 => quote! {
                            let _any_input = false;
                        },
                        1 => {
                            let only_in_occ = &in_occ_items[0];
                            quote! {
                                let _any_input = #only_in_occ > 0;
                            }
                        }
                        _ => quote! {
                            let _any_input = #( (#in_occ_items > 0) )||*;
                        },
                    }
                };

                let node_count_lit = node_count;

                quote! {
                    // Compile-time Send assertion — error here means #node_ty is not Send.
                    {
                        fn _assert_send<_T: core::marker::Send>() {}
                        _assert_send::<#node_ty>();
                    }
                    scope.spawn(move || {
                        #( #in_mut_binds )*
                        #( #out_mut_binds )*
                        let mut telem = #tvar;

                        let mut state = limen_core::scheduling::WorkerState::new(
                            #i,
                            #node_count_lit,
                            clock_ref.now_ticks(),
                        );

                        loop {
                            // --- Refresh WorkerState from concrete edge types ---
                            state.current_tick = clock_ref.now_ticks();

                            // Compute readiness: any input available?
                            #input_avail_expr

                            // Compute max output backpressure
                            let mut _max_wm = limen_core::policy::WatermarkState::BelowSoft;
                            #(
                                {
                                    let _occ = #out_occ_exprs;
                                    if *_occ.watermark() > _max_wm {
                                        _max_wm = *_occ.watermark();
                                    }
                                }
                            )*
                            state.backpressure = _max_wm;

                            // Readiness contract (from scheduling.rs):
                            // - all inputs empty → NotReady
                            // - any input non-empty AND any output ≥ soft → ReadyUnderPressure
                            // - any input non-empty AND all below soft → Ready
                            state.readiness = if !_any_input {
                                limen_core::scheduling::Readiness::NotReady
                            } else if _max_wm >= limen_core::policy::WatermarkState::BetweenSoftAndHard {
                                limen_core::scheduling::Readiness::ReadyUnderPressure
                            } else {
                                limen_core::scheduling::Readiness::Ready
                            };

                            // --- Ask scheduler ---
                            match sched_ref.decide(&state) {
                                limen_core::scheduling::WorkerDecision::Step => {
                                    let mut ctx = limen_core::node::StepContext::new(
                                          #in_q_array,
                                          #out_q_array,
                                          #in_m_array,
                                          #out_m_array,
                                          [ #( #in_pol_vars ),* ],
                                          [ #( #out_pol_vars ),* ],
                                          #node_id as u32,
                                          [ #( #in_ids as u32 ),* ],
                                          [ #( #out_ids as u32 ),* ],
                                          clock_ref,
                                          &mut telem,
                                      );
                                    match #nvar.step(&mut ctx) {
                                        Ok(sr) => {
                                            state.last_step = Some(sr);
                                            state.last_error = false;
                                        }
                                        Err(_e) => {
                                            state.last_step = None;
                                            state.last_error = true;
                                        }
                                    }
                                }
                                limen_core::scheduling::WorkerDecision::WaitMicros(d) => {
                                    ::std::thread::sleep(
                                        ::std::time::Duration::from_micros(d),
                                    );
                                    state.last_step = None;
                                    state.last_error = false;
                                }
                                limen_core::scheduling::WorkerDecision::Stop => break,
                                _ => break, // forward-compat for future variants
                            }
                        }
                    });
                }
            })
            .collect();

        let nc = node_count;
        let ec = edge_count;

        // Collect unique where bounds for ScopedEdge + ScopedManager.
        // Each unique edge type needs: `EdgeTy: ScopedEdge`
        // Each unique (manager_ty, payload) needs: `MgrTy: ScopedManager<Payload>`
        let mut seen_edge_tys: Vec<String> = Vec::new();
        let mut seen_mgr_tys: Vec<String> = Vec::new();
        let mut where_bounds: Vec<TokenStream2> = Vec::new();

        for e in &self.g.edges {
            let ety = &e.ty;
            let ety_str = quote!(#ety).to_string();
            if !seen_edge_tys.contains(&ety_str) {
                seen_edge_tys.push(ety_str);
                where_bounds.push(quote! {
                    #ety: limen_core::edge::ScopedEdge,
                });
            }

            let mty = &e.manager_ty;
            let payload = &e.payload;
            let mgr_key = format!("{}:{}", quote!(#mty), quote!(#payload));
            if !seen_mgr_tys.contains(&mgr_key) {
                seen_mgr_tys.push(mgr_key);
                where_bounds.push(quote! {
                    #mty: limen_core::memory::manager::ScopedManager<#payload>,
                });
            }
        }

        quote! {
            #[cfg(feature = "std")]
            impl limen_core::graph::ScopedGraphApi<#nc, #ec> for #name
            where
                #( #where_bounds )*
            {
                /// Execute all nodes concurrently via scoped threads.
                ///
                /// Each worker is controlled by the provided [`WorkerScheduler`].
                /// Edge occupancy is queried per-step from concrete types (no dyn dispatch)
                /// and fed to the scheduler via [`WorkerState`].
                ///
                /// All node types must implement `Send`. If any node type is not `Send`,
                /// a compile-time assertion in the method body will produce a clear error.
                fn run_scoped<C, T, S>(&mut self, clock: C, telemetry: T, scheduler: S)
                where
                    C: limen_core::platform::PlatformClock + Clone + Send + Sync + 'static,
                    T: limen_core::telemetry::Telemetry + Clone + Send + 'static,
                    S: limen_core::scheduling::WorkerScheduler + 'static,
                {
                    #( #pol_decls )*
                    #( #handle_decls )*
                    #( #telem_decls )*
                    #( #node_borrows )*
                    let clock_ref = &clock;
                    let sched_ref = &scheduler;
                    ::std::thread::scope(|scope| {
                        #( #workers )*
                    });
                }
            }
        }
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
        let manager_tuple_ty = self.manager_tuple_type();
        let ctor_args = self.ctor_args();
        let node_tuple_init = self.node_tuple_init();
        let edge_tuple_init = self.edge_tuple_init();
        let manager_tuple_init = self.manager_tuple_init();

        let node_count = self.g.nodes.len();
        let edge_count = self.ingress_count() + self.g.edges.len();

        let node_descs = self.node_desc_array();
        let edge_descs = self.edge_desc_array();

        let node_policies = self.node_policies_array();
        let edge_policies = self.edge_policies_array();

        let edge_occ_match = self.edge_occupancy_match();
        let write_all = self.write_all_occupancies();
        let refresh_one = self.refresh_for_node();
        let step_match = self.step_by_index();

        let node_access = self.graph_node_access_impls(name);
        let edge_access = self.graph_edge_access_impls(name);
        let node_types = self.graph_node_types_impls(name);
        let node_ctx = self.graph_node_ctx_impls(name);

        let run_scoped_impl = if self.g.emit_concurrent {
            self.emit_run_scoped(name)
        } else {
            quote! {}
        };

        quote! {
            /// Non-std (embedded) graph flavor for this pipeline.
            ///
            /// This variant stores edges as plain `EdgeLink<Q>` using the queue
            /// types specified in the DSL, avoids allocating concurrent endpoints,
            /// and is suitable for `no_std` targets. Ingress edges are implicit
            /// (one per source node) and are not stored; their occupancy is obtained
            /// from the owning source node at runtime.
            #[allow(clippy::complexity)]
            #[allow(dead_code)]
            #vis struct #name {
                /// Node links for all nodes in declaration order.
                nodes: #node_tuple_ty,
                /// Edge links for all *real* edges in declaration order.
                ///
                /// Note: implicit ingress edges are **not** stored here; they are
                /// synthesized for descriptors and occupancy queries.
                edges: #edge_tuple_ty,
                /// Memory managers for all *real* edges in declaration order.
                managers: #manager_tuple_ty,
            }

            impl #name {
                /// Construct a new **non-std** graph instance.
                ///
                /// # Parameters
                /// - One node parameter per node: `node_<idx> : impl Into<EffectiveNodeType>`.
                /// - One queue parameter per *real* edge (offset by ingress):
                ///   `q_<edge_id> : QueueType`.
                /// - One memory manager per *real* edge (offset by ingress):
                ///   `mgr_<edge_id> : ManagerType`.
                ///
                /// This builds `NodeLink` and `EdgeLink` values. Implicit ingress
                /// edges (for sources) are not stored; their occupancy is computed
                /// on demand via the source node.
                #[inline]
                #[allow(dead_code)]
                pub fn new( #ctor_args ) -> Self {
                    let nodes = #node_tuple_init;
                    let edges = #edge_tuple_init;
                    let managers = #manager_tuple_init;
                    Self { nodes, edges, managers }
                }
            }

            #run_scoped_impl

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
                fn get_node_policies(&self) -> [limen_core::policy::NodePolicy; #node_count] {
                    #node_policies
                }
                #[inline]
                fn get_edge_policies(&self) -> [limen_core::policy::EdgePolicy; #edge_count] {
                    #edge_policies
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
            }

            #(#node_access)*
            #(#edge_access)*
            #(#node_types)*
            #(#node_ctx)*
        }
    }
}
