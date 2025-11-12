//! limen-codegen/src/builder.rs
//!
//! Strongly typed, Language Server–friendly builder for Limen graphs
//! (no proc-macro input and no stringly-typed DSL).
//!
//! Use this from your crate’s `build.rs` to construct a
//! [`crate::ast::GraphDef`] with ordinary Rust types and expressions
//! (typically authored via `syn::parse_quote!`). Then pass the finished AST to
//! `expand_ast_to_file(..)` to emit the generated graph implementation.
//!
//! # Example
//!
//! ```rust,ignore
//! use limen_codegen::builder::{GraphBuilder, Node, Edge};
//! use syn::parse_quote;
//!
//! let edge_policy: syn::Expr = parse_quote! {
//!     limen_core::policy::EdgePolicy::default()
//! };
//!
//! let ast = GraphBuilder::new(parse_quote!(pub), parse_quote!(MyGraph))
//!     .node(
//!         Node::new(0)
//!             .ty(parse_quote!(MySource))
//!             .in_ports(0)
//!             .out_ports(1)
//!             .in_payload(parse_quote!(()))
//!             .out_payload(parse_quote!(u32))
//!             .name(parse_quote!(Some("source")))
//!             .ingress_policy(edge_policy.clone()),
//!     )
//!     .node(
//!         Node::new(1)
//!             .ty(parse_quote!(MyMap))
//!             .in_ports(1)
//!             .out_ports(1)
//!             .in_payload(parse_quote!(u32))
//!             .out_payload(parse_quote!(u32))
//!             .name(parse_quote!(Some("map"))),
//!     )
//!     .edge(
//!         Edge::new(0)
//!             .ty(parse_quote!(limen_core::edge::spsc_ringbuf::SpscRingbuf<limen_core::message::Message<u32>>))
//!             .payload(parse_quote!(u32))
//!             .from(0, 0)
//!             .to(1, 0)
//!             .policy(edge_policy.clone())
//!             .name(parse_quote!(Some("source->map"))),
//!     )
//!     .finish();
//!
//! // limen_codegen::expand_ast_to_file(ast, out_path)?;
//! ```

use crate::ast;
use syn::{Expr, Ident, Type, TypePath, Visibility};

/// Builder for a single Limen graph AST (`ast::GraphDef`).
///
/// `GraphBuilder` collects nodes and edges with precise typing, so you get full
/// IDE completion and compiler checking while authoring graphs in `build.rs`.
#[derive(Default)]
pub struct GraphBuilder {
    vis: Option<Visibility>,
    name: Option<Ident>,
    nodes: Vec<ast::NodeDef>,
    edges: Vec<ast::EdgeDef>,
}

impl GraphBuilder {
    /// Create a new graph builder.
    ///
    /// - `vis`: Visibility of the generated graph type (e.g. `pub`).
    /// - `name`: The identifier for the generated graph type.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use limen_codegen::builder::GraphBuilder;
    /// use syn::parse_quote;
    ///
    /// let gb = GraphBuilder::new(parse_quote!(pub), parse_quote!(MyGraph));
    /// ```
    pub fn new(vis: Visibility, name: Ident) -> Self {
        Self {
            vis: Some(vis),
            name: Some(name),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Append a fully specified node to the graph.
    ///
    /// The `Node` parameter is a fluent sub-builder; it is converted to an
    /// `ast::NodeDef` at call time.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use limen_codegen::builder::{GraphBuilder, Node};
    /// use syn::parse_quote;
    ///
    /// let gb = GraphBuilder::new(parse_quote!(pub), parse_quote!(MyGraph))
    ///     .node(
    ///         Node::new(0)
    ///             .ty(parse_quote!(MySource))
    ///             .in_ports(0)
    ///             .out_ports(1)
    ///             .in_payload(parse_quote!(()))
    ///             .out_payload(parse_quote!(u32))
    ///     );
    /// ```
    pub fn node(mut self, n: Node) -> Self {
        self.nodes.push(n.finish());
        self
    }

    /// Append a fully specified edge to the graph.
    ///
    /// The `Edge` parameter is a fluent sub-builder; it is converted to an
    /// `ast::EdgeDef` at call time.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use limen_codegen::builder::{GraphBuilder, Edge};
    /// use syn::parse_quote;
    ///
    /// let policy: syn::Expr = parse_quote!(limen_core::policy::EdgePolicy::default());
    ///
    /// let gb = GraphBuilder::new(parse_quote!(pub), parse_quote!(MyGraph))
    ///     .edge(
    ///         Edge::new(0)
    ///             .ty(parse_quote!(limen_core::edge::NoQueue<u32>))
    ///             .payload(parse_quote!(u32))
    ///             .from(0, 0)
    ///             .to(1, 0)
    ///             .policy(policy)
    ///     );
    /// ```
    pub fn edge(mut self, e: Edge) -> Self {
        self.edges.push(e.finish());
        self
    }

    /// Finalize and produce the `ast::GraphDef`.
    ///
    /// This consumes the builder and returns a complete graph AST suitable for
    /// code generation.
    ///
    /// # Panics
    ///
    /// Panics if the builder was not created via [`GraphBuilder::new`], or if
    /// `vis` or `name` were not set.
    pub fn finish(self) -> ast::GraphDef {
        ast::GraphDef {
            vis: self.vis.expect("visibility required"),
            name: self.name.expect("name required"),
            nodes: self.nodes,
            edges: self.edges,
        }
    }
}

/// Fluent builder for a single node declaration.
pub struct Node {
    idx: usize,
    ty: Option<TypePath>,
    in_ports: Option<usize>,
    out_ports: Option<usize>,
    in_payload: Option<Type>,
    out_payload: Option<Type>,
    name_opt: Option<Expr>,
    ingress_policy_opt: Option<Expr>,
}

impl Node {
    /// Start a node builder for node index `idx`.
    ///
    /// The index is the stable node identifier used throughout the generated
    /// graph. It must be unique within the graph.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use limen_codegen::builder::Node;
    /// let n = Node::new(0);
    /// ```
    pub fn new(idx: usize) -> Self {
        Self {
            idx,
            ty: None,
            in_ports: None,
            out_ports: None,
            in_payload: None,
            out_payload: None,
            name_opt: None,
            ingress_policy_opt: None,
        }
    }

    /// Set the concrete node type (e.g. `MySource`, `MyMap<T>`, `MySink`).
    pub fn ty(mut self, t: TypePath) -> Self {
        self.ty = Some(t);
        self
    }

    /// Set the number of input ports for this node.
    ///
    /// If `in_ports == 0` and `out_ports > 0`, the node will be treated as a
    /// **source** by codegen.
    pub fn in_ports(mut self, n: usize) -> Self {
        self.in_ports = Some(n);
        self
    }

    /// Set the number of output ports for this node.
    ///
    /// If `out_ports == 0` and `in_ports > 0`, the node will be treated as a
    /// **sink** by codegen.
    pub fn out_ports(mut self, n: usize) -> Self {
        self.out_ports = Some(n);
        self
    }

    /// Set the input payload type carried by this node’s input ports.
    pub fn in_payload(mut self, t: Type) -> Self {
        self.in_payload = Some(t);
        self
    }

    /// Set the output payload type produced on this node’s output ports.
    pub fn out_payload(mut self, t: Type) -> Self {
        self.out_payload = Some(t);
        self
    }

    /// Set an optional human-readable node name.
    ///
    /// Pass an expression such as `parse_quote!(Some("map"))` or `parse_quote!(None)`.
    pub fn name(mut self, e: Expr) -> Self {
        self.name_opt = Some(e);
        self
    }

    /// Set the ingress edge policy for **source** nodes.
    ///
    /// Only meaningful when `in_ports == 0`. The policy expression is embedded
    /// verbatim into the generated code.
    pub fn ingress_policy(mut self, e: Expr) -> Self {
        self.ingress_policy_opt = Some(e);
        self
    }

    /// Finalise the node and produce the Nodedef.
    fn finish(self) -> ast::NodeDef {
        ast::NodeDef {
            idx: self.idx,
            ty: self.ty.expect("node.ty"),
            in_ports: self.in_ports.expect("node.in_ports"),
            out_ports: self.out_ports.expect("node.out_ports"),
            in_payload: self.in_payload.expect("node.in_payload"),
            out_payload: self.out_payload.expect("node.out_payload"),
            name_opt: self.name_opt,
            ingress_policy_opt: self.ingress_policy_opt,
        }
    }
}

/// Fluent builder for a single edge declaration.
pub struct Edge {
    idx: usize,
    ty: Option<TypePath>,
    payload: Option<Type>,
    from_node: Option<usize>,
    from_port: Option<usize>,
    to_node: Option<usize>,
    to_port: Option<usize>,
    policy: Option<Expr>,
    name_opt: Option<Expr>,
}

impl Edge {
    /// Start an edge builder for edge index `idx`.
    ///
    /// The index is the stable edge identifier used in descriptors. It must be
    /// unique among *real* (non-ingress) edges.
    pub fn new(idx: usize) -> Self {
        Self {
            idx,
            ty: None,
            payload: None,
            from_node: None,
            from_port: None,
            to_node: None,
            to_port: None,
            policy: None,
            name_opt: None,
        }
    }

    /// Set the queue type for this edge (e.g. an SPSC ring buffer).
    ///
    /// Provide a `TypePath` such as:
    /// `parse_quote!(limen_core::edge::spsc_ringbuf::SpscRingbuf<limen_core::message::Message<u32>>)`.
    pub fn ty(mut self, t: TypePath) -> Self {
        self.ty = Some(t);
        self
    }

    /// Set the payload type carried on this edge.
    pub fn payload(mut self, t: Type) -> Self {
        self.payload = Some(t);
        self
    }

    /// Set the upstream `(node_index, port_index)` for this edge.
    pub fn from(mut self, node: usize, port: usize) -> Self {
        self.from_node = Some(node);
        self.from_port = Some(port);
        self
    }

    /// Set the downstream `(node_index, port_index)` for this edge.
    pub fn to(mut self, node: usize, port: usize) -> Self {
        self.to_node = Some(node);
        self.to_port = Some(port);
        self
    }

    /// Set the edge policy expression (admission, budget, capacity, etc.).
    ///
    /// The expression is embedded verbatim into the generated code.
    pub fn policy(mut self, e: Expr) -> Self {
        self.policy = Some(e);
        self
    }

    /// Set an optional human-readable edge name.
    ///
    /// Pass an expression such as `parse_quote!(Some("a->b"))` or `parse_quote!(None)`.
    pub fn name(mut self, e: Expr) -> Self {
        self.name_opt = Some(e);
        self
    }

    /// Finalise the edge and produce the Edgedef.
    fn finish(self) -> ast::EdgeDef {
        ast::EdgeDef {
            idx: self.idx,
            ty: self.ty.expect("edge.ty"),
            payload: self.payload.expect("edge.payload"),
            from_node: self.from_node.expect("edge.from.node"),
            from_port: self.from_port.expect("edge.from.port"),
            to_node: self.to_node.expect("edge.to.node"),
            to_port: self.to_port.expect("edge.to.port"),
            policy: self.policy.expect("edge.policy"),
            name_opt: self.name_opt,
        }
    }
}
