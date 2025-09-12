#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Sensor SourceNodes for Limen.
//!
//! - [`simulated::SimulatedSource1D`]: produces synthetic `Tensor1D<f32, N>` payloads.
//! - [`csvio::CsvSource1D`]: reads CSV lines into `Tensor1D<f32, N>` (std only).

pub mod simulated;
#[cfg(feature = "std")]
pub mod csvio;
