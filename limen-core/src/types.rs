//! Small shared types and identifiers used throughout the core.

/// A 64-bit trace identifier used to correlate messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceId(pub u64);

/// A 64-bit sequence number assigned by sources or routers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SequenceNumber(pub u64);

/// Monotonic tick unit from the platform clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticks(pub u64);

/// Absolute deadline in nanoseconds since platform boot (or epoch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeadlineNs(pub u64);

/// Quality-of-Service class label attached to messages and used by admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QoSClass {
    /// Latency-critical traffic; favored by EDF schedulers.
    LatencyCritical,
    /// Default best-effort traffic.
    BestEffort,
    /// Background/low-priority traffic.
    Background,
}

/// A logical index for input or output ports on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortIndex(pub usize);

/// A logical index of a node in a graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeIndex(pub usize);
