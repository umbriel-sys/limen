#![warn(missing_docs)]
#![deny(unsafe_code)]
//! # limen-build — proc-macro front for `limen-codegen`
//!
//! Provides the `define_graph! { ... }` macro which parses the Limen graph DSL
//! and expands it into fully-typed Rust code via `limen-codegen`.
//!
//! See `limen-codegen` crate docs for the DSL shape and the exact generated API.

extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::{Literal, TokenStream as TokenStream2};
use quote::quote;

/// Define a Limen graph using the DSL and expand it into generated Rust code.
///
/// This macro forwards its input tokens directly to
/// [`limen_codegen::expand_tokens`] and returns the emitted items.
///
/// Each invocation emits exactly **one** graph flavor. Use the trailing
/// `concurrent;` keyword to select the `std`-gated concurrent flavor.
/// To produce both flavors, call `define_graph!` twice with different
/// struct names (gate the concurrent call with `#[cfg(feature = "std")]`).
///
/// # Example (non-std only)
/// ```ignore
/// use limen_build::define_graph;
///
/// define_graph! {
///     pub struct MyGraph;
///
///     nodes {
///         0: { ty: my::Src, in_ports: 0, out_ports: 1, in_payload: (),  out_payload: u32, name: Some("src"), ingress_policy: MY_Q },
///         1: { ty: my::Map, in_ports: 1, out_ports: 1, in_payload: u32, out_payload: u32, name: Some("map") },
///         2: { ty: my::Sink, in_ports: 1, out_ports: 0, in_payload: u32, out_payload: (),  name: Some("sink") },
///     }
///
///     edges {
///         0: { ty: TestSpscRingBuf<8>, payload: u32, manager: StaticMemoryManager<u32, 8>, from: (0, 0), to: (1, 0), policy: EDGE_POL, name: Some("src->map") },
///         1: { ty: TestSpscRingBuf<8>, payload: u32, manager: StaticMemoryManager<u32, 8>, from: (1, 0), to: (2, 0), policy: EDGE_POL, name: Some("map->sink") },
///     }
/// }
///
/// // Concurrent flavor (only compiled under `std` feature):
/// #[cfg(feature = "std")]
/// define_graph! {
///     pub struct MyGraph;   // will produce MyGraphStd inside concurrent_graph module
///
///     nodes { /* same as above */ }
///     edges {
///         0: { ty: TestSpscRingBuf<8>, payload: u32, manager: ConcurrentMemoryManager<u32>, from: (0, 0), to: (1, 0), policy: EDGE_POL, name: Some("src->map") },
///         1: { ty: TestSpscRingBuf<8>, payload: u32, manager: ConcurrentMemoryManager<u32>, from: (1, 0), to: (2, 0), policy: EDGE_POL, name: Some("map->sink") },
///     }
///     concurrent;
/// }
/// ```
#[proc_macro]
pub fn define_graph(input: TokenStream) -> TokenStream {
    let tokens2 = TokenStream2::from(input);
    match limen_codegen::expand_tokens(tokens2) {
        Ok(out) => out.into(),
        Err(err) => error_to_tokens(err).into(),
    }
}

/// Convert a `CodegenError` into a compile-time error token stream.
///
/// - For parse errors, preserve spans using `syn::Error::to_compile_error`.
/// - For other errors, use `compile_error!("<message>")`.
fn error_to_tokens(err: limen_codegen::CodegenError) -> TokenStream2 {
    match err {
        limen_codegen::CodegenError::Parse(e) => e.to_compile_error(),
        other => {
            let lit = Literal::string(&other.to_string());
            quote! { compile_error!(#lit); }
        }
    }
}
