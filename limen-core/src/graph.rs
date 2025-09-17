//! Graph contracts, descriptors, and a builder for wiring arbitrary topologies.
//!
//! # Layers
//!
//! - [`descriptor`]: storage-only descriptors (borrowed, owned, and const-buffer).
//! - [`validate`]: descriptor validators (no-alloc port checks; acyclicity with/without `alloc`).
//! - [`builder`] (alloc): ergonomic builder that produces a validated owned descriptor.
//!
//! Runtimes consume the **typed** `Graph` trait; tooling and codegen use descriptors.

pub mod descriptor;
pub mod validate;

#[cfg(feature = "alloc")]
pub mod builder;
