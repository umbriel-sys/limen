// Copyright © 2025–present Arlo Louis Byrne (idky137)
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0.
// See the LICENSE-APACHE file in the project root for license terms.

#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-codegen — reusable generator for Limen graphs
//!
//! This crate turns a compact, declarative graph DSL into fully-typed Rust
//! implementations that conform to the `limen-core` graph, node, edge, and
//! policy traits. The emitted code always includes a single concrete graph
//! structure, with an optional std-only scoped execution API.
//!
//! It is designed to be used in **two** ways:
//!
//! 1. **Proc-macro mode** (recommended for quick iteration):
//!
//!    - Add `limen-build` (proc-macro crate) and `limen-core` to your `Cargo.toml`.
//!    - Write the DSL inline in your code using the `define_graph! { ... }` macro.
//!    - The macro forwards its token stream to `limen-codegen::expand_tokens(..)`.
//!
//!    ```rust,ignore
//!    use limen_build::define_graph;
//!
//!    define_graph! {
//!        pub struct MyGraph;
//!
//!        nodes {
//!            0: {
//!                ty: my_crate::nodes::MySourceNode,
//!                in_ports: 0,
//!                out_ports: 1,
//!                in_payload: (),
//!                out_payload: u32,
//!                name: Some("src"),
//!                // See "Ingress edges" below for rules:
//!                // Only valid for source nodes (in_ports == 0, out_ports > 0).
//!                ingress_policy: my_crate::policies::Q32_POLICY
//!            },
//!            1: {
//!                ty: my_crate::nodes::MyMapNode,
//!                in_ports: 1,
//!                out_ports: 1,
//!                in_payload: u32,
//!                out_payload: u32,
//!                name: Some("map")
//!            },
//!            2: {
//!                ty: my_crate::nodes::MySinkNode,
//!                in_ports: 1,
//!                out_ports: 0,
//!                in_payload: u32,
//!                out_payload: (),
//!                name: Some("sink")
//!            },
//!        }
//!
//!        edges {
//!            0: {
//!                ty: limen_core::edge::bench::TestSpscRingBuf<8>,
//!                payload: u32,
//!                manager: limen_core::memory::static_manager::StaticMemoryManager<u32, 8>,
//!                from: (0, 0),
//!                to: (1, 0),
//!                policy: my_crate::policies::EDGE_POLICY,
//!                name: Some("src->map")
//!            },
//!            1: {
//!                ty: limen_core::edge::bench::TestSpscRingBuf<8>,
//!                payload: u32,
//!                manager: limen_core::memory::static_manager::StaticMemoryManager<u32, 8>,
//!                from: (1, 0),
//!                to: (2, 0),
//!                policy: my_crate::policies::EDGE_POLICY,
//!                name: Some("map->sink")
//!            },
//!        }
//!
//!        concurrent;
//!    }
//!    ```
//!
//! The trailing `concurrent;` keyword does not generate a separate graph type.
//! It adds the std-only `ScopedGraphApi` implementation for the same graph.
//!
//! 2. **Build-script mode** (recommended when proc-macros slow down the language server or you want to inspect/generated source):
//!
//!    - Add `limen-codegen` (this crate) and `limen-core` to your `Cargo.toml`.
//!    - Put your DSL in a file (for example, `src/my_graph.limen`).
//!    - In `build.rs`, call `expand_str_to_file(..)` to emit pretty-printed Rust
//!      into `OUT_DIR`, then `include!()` it from your library or binary.
//!
//!    ```rust,ignore
//!    // build.rs
//!    fn main() {
//!        let spec = std::fs::read_to_string("src/my_graph.limen").unwrap();
//!        let out = std::env::var("OUT_DIR").unwrap();
//!        let dest = std::path::Path::new(&out).join("my_graph.rs");
//!        limen_codegen::expand_str_to_file(&spec, &dest).unwrap();
//!        println!("cargo:rerun-if-changed=src/my_graph.limen");
//!    }
//!    ```
//!
//!    ```rust,ignore
//!    // lib.rs or main.rs
//!    include!(concat!(env!("OUT_DIR"), "/my_graph.rs"));
//!    ```
//!
//!    You can also build the graph programmatically in `build.rs` using
//!    [`builder::GraphBuilder`] instead of writing the DSL as a string:
//!
//!    ```rust,ignore
//!    use limen_codegen::builder::{Edge, GraphBuilder, GraphVisibility, Node};
//!    use limen_core::policy::{AdmissionPolicy, EdgePolicy, OverBudgetAction, QueueCaps};
//!
//!    fn main() {
//!        let edge_policy = EdgePolicy::new(
//!            QueueCaps::new(8, 6, None, None),
//!            AdmissionPolicy::DropNewest,
//!            OverBudgetAction::Drop,
//!        );
//!
//!        GraphBuilder::new("MyGraph", GraphVisibility::Public)
//!            .node(
//!                Node::new(0)
//!                    .ty::<my_crate::nodes::MySource>()
//!                    .in_ports(0)
//!                    .out_ports(1)
//!                    .in_payload::<()>()
//!                    .out_payload::<u32>()
//!                    .name(Some("src"))
//!                    .ingress_policy(edge_policy),
//!            )
//!            .node(
//!                Node::new(1)
//!                    .ty::<my_crate::nodes::MyMap>()
//!                    .in_ports(1)
//!                    .out_ports(1)
//!                    .in_payload::<u32>()
//!                    .out_payload::<u32>()
//!                    .name(Some("map")),
//!            )
//!            .node(
//!                Node::new(2)
//!                    .ty::<my_crate::nodes::MySink>()
//!                    .in_ports(1)
//!                    .out_ports(0)
//!                    .in_payload::<u32>()
//!                    .out_payload::<()>()
//!                    .name(Some("sink")),
//!            )
//!            .edge(
//!                Edge::new(0)
//!                    .ty::<my_crate::queues::MyQueue<u32, 8>>()
//!                    .payload::<u32>()
//!                    .manager_ty::<my_crate::memory::MyMemoryManager<u32>>()
//!                    .from(0, 0)
//!                    .to(1, 0)
//!                    .policy(edge_policy)
//!                    .name(Some("src->map")),
//!            )
//!            .edge(
//!                Edge::new(1)
//!                    .ty::<my_crate::queues::MyQueue<u32, 8>>()
//!                    .payload::<u32>()
//!                    .manager_ty::<my_crate::memory::MyMemoryManager<u32>>()
//!                    .from(1, 0)
//!                    .to(2, 0)
//!                    .policy(edge_policy)
//!                    .name(Some("map->sink")),
//!            )
//!            .concurrent(false)
//!            .finish()
//!            .write("my_graph")
//!            .unwrap();
//!    }
//!    ```
//!
//!    Set `.concurrent(true)` to additionally emit the std-only
//!    `ScopedGraphApi` implementation for the same graph type.
//!
//! ## What gets generated
//!
//! Each invocation emits a single concrete graph type.
//!
//! - The generated graph struct stores:
//!   - `nodes`: a tuple of `NodeLink<..>` (one per node),
//!   - `edges`: a tuple of `EdgeLink<..>` (one per **real** edge; see ingress below),
//!   - `managers`: a tuple of memory manager instances (one per real edge).
//!
//! - It also implements `GraphApi` for the concrete type, plus the per-index helper
//!   traits (`GraphNodeAccess`, `GraphEdgeAccess`, `GraphNodeTypes`,
//!   `GraphNodeContextBuilder`) that wire the graph into the Limen runtime APIs.
//!
//! - When `concurrent = false` (default), codegen emits the graph structure and
//!   the core `GraphApi` / node-access / context-builder impls only.
//!
//! - When `concurrent = true`, codegen additionally emits a std-only
//!   `ScopedGraphApi` implementation for that same graph type, behind
//!   `#[cfg(feature = "std")]` in the downstream crate.
//!
//! ### Feature flag note
//! The std-only scoped execution code is emitted behind `#[cfg(feature = "std")]`
//! **in the generated file**.
//! This crate (`limen-codegen`) does not define or forward a `std` feature; you control it in
//! the crate that **compiles** the generated code.
//!
//! ## DSL: shape and types
//!
//! The DSL defines one graph per block:
//!
//! - A visibility and a struct name: `pub struct MyGraph;`
//! - A `nodes { ... }` section: numbered nodes, each with type and I/O shape.
//! - An `edges { ... }` section: numbered edges, each with its queue type, payload type, endpoints, and policy.
//!
//! **Node fields** (all required unless marked optional):
//!
//! - `ty: <TypePath>` — Concrete node implementation type.
//! - `in_ports: <usize>` — Number of input ports (constant).
//! - `out_ports: <usize>` — Number of output ports (constant).
//! - `in_payload: <Type>` — Type received on each input port.
//! - `out_payload: <Type>` — Type emitted on each output port.
//! - `name: <Option<Expr>>` — Optional human-friendly identifier (for descriptors).
//! - `ingress_policy: <Expr>` — **Optional** policy that creates a *synthetic* ingress edge
//!   for this node. See **Ingress edges** below.
//!
//! **Edge fields** (all required unless marked optional):
//!
//! - `ty: <TypePath>` — Queue implementation type for this edge (for example, `TestSpscRingBuf<8>`).
//! - `payload: <Type>` — Payload carried on this edge (must match node `out_payload` / `in_payload`).
//! - `manager: <TypePath>` — Memory manager implementation for this edge (for example,
//!   `StaticMemoryManager<P, DEPTH>` for `no_std` or `ConcurrentMemoryManager<P>` for concurrent graphs).
//! - `from: (<usize>, <usize>)` — `(node_index, out_port_index)`.
//! - `to: (<usize>, <usize>)` — `(node_index, in_port_index)`.
//! - `policy: <Expr>` — Policy value used to compute occupancy and admission.
//! - `name: <Option<Expr>>` — Optional human-friendly identifier (for descriptors).
//!
//! ### Important rules and assumptions
//!
//! 0. **Two edge classes**  
//!    - *Ingress* edges are **synthetic** and created only for source nodes that
//!      specify `ingress_policy`. They occupy the lowest global edge indices.
//!    - *Real* edges are those declared in `edges { ... }` and are stored in the graph.
//!
//! 1. **Contiguous indices**  
//!    - Node indices must be contiguous `0..nodes.len()` with no gaps.
//!    - Edge indices must be contiguous `0..edges.len()` with no gaps.
//!
//! 2. **Port bounds**  
//!    - For every edge, `from_port < from_node.out_ports` and `to_port < to_node.in_ports`.
//!
//! 3. **Payload compatibility**  
//!    - For every edge, `edge.payload == from_node.out_payload == to_node.in_payload` (token-level equality).
//!
//! 4. **Queue uniformity per node**  
//!    - All inbound edges to the same node must have an identical queue type.
//!    - All outbound edges from the same node must have an identical queue type.
//!    - This allows the generator to infer a single `InQ` and `OutQ` type per node.
//!
//! 5. **Manager uniformity per node**
//!    - All inbound edges to the same node must have an identical manager type.
//!    - All outbound edges from the same node must have an identical manager type.
//!    - This allows the generator to infer a single `InM` and `OutM` type per node.
//!
//! 6. **Ingress edges (synthetic)**  
//!    - If a node specifies `ingress_policy`, a *synthetic* ingress edge is created for that node.
//!    - Ingress edges do **not** live in the real `edges` tuple and do **not** carry data;
//!      they exist to expose external ingress occupancy via the node’s source interface.
//!    - **Assumption:** `ingress_policy` may only be specified for **source nodes**
//!      (`in_ports == 0` and `out_ports > 0`) that implement the source interface in `limen-core`.
//!      These ingress edges occupy the lowest global edge indices `[0..ingress_count)`.
//!
//! 7. **Dependency on `limen-core`**  
//!    - Generated code references the `limen_core` crate (note the underscore), which must be
//!      available to the downstream crate. Ensure your Cargo manifest includes a dependency on
//!      `limen-core` (the hyphenated package name maps to the `limen_core` crate identifier).
//!
//! ## Programmatic entry points (when not using the proc macro)
//!
//! All of the following:
//! - parse the DSL (from tokens or string),
//! - validate its structure and typing,
//! - and emit the graph plus any optional scoped API selected by the input AST.
//!
//! - [`expand_tokens`]: parse+validate+emit from a token stream (used by the proc macro).
//! - [`expand_str_to_tokens`]: parse+validate+emit from a `&str` DSL (for build scripts or tests).
//! - [`expand_str_to_string`]: same as above, but pretty-prints to a Rust source string.
//! - [`expand_str_to_file`]: same as above, writes to a path (creating parent directories if needed).
//! - [`expand_ast_to_tokens`], [`expand_ast_to_file`]: like the above, but take a typed AST
//!   (for use with the `builder` module so you can write graphs as normal Rust).
//!
//! Each entry point emits the single graph type, plus the optional std-only
//! scoped execution API determined by the `emit_concurrent` flag on the input AST.
//!
//! All entry points perform **validation** before emitting code. Errors are returned as
//! [`CodegenError`], with precise messages for parsing, validation, pretty-print, or I/O failures.

/// Internal: Abstract syntax tree for the DSL (consumed by parsing, validation, and emission).
mod ast;
/// Optional: typed, LS-friendly graph builder (no proc-macro, no big strings).
pub mod builder;
/// Internal: Code emission — turns a validated AST into a `TokenStream` of Rust code.
mod gen;
/// Internal: DSL parser — converts the `define_graph!` body (or a string) into an AST.
mod parse;
/// Internal: Structural and semantic checks for a well-formed graph.
mod validate;

use proc_macro2::TokenStream as TokenStream2;
use std::path::{Path, PathBuf};

/// Errors that can occur while expanding the graph DSL into Rust code.
#[derive(thiserror::Error, Debug)]
pub enum CodegenError {
    /// The DSL could not be parsed into a valid AST.
    #[error("parse error: {0}")]
    Parse(#[from] syn::Error),

    /// The AST failed semantic validation (for example, non-contiguous indices,
    /// port bound violations, payload mismatches, or queue non-uniformity).
    #[error("validation error: {0}")]
    Validate(String),

    /// I/O failure while reading or writing generated code.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Pretty-printing (token → formatted Rust source) failed.
    #[error("prettyprint failed: {0}")]
    Pretty(String),
}

/// Validate and emit Rust code from a typed [`ast::GraphDef`].
///
/// This is the low-level entry used by [`builder::GraphBuilder`] after it has
/// constructed the AST programmatically.  The graph is validated before emission;
/// if validation fails a [`CodegenError::Validate`] is returned.
///
/// # Errors
/// Returns [`CodegenError::Validate`] if the graph is structurally or semantically invalid.
pub fn expand_ast_to_tokens(g: ast::GraphDef) -> Result<TokenStream2, CodegenError> {
    validate::validate_definition(&g).map_err(CodegenError::Validate)?;
    Ok(gen::emit(&g))
}

/// Try to pretty-print tokens; if that fails, fall back to raw `.to_string()`.
fn tokens_to_string_pretty_or_raw(tokens: &TokenStream2) -> String {
    match syn::parse2::<syn::File>(tokens.clone()) {
        Ok(file) => prettyplease::unparse(&file),
        Err(_) => tokens.to_string(),
    }
}

/// Write tokens to a file, preferring pretty-printing with fallback to raw.
pub fn write_tokens_pretty_or_raw<P: AsRef<std::path::Path>>(
    tokens: &TokenStream2,
    dest: P,
) -> Result<std::path::PathBuf, CodegenError> {
    let s = tokens_to_string_pretty_or_raw(tokens);
    let p = dest.as_ref().to_path_buf();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&p, s)?;
    Ok(p)
}

/// Parse, validate, and emit Rust code from a proc-macro input token stream.
///
/// Emits the graph selected by the DSL, plus the optional std-only
/// `ScopedGraphApi` implementation when the trailing `concurrent;`
/// keyword is present.
///
/// This is the entry used by `limen-build::define_graph! { ... }`.
///
/// # Parameters
/// - `input`: Tokens containing exactly one graph DSL definition (see crate-level docs).
///
/// # Returns
/// - `Ok(TokenStream2)`: The generated Rust code for the graph type and its trait implementations.
/// - `Err(CodegenError)`: If parsing, validation, or emission fails.
///
/// # Errors
/// Returns:
/// - [`CodegenError::Parse`] if the input tokens are not a well-formed graph DSL.
/// - [`CodegenError::Validate`] if the graph is structurally or semantically invalid.
pub fn expand_tokens(input: TokenStream2) -> Result<TokenStream2, CodegenError> {
    let g = syn::parse2::<ast::GraphDef>(input)?;
    validate::validate_definition(&g).map_err(CodegenError::Validate)?;
    Ok(gen::emit(&g))
}

/// Parse, validate, and emit Rust code from a DSL string (build script helper).
///
/// Emits the graph, plus the optional std-only `ScopedGraphApi`
/// implementation selected by the `concurrent` keyword in the DSL.
///
/// Typical usage is inside `build.rs`, or in tests that snapshot generated code.
///
/// # Parameters
/// - `spec`: The graph DSL as a UTF-8 string. It must contain exactly one graph definition.
///
/// # Returns
/// - `Ok(TokenStream2)`: The generated Rust code for the graph type and its trait implementations.
/// - `Err(CodegenError)`: If parsing, validation, or emission fails.
///
/// # Errors
/// Returns:
/// - [`CodegenError::Parse`] if the string is not a well-formed graph DSL.
/// - [`CodegenError::Validate`] if the graph is structurally or semantically invalid.
pub fn expand_str_to_tokens(spec: &str) -> Result<TokenStream2, CodegenError> {
    let g = syn::parse_str::<ast::GraphDef>(spec)?;
    validate::validate_definition(&g).map_err(CodegenError::Validate)?;
    Ok(gen::emit(&g))
}

/// Parse, validate, emit, and **pretty-print** the Rust code for a DSL string.
///
/// This is convenient when you want stable, human-readable source for inspection
/// or to write to disk with [`expand_str_to_file`].
///
/// # Parameters
/// - `spec`: The graph DSL as a UTF-8 string. It must contain exactly one graph definition.
///
/// # Returns
/// - `Ok(String)`: Formatted Rust source for the generated graph.
/// - `Err(CodegenError)`: If parsing, validation, emission, or pretty-printing fails.
///
/// # Errors
/// Returns:
/// - [`CodegenError::Parse`] or [`CodegenError::Validate`] as above.
/// - [`CodegenError::Pretty`] if formatting the generated tokens as a Rust file fails.
pub fn expand_str_to_string(spec: &str) -> Result<String, CodegenError> {
    let tokens = expand_str_to_tokens(spec)?;
    Ok(tokens_to_string_pretty_or_raw(&tokens))
}

/// Parse, validate, emit, pretty-print, and **write** the Rust code for a DSL
/// string to `dest`. Parent directories are created if needed, and writes are
/// performed atomically.
///
/// This helper creates parent directories if needed, writes atomically to `dest`, and returns
/// the resolved path. It is ideal for use in `build.rs`, where you can later `include!()` the file.
///
/// # Parameters
/// - `spec`: The graph DSL as a UTF-8 string. It must contain exactly one graph definition.
/// - `dest`: Destination filesystem path for the generated Rust source file.
///
/// # Returns
/// - `Ok(PathBuf)`: The absolute path that was written.
/// - `Err(CodegenError)`: If parsing, validation, emission, pretty-printing, or I/O fails.
///
/// # Errors
/// Returns:
/// - [`CodegenError::Parse`] or [`CodegenError::Validate`] as above.
/// - [`CodegenError::Pretty`] if formatting the generated tokens as a Rust file fails.
/// - [`CodegenError::Io`] if filesystem operations fail (for example, permission denied).
pub fn expand_str_to_file<P: AsRef<Path>>(spec: &str, dest: P) -> Result<PathBuf, CodegenError> {
    let tokens = expand_str_to_tokens(spec)?;
    write_tokens_pretty_or_raw(&tokens, dest)
}

/// Validate, emit, pretty-print, and **write** a typed [`ast::GraphDef`] to `dest`.
///
/// Combines [`expand_ast_to_tokens`] with [`write_tokens_pretty_or_raw`].
/// Parent directories are created if needed.
///
/// # Parameters
/// - `g`: The graph AST, typically produced by [`builder::GraphBuilder`].
/// - `dest`: Destination filesystem path for the generated Rust source file.
///
/// # Returns
/// - `Ok(PathBuf)`: The absolute path that was written.
/// - `Err(CodegenError)`: If validation, emission, or I/O fails.
///
/// # Errors
/// Returns [`CodegenError::Validate`], or [`CodegenError::Io`] if filesystem
/// operations fail (for example, permission denied or out of disk space).
pub fn expand_ast_to_file<P: AsRef<Path>>(
    g: ast::GraphDef,
    dest: P,
) -> Result<PathBuf, CodegenError> {
    let tokens = expand_ast_to_tokens(g)?;
    write_tokens_pretty_or_raw(&tokens, dest)
}
