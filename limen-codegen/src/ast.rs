//! Abstract Syntax Tree (AST) for the Limen graph DSL.
//!
//! This module defines the in-memory representation produced by the parser
//! (`parse` module) and consumed by the validator (`validate` module) and the
//! code emitter (`gen` module).
//!
//! ## Notes
//! - Indices (`idx`, `from_node`, `to_node`, etc.) are parsed as raw numbers.
//!   Structural constraints (contiguous ranges, port bounds, payload agreement,
//!   queue uniformity) are enforced in `validate`, not here.
//! - `syn::TypePath`, `syn::Type`, and `syn::Expr` allow the DSL to embed
//!   Rust paths, types, and expressions verbatim (for node/edge types,
//!   payload types, and policies).

use syn::{Expr, Ident, Type, TypePath, Visibility};

/// A complete graph definition: visibility, type name, nodes, and edges.
pub struct GraphDef {
    /// Visibility for the generated graph type (e.g., `pub`).
    pub vis: Visibility,
    /// Name of the generated graph struct (e.g., `MyGraph`).
    pub name: Ident,
    /// Whether to emit the std-only scoped execution API (`ScopedGraphApi`)
    /// for this graph. The graph structure itself is unchanged.
    pub emit_concurrent: bool,
    /// All node declarations for this graph, indexed by `NodeDef::idx`.
    pub nodes: Vec<NodeDef>,
    /// All edge declarations for this graph, indexed by `EdgeDef::idx`.
    pub edges: Vec<EdgeDef>,
}

/// A single node declaration.
///
/// The node describes its concrete implementation type and its I/O shape.
/// Optional metadata includes a human-readable name and an ingress policy.
/// The latter creates a *synthetic* ingress edge for source nodes
/// (`in_ports == 0 && out_ports > 0`), consumed by the generator.
pub struct NodeDef {
    /// Node index in the range `0..nodes.len()` (validated later).
    pub idx: usize,
    /// Concrete node type path (e.g., `my_crate::nodes::MyMapNode`).
    pub ty: TypePath,
    /// Number of input ports for this node (constant).
    pub in_ports: usize,
    /// Number of output ports for this node (constant).
    pub out_ports: usize,
    /// Payload type received on each input port.
    pub in_payload: Type,
    /// Payload type emitted on each output port.
    pub out_payload: Type,
    /// Optional human-friendly node name (e.g., `Some("map")`).
    pub name_opt: Option<Expr>,
    /// Optional ingress policy expression. When present on a **source** node,
    /// a synthetic ingress edge is generated to expose external occupancy.
    pub ingress_policy_opt: Option<Expr>,
}

/// A single edge declaration.
///
/// Each edge binds one upstream node’s output port to one downstream node’s
/// input port, specifying the queue type, payload type, and policy.
///
/// The generator also creates **synthetic** ingress edges (not represented by
/// `EdgeDef`) for nodes that declare `ingress_policy_opt`; those are handled
/// internally during code emission.
pub struct EdgeDef {
    /// Edge index in the range `0..edges.len()` (validated later).
    pub idx: usize,
    /// Queue implementation type (e.g., `limen_core::edge::spsc_array::StaticRing<8>`).
    pub ty: TypePath,
    /// Payload type carried on this edge; must match the connected nodes’
    /// `out_payload` and `in_payload` (validated later).
    pub payload: Type,
    /// Memory manager implementation.
    pub manager_ty: TypePath,
    /// Upstream node index (source endpoint).
    pub from_node: usize,
    /// Upstream node output port index.
    pub from_port: usize,
    /// Downstream node index (destination endpoint).
    pub to_node: usize,
    /// Downstream node input port index.
    pub to_port: usize,
    /// Policy expression used for occupancy/admission decisions.
    pub policy: Expr,
    /// Optional human-friendly edge name (e.g., `Some("src->map")`).
    pub name_opt: Option<Expr>,
}
