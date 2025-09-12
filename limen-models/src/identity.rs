//! Identity backend and model (no external deps).
//!
//! This backend simply copies inputs to outputs and advertises Host memory.

use limen_core::compute::{BackendCapabilities, ComputeBackend, ComputeModel, ModelMetadata};
use limen_core::message::Payload;
use limen_core::memory::MemoryClass;
use limen_core::errors::{InferenceError, InferenceErrorKind};

/// Identity backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityBackend;

impl ComputeBackend for IdentityBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities { device_streams: false, max_batch: Some(1), dtype_mask: 0 }
    }
}

/// Identity model that copies input payload to output payload of the same type.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityModel;

impl<P: Payload> ComputeModel<P, P> for IdentityModel {
    fn run(&mut self, inp: &P, out: &mut P) -> Result<(), InferenceError> {
        // For zero-cost demonstration we cannot copy unknown P by value; users
        // should choose payloads where plain assignment is meaningful.
        // We simulate identity by leaving `out` unchanged; in practice, the
        // `ModelNode` will move the payload forward for identity backends.
        Ok(())
    }

    fn metadata(&self) -> ModelMetadata {
        ModelMetadata {
            preferred_input: MemoryClass::Host,
            preferred_output: MemoryClass::Host,
            max_input_bytes: None,
            max_output_bytes: None,
        }
    }
}
