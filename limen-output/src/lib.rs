#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Output SinkNodes for Limen.
//!
//! - [`stdout::StdoutSink`]: prints payloads with a configurable prefix.
//! - [`file::FileSink`]: appends payloads to a file (std).

pub mod stdout;
#[cfg(feature = "std")]
pub mod file;
