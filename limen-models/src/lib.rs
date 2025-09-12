#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Compute backends and model nodes for Limen.
//!
//! - [`identity`]: a pure-Rust identity backend useful for tests and examples.
//! - [`nodes::ModelNode`]: a generic node wrapper that executes a `ComputeModel`.
//! - `backend-tract` (feature, POC): structure for integrating Tract (not enabled by default).

pub mod identity;
pub mod nodes;

pub use limen_core as core;
pub use limen_core::compute as compute;
