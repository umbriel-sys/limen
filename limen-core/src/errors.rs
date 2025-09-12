//! Error families used across Limen Core.
//!
//! Errors are designed to be allocation-free in P0, bounded in P1, and richer
//! in P2. The types here avoid `std::error::Error` unless the `std` feature is
//! enabled.

/// Generic runtime error kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeErrorKind {
    /// An invariant has been violated (e.g., cyclic graph or type mismatch).
    InvariantViolation,
    /// A platform service was requested but is unavailable.
    PlatformUnavailable,
    /// The operation is unsupported in this profile or configuration.
    Unsupported,
    /// An unspecified failure occurred.
    Unknown,
}

/// Errors originating from queue operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// The queue is at or above the hard watermark capacity.
    AtOrAboveHardCap,
    /// The queue is backpressured but not full; caller may retry later.
    Backpressured,
    /// The queue is empty when a pop operation was requested.
    Empty,
}

/// Errors from node execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeErrorKind {
    /// Inputs were not available to progress this node.
    NoInput,
    /// Outputs could not be enqueued due to backpressure.
    Backpressured,
    /// An execution budget or deadline was exceeded.
    OverBudget,
    /// External dependency (device, transport) was unavailable or timed out.
    ExternalUnavailable,
    /// A generic failure in node logic.
    ExecutionFailed,
}

/// A unified error used by node lifecycle methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeError {
    /// The error kind.
    pub kind: NodeErrorKind,
    /// Optional numeric code for platform/backend-specific mapping.
    pub code: u32,
}

impl NodeError {
    /// Construct a new node error with the given kind and optional code.
    pub const fn new(kind: NodeErrorKind, code: u32) -> Self {
        Self { kind, code }
    }
}

/// Scheduler-related errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerError {
    /// The scheduler cannot proceed due to an invariant violation.
    InvariantViolation,
    /// An internal error occurred.
    Internal,
}

/// Graph validation and wiring errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphError {
    /// The graph contains a cycle.
    // TODO: ENABLE!
    Cyclic,
    /// Port schema or memory placement is incompatible across an edge.
    IncompatiblePorts,
    /// Queue capacity or watermark configuration is invalid.
    InvalidCapacity,
}

/// Errors related to model loading and inference execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceErrorKind {
    /// A model artifact is invalid or unsupported.
    InvalidArtifact,
    /// The input or output payload is incompatible with the model.
    ShapeOrTypeMismatch,
    /// Execution failed inside the backend.
    ExecutionFailed,
    /// Backend resource not available (e.g., device).
    ResourceUnavailable,
}

/// Inference error including a kind and optional code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferenceError {
    /// Error kind.
    pub kind: InferenceErrorKind,
    /// Optional numeric code.
    pub code: u32,
}

impl InferenceError {
    /// Construct a new inference error.
    pub const fn new(kind: InferenceErrorKind, code: u32) -> Self {
        Self { kind, code }
    }
}
