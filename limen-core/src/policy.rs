//! Policies for batching, budgets, deadlines, admission, and capacities.

use crate::types::{DeadlineNs, QoSClass, Ticks};

/// Batch formation policy: fixed-N and/or Δt micro-batching.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchingPolicy {
    /// Fixed number of items per batch (>= 1). If `None`, do not use fixed-N.
    fixed_n: Option<usize>,
    /// Maximum micro-batching window in ticks (Δt). If `None`, no Δt cap.
    max_delta_t: Option<Ticks>,
}

impl BatchingPolicy {
    /// No batching (batch size 1).
    pub const fn none() -> Self {
        Self {
            fixed_n: Some(1),
            max_delta_t: None,
        }
    }

    /// Fixed-N batching.
    pub const fn fixed(n: usize) -> Self {
        Self {
            fixed_n: Some(n),
            max_delta_t: None,
        }
    }

    /// Δt-bounded micro-batching (runtime may still emit batch < N if time cap hits).
    pub const fn delta_t(cap: Ticks) -> Self {
        Self {
            fixed_n: None,
            max_delta_t: Some(cap),
        }
    }

    /// Combined fixed-N and Δt caps.
    pub const fn fixed_and_delta_t(n: usize, cap: Ticks) -> Self {
        Self {
            fixed_n: Some(n),
            max_delta_t: Some(cap),
        }
    }

    /// Borrow the fixed-N batching value (if any).
    #[inline]
    pub const fn fixed_n(&self) -> &Option<usize> {
        &self.fixed_n
    }

    /// Borrow the delta-t cap (if any).
    #[inline]
    pub const fn max_delta_t(&self) -> &Option<Ticks> {
        &self.max_delta_t
    }
}

impl Default for BatchingPolicy {
    fn default() -> Self {
        BatchingPolicy::none()
    }
}

/// Budget policy for node execution.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BudgetPolicy {
    /// Per-step tick budget; exceeding invokes over-budget action.
    tick_budget: Option<Ticks>,
    /// *Hard* guardrail for a single step. If exceeded, the runtime may call
    /// `Node::on_watchdog_timeout` and/or apply `OverBudgetAction`.
    ///
    /// Keep in no_std: only requires a monotonic clock; no OS timers.
    watchdog_ticks: Option<Ticks>,
}

impl BudgetPolicy {
    /// Construct a new budget policy.
    pub const fn new(tick_budget: Option<Ticks>, watchdog_ticks: Option<Ticks>) -> Self {
        Self {
            tick_budget,
            watchdog_ticks,
        }
    }

    /// Borrow the tick budget.
    #[inline]
    pub const fn tick_budget(&self) -> &Option<Ticks> {
        &self.tick_budget
    }

    /// Borrow the watchdog ticks.
    #[inline]
    pub const fn watchdog_ticks(&self) -> &Option<Ticks> {
        &self.watchdog_ticks
    }
}

/// Deadline policy for messages processed by a node.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeadlinePolicy {
    /// Whether to require absolute deadlines on inputs (P2).
    require_absolute_deadline: bool,
    /// Optional slack tolerance (ns) before defaulting/degrading.
    slack_tolerance_ns: Option<DeadlineNs>,
    /// Synthesize a deadline when inputs have none: absolute_deadline = now + value.
    /// Leave `None` to avoid synthesizing (strict mode or non-EDF).
    default_deadline_ns: Option<DeadlineNs>,
}

impl DeadlinePolicy {
    /// Construct a deadline policy.
    pub const fn new(
        require_absolute_deadline: bool,
        slack_tolerance_ns: Option<DeadlineNs>,
        default_deadline_ns: Option<DeadlineNs>,
    ) -> Self {
        Self {
            require_absolute_deadline,
            slack_tolerance_ns,
            default_deadline_ns,
        }
    }

    /// Borrow the require_absolute_deadline bool.
    #[inline]
    pub const fn require_absolute_deadline(&self) -> bool {
        self.require_absolute_deadline
    }

    /// Borrow the slack_tolerance_ns.
    #[inline]
    pub const fn slack_tolerance_ns(&self) -> Option<DeadlineNs> {
        self.slack_tolerance_ns
    }

    /// Borrow the default_deadline_ns.
    #[inline]
    pub const fn default_deadline_ns(&self) -> Option<DeadlineNs> {
        self.default_deadline_ns
    }
}

/// Action to take when budgets or deadlines are breached.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverBudgetAction {
    /// Drop the message(s).
    Drop,
    /// Skip the stage and signal upstream (if applicable).
    SkipStage,
    /// Degrade to a faster path (e.g., quantized model).
    Degrade,
    /// Emit a default value.
    DefaultOnTimeout,
}

/// Queue capacity and watermark configuration.
///
/// `soft_*` define backpressure **watermarks**; `max_*` define **hard caps**.
/// Watermark state is derived from live occupancy snapshots and drives scheduling/backpressure.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueCaps {
    /// Maximum number of items permitted (hard cap).
    pub max_items: usize,
    /// Soft watermark for items; admission adapts between soft and hard.
    pub soft_items: usize,
    /// Optional byte cap and soft watermark for payload sizes.
    pub max_bytes: Option<usize>,
    /// Optional soft watermark in bytes.
    pub soft_bytes: Option<usize>,
}

impl QueueCaps {
    /// Construct new queue caps.
    pub const fn new(
        max_items: usize,
        soft_items: usize,
        max_bytes: Option<usize>,
        soft_bytes: Option<usize>,
    ) -> Self {
        Self {
            max_items,
            soft_items,
            max_bytes,
            soft_bytes,
        }
    }

    /// Borrow max_items.
    #[inline]
    pub const fn max_items(&self) -> &usize {
        &self.max_items
    }

    /// Borrow soft_items.
    #[inline]
    pub const fn soft_items(&self) -> &usize {
        &self.soft_items
    }

    /// Borrow max_bytes.
    #[inline]
    pub const fn max_bytes(&self) -> &Option<usize> {
        &self.max_bytes
    }

    /// Borrow soft_bytes.
    #[inline]
    pub const fn soft_bytes(&self) -> &Option<usize> {
        &self.soft_bytes
    }

    /// Return `true` if the given occupancy is below soft watermarks.
    pub fn below_soft(&self, items: usize, bytes: usize) -> bool {
        let items_ok = items < self.soft_items;
        let bytes_ok = match (self.max_bytes, self.soft_bytes) {
            (Some(_), Some(soft)) => bytes < soft,
            _ => true,
        };
        items_ok && bytes_ok
    }

    /// Return `true` if the occupancy is at or above hard cap.
    pub fn at_or_above_hard(&self, items: usize, bytes: usize) -> bool {
        if items >= self.max_items {
            return true;
        }
        if let Some(maxb) = self.max_bytes {
            if bytes >= maxb {
                return true;
            }
        }
        false
    }
}

/// Watermark state derived from queue occupancy and caps.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatermarkState {
    /// Well below soft watermarks.
    BelowSoft,
    /// Between soft and hard thresholds.
    BetweenSoftAndHard,
    /// At or above hard cap.
    AtOrAboveHard,
}

/// Admission behavior when the queue is not clearly below soft caps.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionPolicy {
    /// Deadline-aware and QoS-aware admission between soft and hard caps.
    DeadlineAndQoSAware,
    /// Simple drop-newest under pressure.
    DropNewest,
    /// Simple drop-oldest under pressure.
    DropOldest,
    /// Block producer until space is available (P2 only).
    Block,
}

/// Decision returned by an admission controller.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Admit the item.
    Admit,
    /// Reject the item per policy.
    Reject,
}

/// Per-edge policy bundle.
///
/// `caps` → soft/hard watermarks; `admission` → behavior between soft/hard;
/// `over_budget` → action when capacity/budget constraints are breached.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgePolicy {
    /// Capacity and watermarks.
    pub caps: QueueCaps,
    /// Admission behavior between soft and hard thresholds.
    pub admission: AdmissionPolicy,
    /// Action to take on over-budget scenarios at this edge.
    pub over_budget: OverBudgetAction,
}

impl EdgePolicy {
    /// Construct a new `EdgePolicy`.
    pub const fn new(
        caps: QueueCaps,
        admission: AdmissionPolicy,
        over_budget: OverBudgetAction,
    ) -> Self {
        Self {
            caps,
            admission,
            over_budget,
        }
    }

    /// Borrow caps.
    #[inline]
    pub const fn caps(&self) -> &QueueCaps {
        &self.caps
    }

    /// Borrow admission policy.
    #[inline]
    pub const fn admission(&self) -> &AdmissionPolicy {
        &self.admission
    }

    /// Borrow over-budget action.
    #[inline]
    pub const fn over_budget(&self) -> &OverBudgetAction {
        &self.over_budget
    }

    /// Compute a watermark state from occupancy stats.
    pub fn watermark(&self, items: usize, bytes: usize) -> WatermarkState {
        if self.caps.at_or_above_hard(items, bytes) {
            WatermarkState::AtOrAboveHard
        } else if self.caps.below_soft(items, bytes) {
            WatermarkState::BelowSoft
        } else {
            WatermarkState::BetweenSoftAndHard
        }
    }

    /// Apply admission logic based on header hints (deadline/qos) and occupancy.
    pub fn decide(
        &self,
        items: usize,
        bytes: usize,
        _deadline: Option<DeadlineNs>,
        _qos: QoSClass,
    ) -> AdmissionDecision {
        match self.watermark(items, bytes) {
            WatermarkState::BelowSoft => AdmissionDecision::Admit,
            WatermarkState::AtOrAboveHard => AdmissionDecision::Reject,
            WatermarkState::BetweenSoftAndHard => match self.admission {
                AdmissionPolicy::DeadlineAndQoSAware => {
                    // The full policy may weigh deadline & QoS; the core keeps it simple. TODO: CHECK!
                    AdmissionDecision::Admit
                }
                AdmissionPolicy::DropNewest => AdmissionDecision::Reject,
                AdmissionPolicy::DropOldest => AdmissionDecision::Admit, // queue impl will evict oldest TODO: CHECK!
                AdmissionPolicy::Block => AdmissionDecision::Reject, // core cannot block TODO: FIX!
            },
        }
    }
}

/// Policy bundle attached to a node.
///
/// Used by schedulers:
/// - `batching.fixed_n`/`max_delta_t` guide batch formation.
/// - `budget.tick_budget` (soft) and `budget.watchdog_ticks` (hard) guide time budgeting.
/// - `deadline.default_deadline_ns` allows EDF synthesis when inputs have no deadlines,
///   `deadline.slack_tolerance_ns` provides grace, and `require_absolute_deadline` enforces strictness.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NodePolicy {
    /// Batch formation policy.
    batching: BatchingPolicy,
    /// Budget policy for execution steps.
    budget: BudgetPolicy,
    /// Deadline policy for inputs/outputs.
    deadline: DeadlinePolicy,
}

impl NodePolicy {
    /// Construct a `NodePolicy` explicitly.
    pub const fn new(
        batching: BatchingPolicy,
        budget: BudgetPolicy,
        deadline: DeadlinePolicy,
    ) -> Self {
        Self {
            batching,
            budget,
            deadline,
        }
    }

    /// Borrow the batching policy.
    #[inline]
    pub const fn batching(&self) -> &BatchingPolicy {
        &self.batching
    }

    /// Borrow the budget policy.
    #[inline]
    pub const fn budget(&self) -> &BudgetPolicy {
        &self.budget
    }

    /// borrow the deadline policy.
    #[inline]
    pub const fn deadline(&self) -> &DeadlinePolicy {
        &self.deadline
    }
}
