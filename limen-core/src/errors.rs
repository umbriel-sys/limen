//! Error families used across Limen Core.
//!
//! Errors are designed to be allocation-free in P0, bounded in P1, and richer
//! in P2. The types here avoid `std::error::Error` unless the `std` feature is
//! enabled.

use core::fmt;

#[cfg(feature = "std")]
use std::error::Error;

use crate::types::EdgeIndex;

// **** Edge Errors *****

/// Errors originating from queue operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// The queue is at or above the hard watermark capacity.
    AtOrAboveHardCap,
    /// The queue is backpressured but not full; caller may retry later.
    Backpressured,
    /// The queue is empty when a pop operation was requested.
    Empty,
    /// The queue lock has been poisoned (concurrent mode only).
    Poisoned,
}

impl fmt::Display for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueueError::AtOrAboveHardCap => {
                f.write_str("queue is at or above the hard watermark capacity")
            }
            QueueError::Backpressured => {
                f.write_str("queue is backpressured but not full; caller may retry later")
            }
            QueueError::Empty => f.write_str("queue is empty"),
            QueueError::Poisoned => f.write_str("queue lock is poisoned"),
        }
    }
}

#[cfg(feature = "std")]
impl Error for QueueError {}

// ***** Node Errors *****

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

impl fmt::Display for NodeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeErrorKind::NoInput => {
                f.write_str("inputs were not available to progress this node")
            }
            NodeErrorKind::Backpressured => {
                f.write_str("outputs could not be enqueued due to backpressure")
            }
            NodeErrorKind::OverBudget => {
                f.write_str("an execution budget or deadline was exceeded")
            }
            NodeErrorKind::ExternalUnavailable => {
                f.write_str("an external dependency was unavailable or timed out")
            }
            NodeErrorKind::ExecutionFailed => {
                f.write_str("a generic failure occurred in node logic")
            }
        }
    }
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

impl fmt::Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "node error: {} (code: {})", self.kind, self.code)
    }
}

#[cfg(feature = "std")]
impl Error for NodeError {}

impl NodeError {
    /// Creates a `NoInput` error.
    #[inline]
    pub const fn no_input() -> Self {
        Self::new(NodeErrorKind::NoInput, 0)
    }
    /// Creates a `Backpressured` error.
    #[inline]
    pub const fn backpressured() -> Self {
        Self::new(NodeErrorKind::Backpressured, 0)
    }
    /// Creates an `OverBudget` error.
    #[inline]
    pub const fn over_budget() -> Self {
        Self::new(NodeErrorKind::OverBudget, 0)
    }
    /// Creates an `ExternalUnavailable` error.
    #[inline]
    pub const fn external_unavailable() -> Self {
        Self::new(NodeErrorKind::ExternalUnavailable, 0)
    }
    /// Creates an `ExecutionFailed` error.
    #[inline]
    pub const fn execution_failed() -> Self {
        Self::new(NodeErrorKind::ExecutionFailed, 0)
    }

    /// Same as the above but lets you tack on a backend/platform error code.
    #[inline]
    pub const fn with_code(mut self, code: u32) -> Self {
        self.code = code;
        self
    }
}

impl From<NodeErrorKind> for NodeError {
    #[inline]
    fn from(kind: NodeErrorKind) -> Self {
        NodeError::new(kind, 0)
    }
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

impl fmt::Display for InferenceErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InferenceErrorKind::InvalidArtifact => {
                f.write_str("model artifact is invalid or unsupported")
            }
            InferenceErrorKind::ShapeOrTypeMismatch => {
                f.write_str("input or output payload is incompatible with the model")
            }
            InferenceErrorKind::ExecutionFailed => {
                f.write_str("execution failed inside the backend")
            }
            InferenceErrorKind::ResourceUnavailable => {
                f.write_str("backend resource is unavailable")
            }
        }
    }
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

impl fmt::Display for InferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "inference error: {} (code: {})", self.kind, self.code)
    }
}

#[cfg(feature = "std")]
impl Error for InferenceError {}

// ***** Graph Errors *****

/// Graph validation and wiring errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphError {
    /// The graph contains a cycle.
    // TODO: ENABLE?
    Cyclic,
    /// Port schema or memory placement is incompatible across an edge.
    IncompatiblePorts,
    /// Queue capacity or watermark configuration is invalid.
    InvalidCapacity,
    /// Invalid graph index used.
    InvalidEdgeIndex,
    /// Failed to sample occupancy for the given edge (e.g., poisoned lock or device error).
    OccupancySampleFailed(EdgeIndex),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::Cyclic => f.write_str("graph contains a cycle"),
            GraphError::IncompatiblePorts => {
                f.write_str("port schema or memory placement is incompatible across an edge")
            }
            GraphError::InvalidCapacity => {
                f.write_str("queue capacity or watermark configuration is invalid")
            }
            GraphError::InvalidEdgeIndex => f.write_str("edge index is invalid"),
            GraphError::OccupancySampleFailed(ei) => {
                write!(f, "failed to sample occupancy for edge {}", ei.0)
            }
        }
    }
}

#[cfg(feature = "std")]
impl Error for GraphError {}

// ***** Runtime Errors *****

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

impl fmt::Display for RuntimeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeErrorKind::InvariantViolation => f.write_str("an invariant has been violated"),
            RuntimeErrorKind::PlatformUnavailable => {
                f.write_str("a requested platform service is unavailable")
            }
            RuntimeErrorKind::Unsupported => {
                f.write_str("the operation is unsupported in this profile or configuration")
            }
            RuntimeErrorKind::Unknown => f.write_str("an unspecified failure occurred"),
        }
    }
}

#[cfg(feature = "std")]
impl Error for RuntimeErrorKind {}

/// Scheduler-related errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerError {
    /// The scheduler cannot proceed due to an invariant violation.
    InvariantViolation,
    /// An internal error occurred.
    Internal,
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchedulerError::InvariantViolation => {
                f.write_str("the scheduler cannot proceed due to an invariant violation")
            }
            SchedulerError::Internal => f.write_str("an internal scheduler error occurred"),
        }
    }
}

#[cfg(feature = "std")]
impl Error for SchedulerError {}

// ***** Source Errors *****

/// Source / sensor related errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorError {
    /// Sensor open failed.
    OpenFailed,
    /// Sensor read failed.
    ReadFailed,
    /// Sensor stream ended.
    EndOfStream,
    /// Sensor reset failed.
    ResetFailed,
    /// Invalid sensor configuration
    ConfigurationInvalid,
}

impl fmt::Display for SensorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SensorError::OpenFailed => f.write_str("sensor open failed"),
            SensorError::ReadFailed => f.write_str("sensor read failed"),
            SensorError::EndOfStream => f.write_str("sensor stream ended"),
            SensorError::ResetFailed => f.write_str("sensor reset failed"),
            SensorError::ConfigurationInvalid => f.write_str("invalid sensor configuration"),
        }
    }
}

#[cfg(feature = "std")]
impl Error for SensorError {}

// ***** Sink Errors *****

/// Output / sink related errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputError {
    /// Sink write failed.
    WriteFailed,
    /// Sink flush failed.
    FlushFailed,
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputError::WriteFailed => f.write_str("sink write failed"),
            OutputError::FlushFailed => f.write_str("sink flush failed"),
        }
    }
}

#[cfg(feature = "std")]
impl Error for OutputError {}

// ***** Runtime Errors *****

/// Error surface for runtimes: can wrap graph- and node-level errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeError {
    /// Graph errors
    Graph(GraphError),
    /// Node errors
    Node(NodeError),
}

impl From<GraphError> for RuntimeError {
    #[inline]
    fn from(e: GraphError) -> Self {
        RuntimeError::Graph(e)
    }
}

impl From<NodeError> for RuntimeError {
    #[inline]
    fn from(e: NodeError) -> Self {
        RuntimeError::Node(e)
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::Graph(e) => write!(f, "runtime graph error: {e}"),
            RuntimeError::Node(e) => write!(f, "runtime node error: {e}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RuntimeError {}
