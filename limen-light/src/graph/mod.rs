//! Graph integration traits and simple linear pipeline helpers.
//!
//! To run a statically wired DAG on `limen-light`, implement either
//! [`runtime::p0::GraphP0`] or [`runtime::p1::GraphP1`] for your graph type.
//!
//! This module also provides a **simple 5-stage pipeline** helper for the
//! common pattern: `Source -> Pre -> Model -> Post -> Sink` with 1 input / 1
//! output ports (except the Source and Sink). This demonstrates how to build
//! `StepContext`s and wire queues without dynamic dispatch.

pub mod simple_chain;
