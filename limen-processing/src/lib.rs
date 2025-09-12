#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Processing nodes and operators for Limen.
//!
//! Provided nodes (POC set):
//! - [`identity::IdentityNode`]
//! - [`math::NormalizeNode`]
//! - [`math::MovingAverageNode`]
//! - [`logic::ThresholdNode`]
//! - [`logic::DebounceNode`]
//! - [`logic::ArgmaxClassifyNode`]
//!
//! Provided payloads:
//! - [`payload::Tensor1D<T, N>`], a typed 1-D array with a `Payload` impl.
//! - [`payload::Label`], a single-byte class label payload.
//!
//! Operators:
//! - [`ops::CopySplit`], [`ops::ScatterSplit`], [`ops::ConcatJoin`], [`ops::SumJoin`]

extern crate alloc;

pub mod payload;
pub mod identity;
pub mod math;
pub mod logic;
pub mod ops;
