//! Graph descriptors, port indices, and validation contracts.
//!
//! The core does not implement dynamic graph construction. Runtimes or code
//! generators build typed graphs and may use these descriptors for validation.

use crate::errors::GraphError;
use crate::memory::{PlacementAcceptance};
use crate::policy::{EdgePolicy};
use crate::types::{NodeIndex, PortIndex};

/// A port schema describes payload type information in an implementation-defined way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortSchema {
    /// Whether the port accepts zero-copy for specific placements.
    pub acceptance: PlacementAcceptance,
    /// Implementation-defined payload metadata (opaque to core).
    pub meta_flags: u32,
}

/// An edge descriptor couples an output port of one node to an input port of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeDescriptor {
    /// Source node index.
    pub from: NodeIndex,
    /// Source output port index.
    pub from_port: PortIndex,
    /// Destination node index.
    pub to: NodeIndex,
    /// Destination input port index.
    pub to_port: PortIndex,
    /// Policy for the edge (caps, admission, over-budget behavior).
    pub policy: EdgePolicy,
}

/// An interface for graph validation.
pub trait GraphValidator {
    /// Validate acyclicity, port compatibility, and capacity sanity.
    fn validate(&self) -> Result<(), GraphError>;
}
