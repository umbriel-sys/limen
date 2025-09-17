//! Compute backend and model traits.
//!
//! These traits allow `limen-models` to compose existing inference engines
//! (Tract, TFLite, ORT, vendor DSP) **without** dynamic dispatch in the hot path.
//! Adapters are monomorphized by generics and hidden behind feature gates in
//! the `limen-models` crate.

use crate::errors::InferenceError;
use crate::memory::MemoryClass;
use crate::message::payload::Payload;

/// Capability descriptor of a compute backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// Whether the backend supports device streams (async/event completion).
    pub device_streams: bool,
    /// Maximum supported batch size, if any.
    pub max_batch: Option<usize>,
    /// Bitfield for supported data types (backend-defined; optional use).
    pub dtype_mask: u64,
}

/// Model metadata describing input/output shapes and preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelMetadata {
    /// Preferred input memory class (Host/Pinned/Device).
    pub preferred_input: MemoryClass,
    /// Preferred output memory class.
    pub preferred_output: MemoryClass,
    /// Optional maximum input size in bytes (admission hint).
    pub max_input_bytes: Option<usize>,
    /// Optional maximum output size in bytes.
    pub max_output_bytes: Option<usize>,
}

/// A loaded model that can be executed synchronously or via streams.
pub trait ComputeModel<InP: Payload, OutP: Payload> {
    /// Execute the model synchronously: `out` becomes the model output for `inp`.
    fn run(&mut self, inp: &InP, out: &mut OutP) -> Result<(), InferenceError>;

    /// Return model metadata.
    fn metadata(&self) -> ModelMetadata;
}

/// An engine that can load/construct models and provide memory hooks.
pub trait ComputeBackend {
    /// Return the backend capabilities.
    fn capabilities(&self) -> BackendCapabilities;
}

/// A simple artifact passed to backends for model creation (POC-friendly).
#[cfg(feature = "std")]
#[derive(Debug, Clone)]
pub struct ModelArtifact {
    /// Raw bytes of a model file or an engine-specific blob.
    pub bytes: std::sync::Arc<Vec<u8>>,
    /// Optional label or path hint.
    pub label: Option<String>,
}

#[cfg(feature = "std")]
impl ModelArtifact {
    /// Construct from raw bytes.
    // TODO: FIX!
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            bytes: std::sync::Arc::new(bytes),
            label: None,
        }
    }
}
