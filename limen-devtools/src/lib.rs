//! Devtools: minimal code generation and test utilities (POC).
//!
//! The code generator ingests a JSON/TOML config describing a simple 5-stage
//! pipeline and emits a typed Rust module that can be compiled into an app.

use serde::{Deserialize, Serialize};

/// A minimal config describing a linear chain with compile-time constants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleChainConfig {
    /// Number of tensor elements for the 1D payload.
    pub tensor_len: usize,
    /// Moving average window size.
    pub movavg_win: usize,
    /// Threshold index and value.
    pub threshold_index: usize,
    pub threshold_value: f32,
}

/// Render a Rust module implementing a typed SimpleChain5 for the given config.
pub fn render_simple_chain_module(cfg: &SimpleChainConfig) -> String {
    format!(r#"
pub mod generated {{
    use limen_processing::payload::Tensor1D;
    use limen_processing::math::{{NormalizeNode, MovingAverageNode}};
    use limen_processing::logic::{{ThresholdNode, DebounceNode}};
    use limen_processing::identity::IdentityNode;
    use limen_models::identity::{{IdentityBackend, IdentityModel}};
    use limen_models::nodes::ModelNode;
    use limen_sensors::simulated::SimulatedSource1D;
    use limen_output::stdout::StdoutSink;
    use limen::spsc::lockfree_ring::LockFreeRing;
    use limen_core::message::Message;
    use limen_core::policy::EdgePolicy;

    pub const N: usize = {n};
    pub const W: usize = {w};

    pub type P1 = Tensor1D<f32, N>;
    pub type P2 = Tensor1D<f32, N>;
    pub type P3 = Tensor1D<f32, N>;
    pub type P4 = Tensor1D<f32, N>;

    pub type Q1 = LockFreeRing<Message<P1>>;
    pub type Q2 = LockFreeRing<Message<P2>>;
    pub type Q3 = LockFreeRing<Message<P3>>;
    pub type Q4 = LockFreeRing<Message<P4>>;

    pub fn make_policies(default: EdgePolicy) -> (EdgePolicy, EdgePolicy, EdgePolicy, EdgePolicy) {{
        (default, default, default, default)
    }}
}}
"#, n = cfg.tensor_len, w = cfg.movavg_win)
}
