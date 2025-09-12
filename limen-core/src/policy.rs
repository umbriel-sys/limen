//! Policies for batching, budgets, deadlines, admission, and capacities.

use crate::types::{DeadlineNs, QoSClass, Ticks};

/// Batch formation policy: fixed-N and/or Δt micro-batching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchingPolicy {
    /// Fixed number of items per batch (>= 1). If `None`, do not use fixed-N.
    pub fixed_n: Option<usize>,
    /// Maximum micro-batching window in ticks (Δt). If `None`, no Δt cap.
    pub max_delta_t: Option<Ticks>,
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
}

/// Budget policy for node execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetPolicy {
    /// Per-step tick budget; exceeding invokes over-budget action.
    pub tick_budget: Option<Ticks>,
}

/// Deadline policy for messages processed by a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadlinePolicy {
    /// Whether to require absolute deadlines on inputs (P2).
    pub require_absolute_deadline: bool,
    /// Optional slack tolerance (ns) before defaulting/degrading.
    pub slack_tolerance_ns: Option<DeadlineNs>,
}

/// Action to take when budgets or deadlines are breached.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Admit the item.
    Admit,
    /// Reject the item per policy.
    Reject,
}

/// Per-edge policy bundle.
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
