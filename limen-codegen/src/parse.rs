//! Parser for the Limen graph DSL.
//!
//! This module translates the *tokenized* DSL (as provided by a proc-macro
//! invocation or `syn::parse_str`) into the AST types in [`crate::ast`].
//!
//! ## Input → Output
//! - **Input**: a token stream representing the DSL with the exact shape:
//!   ```text
//!   <vis> struct <GraphName>;
//!   nodes {
//!       <idx>: { ty: <TypePath>, in_ports: <usize>, out_ports: <usize>,
//!                in_payload: <Type>, out_payload: <Type>,
//!                name: <Expr or None>, ingress_policy: <Expr or None> },
//!       ...
//!   }
//!   edges {
//!       <idx>: { ty: <TypePath>, payload: <Type>,
//!                manager: <TypePath>,
//!                from: (<usize>, <usize>), to: (<usize>, <usize>),
//!                policy: <Expr>, name: <Expr or None> },
//!       ...
//!   }
//!   concurrent;   // optional — emit the std concurrent graph flavor
//!   ```
//!   where:
//!   - `<vis>` is a standard Rust visibility (e.g., `pub` or empty).
//!   - `<GraphName>` is an identifier.
//!   - Numeric fields are **integer literals** (no paths, consts, or exprs).
//!   - `ty`, `in_payload`, `out_payload`, `payload` accept Rust type syntax.
//!   - `name` and `policy` accept general Rust expressions; `name` is optional
//!     and may be `None` or any expression of a `Some(...)` form.
//!   - `ingress_policy` is optional and only meaningful for **source** nodes
//!     (nodes with `in_ports == 0 && out_ports > 0`). The generator may create
//!     synthetic ingress edges from it.
//!   - `concurrent` is an optional trailing keyword. When present, the
//!     generator will emit **only** the concurrent (`std`-gated) graph
//!     (inside `pub mod concurrent_graph`). When absent, only the non-std
//!     graph is emitted. Manager types must satisfy `Clone + Send + 'static`
//!     when concurrent is enabled.
//!
//! - **Output**: a fully populated [`crate::ast::GraphDef`]. Structural and
//!   semantic validation (contiguous indices, port bounds, payload agreement,
//!   queue uniformity) is deferred to `crate::validate::validate_definition`.
//!
//! ## Error behavior
//! - The parser checks *shape* and *presence* of required fields, emitting
//!   `syn::Error` on missing or unknown keys and on malformed literals.
//! - Validation that depends on **cross-item relations** is not performed here.
//!
//! ## Example
//! ```text
//! pub struct MyGraph;
//! nodes {
//!   0: { ty: my::Source,  in_ports: 0, out_ports: 1, in_payload: (),  out_payload: u32, name: Some("src"), ingress_policy: MY_QPOL },
//!   1: { ty: my::MapU32,  in_ports: 1, out_ports: 1, in_payload: u32, out_payload: u32, name: Some("map") },
//!   2: { ty: my::SinkU32, in_ports: 1, out_ports: 0, in_payload: u32, out_payload: (),  name: Some("sink") },
//! }
//!
//! edges {
//!   0: { ty: StaticRing<8>, payload: u32, manager: StaticMemoryManager<u32, 8>, from: (0, 0), to: (1, 0), policy: QPOL, name: Some("src->map") },
//!   1: { ty: StaticRing<8>, payload: u32, manager: StaticMemoryManager<u32, 8>, from: (1, 0), to: (2, 0), policy: QPOL, name: Some("map->sink") },
//! }
//! ```
//!
//! The above produces a [`GraphDef`] with 3 nodes and 2 edges. For node 0,
//! the optional `ingress_policy` will be carried forward to code generation
//! (where a synthetic ingress edge may be emitted).

use crate::ast::{EdgeDef, GraphDef, NodeDef};

use proc_macro2::Span;
use syn::parse::{Parse, ParseStream};
use syn::{braced, parenthesized, Expr, Ident, Result, Token, Type, TypePath, Visibility};

impl Parse for GraphDef {
    /// Parse a `GraphDef` from the DSL token stream.
    ///
    /// # Input form
    /// See the module-level docs for the precise grammar and examples.
    ///
    /// # Output
    /// Returns a fully populated [`GraphDef`] with raw indices and fields.
    /// Cross-item validation is **not** performed here; call
    /// [`crate::validate::validate_definition`] after parsing.
    fn parse(input: ParseStream) -> Result<Self> {
        // <vis> struct <GraphName> ;
        let vis: Visibility = input.parse()?;
        input.parse::<Token![struct]>()?;
        let name: Ident = input.parse()?;
        input.parse::<Token![;]>()?;

        // nodes { ... }
        let nodes_kw: Ident = input.parse()?;
        if nodes_kw != "nodes" {
            return Err(syn::Error::new_spanned(nodes_kw, "expected `nodes` block"));
        }
        let nodes_content;
        braced!(nodes_content in input);
        let mut nodes = Vec::new();
        while !nodes_content.is_empty() {
            // <idx> : { ... }
            let idx = parse_usize_lit(nodes_content.parse()?)?;
            nodes_content.parse::<Token![:]>()?;
            let body;
            braced!(body in nodes_content);

            let (mut ty, mut in_ports, mut out_ports, mut in_payload, mut out_payload) =
                (None, None, None, None, None);
            let (mut name_opt, mut ingress_policy_opt) = (None, None);

            // ty: <TypePath>, in_ports: <usize>, ...
            while !body.is_empty() {
                let key: Ident = body.parse()?;
                body.parse::<Token![:]>()?;
                match key.to_string().as_str() {
                    "ty" => ty = Some(body.parse()?),
                    "in_ports" => in_ports = Some(parse_usize_lit(body.parse()?)?),
                    "out_ports" => out_ports = Some(parse_usize_lit(body.parse()?)?),
                    "in_payload" => in_payload = Some(body.parse()?),
                    "out_payload" => out_payload = Some(body.parse()?),
                    "name" => name_opt = Some(body.parse()?),
                    "ingress_policy" => ingress_policy_opt = Some(body.parse()?),
                    other => {
                        return Err(syn::Error::new_spanned(
                            key,
                            format!("unknown node field `{other}`"),
                        ))
                    }
                }
                if body.peek(Token![,]) {
                    body.parse::<Token![,]>()?;
                }
            }

            nodes.push(NodeDef {
                idx,
                ty: ty.ok_or_else(|| err("node.ty is required"))?,
                in_ports: in_ports.ok_or_else(|| err("node.in_ports is required"))?,
                out_ports: out_ports.ok_or_else(|| err("node.out_ports is required"))?,
                in_payload: in_payload.ok_or_else(|| err("node.in_payload is required"))?,
                out_payload: out_payload.ok_or_else(|| err("node.out_payload is required"))?,
                name_opt,
                ingress_policy_opt,
            });

            if nodes_content.peek(Token![,]) {
                nodes_content.parse::<Token![,]>()?;
            }
        }

        // edges { ... }
        let edges_kw: Ident = input.parse()?;
        if edges_kw != "edges" {
            return Err(syn::Error::new_spanned(edges_kw, "expected `edges` block"));
        }
        let edges_content;
        braced!(edges_content in input);
        let mut edges = Vec::new();
        while !edges_content.is_empty() {
            // <idx> : { ... }
            let idx = parse_usize_lit(edges_content.parse()?)?;
            edges_content.parse::<Token![:]>()?;
            let body;
            braced!(body in edges_content);

            let (mut ty, mut payload) = (None::<TypePath>, None::<Type>);
            let (mut from_node, mut from_port, mut to_node, mut to_port) = (None, None, None, None);
            let (mut policy, mut name_opt) = (None, None);
            let mut manager_ty = None::<TypePath>;

            // ty: <TypePath>, payload: <Type>, from: (<usize>,<usize>), to: (...), policy: <Expr>, name: <Expr?>
            while !body.is_empty() {
                let key: Ident = body.parse()?;
                body.parse::<Token![:]>()?;
                match key.to_string().as_str() {
                    "ty" => ty = Some(body.parse()?),
                    "payload" => payload = Some(body.parse()?),
                    "manager" => manager_ty = Some(body.parse()?),
                    "from" => {
                        let paren;
                        parenthesized!(paren in body);
                        let a: Expr = paren.parse()?;
                        paren.parse::<Token![,]>()?;
                        let b: Expr = paren.parse()?;
                        from_node = Some(parse_usize_lit(a)?);
                        from_port = Some(parse_usize_lit(b)?);
                    }
                    "to" => {
                        let paren;
                        parenthesized!(paren in body);
                        let a: Expr = paren.parse()?;
                        paren.parse::<Token![,]>()?;
                        let b: Expr = paren.parse()?;
                        to_node = Some(parse_usize_lit(a)?);
                        to_port = Some(parse_usize_lit(b)?);
                    }
                    "policy" => policy = Some(body.parse()?),
                    "name" => name_opt = Some(body.parse()?),
                    other => {
                        return Err(syn::Error::new_spanned(
                            key,
                            format!("unknown edge field `{other}`"),
                        ))
                    }
                }
                if body.peek(Token![,]) {
                    body.parse::<Token![,]>()?;
                }
            }

            edges.push(EdgeDef {
                idx,
                ty: ty.ok_or_else(|| err("edge.ty is required"))?,
                payload: payload.ok_or_else(|| err("edge.payload is required"))?,
                manager_ty: manager_ty.ok_or_else(|| err("edge.manager is required"))?,
                from_node: from_node.ok_or_else(|| err("edge.from is required"))?,
                from_port: from_port.ok_or_else(|| err("edge.from is required"))?,
                to_node: to_node.ok_or_else(|| err("edge.to is required"))?,
                to_port: to_port.ok_or_else(|| err("edge.to is required"))?,
                policy: policy.ok_or_else(|| err("edge.policy is required"))?,
                name_opt,
            });

            if edges_content.peek(Token![,]) {
                edges_content.parse::<Token![,]>()?;
            }
        }

        // Optional: concurrent;
        let emit_concurrent = if !input.is_empty() {
            let kw: Ident = input.parse()?;
            if kw != "concurrent" {
                return Err(syn::Error::new_spanned(
                    kw,
                    "expected `concurrent` or end of input",
                ));
            }
            input.parse::<Token![;]>()?;
            true
        } else {
            false
        };

        Ok(GraphDef {
            vis,
            name,
            nodes,
            edges,
            emit_concurrent,
        })
    }
}

/// Create a site-anchored `syn::Error` with a short message.
///
/// Used for required-field errors during parsing.
fn err(msg: &str) -> syn::Error {
    syn::Error::new(Span::call_site(), msg)
}

/// Parse a `usize` **integer literal** from a general `syn::Expr`.
///
/// The DSL requires indices and port counts to be *integer literals*.
/// Any other form (paths, arithmetic, constants) is rejected here to keep
/// the grammar simple and predictable for code generation.
fn parse_usize_lit(e: syn::Expr) -> syn::Result<usize> {
    match e {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(li),
            ..
        }) => li.base10_parse::<usize>(),
        _ => Err(syn::Error::new_spanned(e, "expected integer literal")),
    }
}
