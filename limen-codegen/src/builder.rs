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
use limen_core::prelude::payload::Payload;
use std::any::type_name;
use std::path::Path;
use std::path::PathBuf;
use syn::parse_str;
use syn::{Expr, Ident, Type, TypePath, Visibility};

// Bring core types in so the builder can accept "real" NodeLink/EdgeLink values.
use limen_core::edge::link::EdgeLink;
use limen_core::message::Message;
use limen_core::node::link::NodeLink;
use limen_core::policy::{AdmissionPolicy, EdgePolicy, OverBudgetAction};

/// Visibility options for generated graph types used by `GraphBuilder::new`.
#[derive(Debug, Clone, Copy)]
pub enum GraphVisibility {
    /// Creates graph with public visibility.
    Public,
    /// Creates graph with pub(crate) visibility.
    Crate,
    /// Creates graph with pub(super) visibility.
    Super,
    /// Creates graph with private visibility.
    Private,
}

impl GraphVisibility {
    fn as_syn_visibility(&self) -> Visibility {
        match &self {
            GraphVisibility::Public => parse_str::<Visibility>("pub").expect("parse pub"),
            GraphVisibility::Crate => {
                parse_str::<Visibility>("pub(crate)").expect("parse pub(crate)")
            }
            GraphVisibility::Super => {
                parse_str::<Visibility>("pub(super)").expect("parse pub(super)")
            }
            GraphVisibility::Private => Visibility::Inherited,
        }
    }
}

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
    /// # Parameters
    /// - `name`: graph type name as an identifier string (e.g. `"MyGraph"`).
    /// - `vis`: a `GraphVisibility` enum controlling `pub` / `pub(crate)` / `pub(super)` / private.
    ///
    /// This convenience avoids calling `syn::parse_quote` in build.rs; we
    /// synthesize a `syn::Visibility` internally.
    pub fn new(name: &str, vis: GraphVisibility) -> Self {
        let vis_syn = vis.as_syn_visibility();
        let name_ident: Ident = parse_str(name).expect("invalid graph name");
        Self {
            vis: Some(vis_syn),
            name: Some(name_ident),
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

    /// Append a Node described by a core `NodeLink` — the builder extracts
    /// its type, payload types, id, and optional name and constructs an AST
    /// `ast::NodeDef` entry.
    ///
    /// `ingress_policy` is optional; supply it for source nodes.
    pub fn node_from_link<N, const IN: usize, const OUT: usize, InP, OutP>(
        mut self,
        link: NodeLink<N, IN, OUT, InP, OutP>,
        ingress_policy: Option<EdgePolicy>,
    ) -> Self
    where
        InP: Payload + 'static,
        OutP: Payload + 'static,
        N: limen_core::node::Node<IN, OUT, InP, OutP> + 'static,
    {
        // Extract numeric index
        let idx = *link.id().as_usize();

        // Type names for node and payload types
        let node_ty = type_of_val_to_syn_type::<N>();
        let in_p_ty = type_of_val_to_syn_type::<InP>();
        let out_p_ty = type_of_val_to_syn_type::<OutP>();

        // Optional name expression
        let name_opt = link.name().map(|nm| name_to_expr(Some(nm)));

        // Optional ingress policy expression (convert if supplied).
        let ingress_expr = match ingress_policy {
            Some(p) => {
                let s = edge_policy_value_to_string(&p);
                Some(parse_str::<Expr>(&s).expect("failed to parse EdgePolicy expr"))
            }
            None => None,
        };

        self.nodes.push(ast::NodeDef {
            idx,
            ty: match node_ty {
                Type::Path(tp) => tp, // convert Type -> TypePath
                _ => panic!("node type must be a path"),
            },
            in_ports: IN,
            out_ports: OUT,
            in_payload: in_p_ty,
            out_payload: out_p_ty,
            name_opt,
            ingress_policy_opt: ingress_expr,
        });
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

    /// Append an Edge described by a core `EdgeLink`. The builder extracts the
    /// queue type, payload type, endpoints, policy, and id and converts them
    /// into an `ast::EdgeDef`.
    pub fn edge_from_link<Q, P>(mut self, link: EdgeLink<Q, P>) -> Self
    where
        P: Payload + 'static,
        Q: limen_core::edge::Edge<Item = Message<P>> + 'static,
    {
        let id = *link.id().as_usize();

        // Queue type and payload type
        let q_ty = type_of_val_to_syn_type::<Q>();
        let p_ty = type_of_val_to_syn_type::<P>();

        // Endpoint indices: upstream and downstream NodeIndex / PortIndex -> usize
        let up = link.upstream_port();
        let dn = link.downstream_port();
        let from_node = *up.node().as_usize();
        let from_port = *up.port().as_usize();
        let to_node = *dn.node().as_usize();
        let to_port = *dn.port().as_usize();

        // Policy expression
        let policy_expr = {
            let s = edge_policy_value_to_string(link.policy());
            parse_str::<Expr>(&s).expect("failed to parse EdgePolicy")
        };

        let name_opt = link.name().map(|nm| name_to_expr(Some(nm)));

        self.edges.push(ast::EdgeDef {
            idx: id,
            ty: match q_ty {
                Type::Path(tp) => tp,
                _ => panic!("edge queue type must be a path"),
            },
            payload: p_ty,
            from_node,
            from_port,
            to_node,
            to_port,
            policy: policy_expr,
            name_opt,
        });

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
    pub fn to_graph_def(self) -> ast::GraphDef {
        ast::GraphDef {
            vis: self.vis.expect("visibility required"),
            name: self.name.expect("name required"),
            nodes: self.nodes,
            edges: self.edges,
        }
    }
    /// Finalize and produce a `GeneratedGraph` that owns the AST and can write it to file.
    pub fn finish(self) -> GraphWriter {
        GraphWriter {
            g: ast::GraphDef {
                vis: self.vis.expect("visibility required"),
                name: self.name.expect("name required"),
                nodes: self.nodes,
                edges: self.edges,
            },
        }
    }
}

/// The result of `GraphBuilder::finish()` — owns the AST and can write it to disk.
pub struct GraphWriter {
    g: ast::GraphDef,
}

impl GraphWriter {
    /// Consume `self` and write the generated code to `dest` using the
    /// codegen emitter and formatter. Returns the path written.
    pub fn write_to_path<P: AsRef<Path>>(self, dest: P) -> Result<PathBuf, crate::CodegenError> {
        crate::expand_ast_to_file(self.g, dest)
    }

    /// Write graph to default path.
    /// - `$OUT_DIR/generated/<filename>`.
    ///
    /// Returns an error if `OUT_DIR` is not set or the write fails.
    pub fn write(self, filename: &str) -> Result<PathBuf, crate::CodegenError> {
        let name_with_extension = {
            let p = std::path::Path::new(filename);
            if p.extension().is_none() {
                // No extension -> append .rs to the input string as given.
                format!("{}.rs", filename)
            } else {
                // Keep exactly what caller passed
                filename.to_string()
            }
        };

        // Read OUT_DIR (map VarError into CodegenError::Io)
        let out_dir = std::env::var("OUT_DIR").map_err(|e| {
            crate::CodegenError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("OUT_DIR environment variable not set: {}", e),
            ))
        })?;

        let dest_dir = std::path::Path::new(&out_dir).join("generated");
        std::fs::create_dir_all(&dest_dir).map_err(crate::CodegenError::Io)?;
        let dest = dest_dir.join(name_with_extension);

        // Use existing writer (consumes self)
        crate::expand_ast_to_file(self.g, &dest)?;

        // Cargo bookkeeping so build script is re-run when build.rs changes
        // and give a visible message with the written path.
        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:warning=generated graph A -> {}", dest.display());

        Ok(dest)
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

    /// Set the concrete node type using a real Rust type `T`.
    /// This avoids calling `syn::parse_quote!` at the call site.
    pub fn ty<T: 'static>(mut self) -> Self {
        let t = type_of_val_to_syn_type::<T>();
        match t {
            Type::Path(tp) => {
                // convert syn::Type::Path into TypePath
                self.ty = Some(TypePath {
                    qself: None,
                    path: tp.path,
                });
            }
            _ => panic!("ty_val: expected a path type for node type"),
        }
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

    /// Set the input payload type using a Rust type `T`.
    pub fn in_payload<T: 'static>(mut self) -> Self {
        let t = type_of_val_to_syn_type::<T>();
        self.in_payload = Some(t);
        self
    }

    /// Set the output payload type using a Rust type `T`.
    pub fn out_payload<T: 'static>(mut self) -> Self {
        let t = type_of_val_to_syn_type::<T>();
        self.out_payload = Some(t);
        self
    }

    /// Set the optional node name from a Rust `&'static str` (or `None`).
    pub fn name(mut self, s: Option<&'static str>) -> Self {
        self.name_opt = Some(name_to_expr(s));
        self
    }

    /// Supply a concrete `EdgePolicy` value for ingress policy (converted internally).
    pub fn ingress_policy(mut self, p: EdgePolicy) -> Self {
        let s = edge_policy_value_to_string(&p);
        self.ingress_policy_opt = Some(parse_str::<Expr>(&s).expect("parse EdgePolicy"));
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

    /// Set the edge queue type using a real Rust type `T`.
    pub fn ty<T: 'static>(mut self) -> Self {
        let t = type_of_val_to_syn_type::<T>();
        match t {
            Type::Path(tp) => {
                self.ty = Some(TypePath {
                    qself: None,
                    path: tp.path,
                });
            }
            _ => panic!("ty_val: expected a path type for edge queue"),
        }
        self
    }

    /// Set the edge payload type using a Rust type `T`.
    pub fn payload<T: 'static>(mut self) -> Self {
        let t = type_of_val_to_syn_type::<T>();
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

    /// Supply a concrete `EdgePolicy` value (converted internally).
    pub fn policy(mut self, p: EdgePolicy) -> Self {
        let s = edge_policy_value_to_string(&p);
        self.policy = Some(parse_str::<Expr>(&s).expect("parse EdgePolicy"));
        self
    }

    /// Set the optional edge name using a Rust `&'static str` (or `None`).
    pub fn name(mut self, s: Option<&'static str>) -> Self {
        self.name_opt = Some(name_to_expr(s));
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

/// Turn an `Option<&'static str>` name into a `syn::Expr` (Some("...") or None).
fn name_to_expr(n: Option<&'static str>) -> Expr {
    match n {
        Some(s) => parse_str::<Expr>(&format!("Some({:?})", s)).expect("parse name"),
        None => parse_str::<Expr>("None").expect("parse None"),
    }
}

/// Convert a Rust generic type `T` into a `syn::Type` by using `type_name::<T>()`.
/// This is the mechanism that lets the builder accept real Rust types without
/// the caller having to `syn::parse_quote` them.
fn type_of_val_to_syn_type<T: 'static>() -> Type {
    let s = type_name::<T>();
    parse_str::<Type>(s).unwrap_or_else(|e| panic!("failed to parse type `{}`: {}", s, e))
}

/// Convert a runtime `EdgePolicy` value into a fully-qualified Rust expression
/// string that constructs an equivalent `EdgePolicy` using public constructors.
/// We use the public getters you added (caps(), admission(), over_budget()).
fn edge_policy_value_to_string(p: &EdgePolicy) -> String {
    // Queue caps
    let caps = p.caps(); // assume returns Copy/Clone or by value
    let max_items = caps.max_items();
    let soft_items = caps.soft_items();
    let max_bytes = match caps.max_bytes() {
        Some(b) => format!("Some({})", b),
        None => "None".to_string(),
    };
    let soft_bytes = match caps.soft_bytes() {
        Some(b) => format!("Some({})", b),
        None => "None".to_string(),
    };

    // admission
    let admission_str = match p.admission() {
        AdmissionPolicy::DropNewest => "limen_core::policy::AdmissionPolicy::DropNewest",
        AdmissionPolicy::DropOldest => "limen_core::policy::AdmissionPolicy::DropOldest",
        AdmissionPolicy::Block => "limen_core::policy::AdmissionPolicy::Block",
        AdmissionPolicy::DeadlineAndQoSAware => {
            "limen_core::policy::AdmissionPolicy::DeadlineAndQoSAware"
        }
        _ => panic!("Unsupported AdmissionPolicy {:?}", p.admission()),
    };

    let over_str = match p.over_budget() {
        OverBudgetAction::Drop => "limen_core::policy::OverBudgetAction::Drop",
        OverBudgetAction::SkipStage => "limen_core::policy::OverBudgetAction::SkipStage",
        OverBudgetAction::Degrade => "limen_core::policy::OverBudgetAction::Degrade",
        OverBudgetAction::DefaultOnTimeout => {
            "limen_core::policy::OverBudgetAction::DefaultOnTimeout"
        }
        _ => panic!("Unsupported OverBudgetAction {:?}", p.over_budget()),
    };

    format!(
        "limen_core::policy::EdgePolicy::new(limen_core::policy::QueueCaps::new({}, {}, {}, {}), {}, {})",
        max_items, soft_items, max_bytes, soft_bytes, admission_str, over_str
    )
}
