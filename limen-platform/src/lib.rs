#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Platform adapters for Limen.
//!
//! Initial module provides a Linux/desktop `StdClock` compatible with `limen-core`.

pub mod linux;
