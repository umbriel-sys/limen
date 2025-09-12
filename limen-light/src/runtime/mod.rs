//! P0 and P1 runtimes.
//!
//! The runtimes orchestrate stepping your statically-typed graph by calling a
//! `Graph` trait you implement (or generate). This keeps `limen-core` clean and
//! avoids dynamic dispatch.

pub mod p0;
#[cfg(feature = "alloc")]
pub mod p1;
