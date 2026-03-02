//! Compute backend and model traits (dyn-free, explicit; no defaults).
//!
//! Backends implement `ComputeBackend<InP, OutP>` and return a concrete `Model`
//! that implements `ComputeModel<InP, OutP>`. The hot path is `infer_one`,
//! which performs exactly one synchronous inference step for a single input.
//!
//! Backends are monomorphized by generics (no dynamic dispatch). The model API
//! is intentionally decoupled from graph/queue details so nodes own batching,
//! backpressure, and telemetry.

use crate::errors::InferenceError;
use crate::memory::MemoryClass;
use crate::message::payload::Payload;
use crate::prelude::Batch;

/// Capability descriptor of a compute backend.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// Whether the backend supports device streams (async/event completion).
    device_streams: bool,
    /// Maximum supported batch size, if any.
    max_batch: Option<usize>,
    /// Bitfield for supported data types (backend-defined; optional use).
    dtype_mask: u64,
}

impl BackendCapabilities {
    /// Create a new `BackendCapabilities`.
    #[inline]
    pub fn new(device_streams: bool, max_batch: Option<usize>, dtype_mask: u64) -> Self {
        Self {
            device_streams,
            max_batch,
            dtype_mask,
        }
    }

    /// Whether the backend supports device streams (async/event completion).
    #[inline]
    pub fn device_streams(&self) -> &bool {
        &self.device_streams
    }

    /// Maximum supported batch size, if any.
    #[inline]
    pub fn max_batch(&self) -> &Option<usize> {
        &self.max_batch
    }

    /// Bitfield for supported data types (backend-defined; optional use).
    #[inline]
    pub fn dtype_mask(&self) -> &u64 {
        &self.dtype_mask
    }
}

/// Model metadata describing input/output shapes and preferences.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelMetadata {
    /// Preferred input memory class (Host/Pinned/Device).
    preferred_input: MemoryClass,
    /// Preferred output memory class.
    preferred_output: MemoryClass,
    /// Optional maximum input size in bytes (admission hint).
    max_input_bytes: Option<usize>,
    /// Optional maximum output size in bytes.
    max_output_bytes: Option<usize>,
}

impl ModelMetadata {
    /// Create a new `ModelMetadata`.
    #[inline]
    pub fn new(
        preferred_input: MemoryClass,
        preferred_output: MemoryClass,
        max_input_bytes: Option<usize>,
        max_output_bytes: Option<usize>,
    ) -> Self {
        Self {
            preferred_input,
            preferred_output,
            max_input_bytes,
            max_output_bytes,
        }
    }

    /// Preferred input memory class (Host/Pinned/Device).
    #[inline]
    pub fn preferred_input(&self) -> &MemoryClass {
        &self.preferred_input
    }

    /// Preferred output memory class.
    #[inline]
    pub fn preferred_output(&self) -> &MemoryClass {
        &self.preferred_output
    }

    /// Optional maximum input size in bytes (admission hint).
    #[inline]
    pub fn max_input_bytes(&self) -> &Option<usize> {
        &self.max_input_bytes
    }

    /// Optional maximum output size in bytes.
    #[inline]
    pub fn max_output_bytes(&self) -> &Option<usize> {
        &self.max_output_bytes
    }
}

/// A loaded model that can perform inference.
pub trait ComputeModel<InP: Payload, OutP: Payload> {
    /// Prepare internal state (allocate work buffers, compile kernels, etc.).
    fn init(&mut self) -> Result<(), InferenceError>;

    /// Single-item inference (1×1).
    fn infer_one(&mut self, inp: &InP, out: &mut OutP) -> Result<(), InferenceError>;

    /// Optional: batched inference. Default loops `infer_one`.
    #[inline]
    fn infer_batch(
        &mut self,
        inps: Batch<'_, InP>,
        outs: &mut [OutP],
    ) -> Result<(), InferenceError> {
        // Default: call infer_one using references into the Batch's messages.
        for (m, o) in inps.messages().iter().zip(outs.iter_mut()) {
            // Message::payload() returns &InP so we pass a reference, no move/clones.
            self.infer_one(m.payload(), o)?;
        }
        Ok(())
    }

    /// Ensure outstanding device work is complete (if any).
    fn drain(&mut self) -> Result<(), InferenceError>;

    /// Reset internal state to a known baseline (drop caches, etc.).
    fn reset(&mut self) -> Result<(), InferenceError>;

    /// Return model metadata (I/O placement preferences, limits).
    fn metadata(&self) -> ModelMetadata;
}

/// A dyn-free engine that constructs models and reports capabilities.
pub trait ComputeBackend<InP: Payload, OutP: Payload> {
    /// Concrete model type (no trait objects).
    type Model: ComputeModel<InP, OutP>;

    /// Backend-specific error.
    type Error;

    /// Backend-chosen borrowed descriptor used to load a model.
    ///
    /// Examples:
    /// - on `std`:    `type ModelDescriptor<'desc> = &'desc ModelArtifact;`
    /// - on `no_std`: `type ModelDescriptor<'desc> = &'desc [u8];`
    type ModelDescriptor<'desc>
    where
        Self: 'desc;

    /// Capability report.
    fn capabilities(&self) -> BackendCapabilities;

    /// Load a model from a descriptor.
    fn load_model<'desc>(
        &self,
        desc: Self::ModelDescriptor<'desc>,
    ) -> Result<Self::Model, Self::Error>;
}

/// A simple artifact passed to backends for model creation (POC-friendly).
#[cfg(feature = "std")]
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ModelArtifact {
    /// Raw bytes of a model file or an engine-specific blob.
    bytes: std::sync::Arc<Vec<u8>>,
    /// Optional label or path hint.
    label: Option<String>,
}

#[cfg(feature = "std")]
impl ModelArtifact {
    /// Construct from an Arc of bytes and an optional label.
    #[inline]
    pub fn new(bytes: std::sync::Arc<Vec<u8>>, label: Option<String>) -> Self {
        Self { bytes, label }
    }

    /// Construct from raw bytes.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            bytes: std::sync::Arc::new(bytes),
            label: None,
        }
    }

    /// Convenience: load from a file path.
    pub fn from_file<P: AsRef<std::path::Path>>(path: P) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        Ok(Self::from_bytes(bytes))
    }

    /// Access the bytes (cloned Arc).
    #[inline]
    pub fn bytes(&self) -> std::sync::Arc<Vec<u8>> {
        self.bytes.clone()
    }

    /// Access the optional label as `Option<&str>`.
    #[inline]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}
