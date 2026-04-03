# Policy Guide

Policies are the contract mechanism in Limen. They are defined at graph
construction time and enforced at runtime — not observed after the fact.
A message that violates a freshness policy should not reach a node. A node
that exceeds its budget should be preempted.

---

## EdgePolicy

Controls admission to a queue:

```rust
pub struct EdgePolicy {
    caps: QueueCaps,
    admission: AdmissionPolicy,
    over_budget: OverBudgetAction,
}
```

### QueueCaps

```rust
pub struct QueueCaps {
    max_items: usize,       // Hard item cap
    soft_items: usize,      // Soft watermark
    max_bytes: Option<usize>,   // Hard byte cap
    soft_bytes: Option<usize>,  // Soft byte watermark
}
```

### WatermarkState

Derived from occupancy vs. `QueueCaps`:

| State | Condition |
|---|---|
| `BelowSoft` | Items and bytes below soft watermarks |
| `BetweenSoftAndHard` | Above soft but below hard on at least one dimension |
| `AtOrAboveHard` | At or above hard cap on any dimension |

### AdmissionPolicy

```rust
pub enum AdmissionPolicy {
    DeadlineAndQoSAware,  // Full policy: uses QoS + deadline for admission
    DropNewest,           // Drop incoming message when above watermark
    DropOldest,           // Evict oldest message to make room
    Block,                // Block until space is available
}
```

### AdmissionDecision

The result of evaluating a push against the current occupancy:

```rust
pub enum AdmissionDecision {
    Admit,
    Reject,
    DropNewest,
    Evict(usize),               // Evict N oldest items
    EvictUntilBelowHard,        // Evict until below hard cap
    Block,
}
```

### Admission Flow

| Watermark State | DropNewest | DropOldest | Block |
|---|---|---|---|
| `BelowSoft` | Admit | Admit | Admit |
| `BetweenSoftAndHard` | Admit | Admit | Admit |
| `AtOrAboveHard` | DropNewest | Evict(1) | Block |

`DeadlineAndQoSAware` uses additional header metadata (deadline, QoS class)
to make finer-grained decisions.

### OverBudgetAction

What to do when a message exceeds the byte budget:

```rust
pub enum OverBudgetAction {
    Drop,             // Drop the message
    SkipStage,        // Skip processing, forward as-is
    Degrade,          // Use degraded processing path
    DefaultOnTimeout, // Use default output on timeout
}
```

---

## NodePolicy

Controls execution semantics:

```rust
pub struct NodePolicy {
    batching: BatchingPolicy,
    budget: BudgetPolicy,
    deadline: DeadlinePolicy,
}
```

### BatchingPolicy

```rust
pub struct BatchingPolicy {
    fixed_n: Option<usize>,         // Fixed items per batch
    max_delta_t: Option<Ticks>,     // Max time span between first and last item
    window_kind: WindowKind,
}
```

| Constructor | Behaviour |
|---|---|
| `BatchingPolicy::none()` | Batch size 1 (default) |
| `BatchingPolicy::fixed(n)` | Exactly N items per batch |
| `BatchingPolicy::delta_t(t)` | Items within delta-t of front |
| `BatchingPolicy::fixed_and_delta_t(n, t)` | Whichever limit is reached first |

### WindowKind

```rust
pub enum WindowKind {
    Disjoint,                       // Non-overlapping windows (default)
    Sliding(SlidingWindow),         // Overlapping; stride <= window size
}
```

### BudgetPolicy

```rust
pub struct BudgetPolicy {
    tick_budget: Option<Ticks>,     // Soft per-step budget
    watchdog_ticks: Option<Ticks>,  // Hard watchdog timeout
}
```

### DeadlinePolicy

```rust
pub struct DeadlinePolicy {
    require_absolute_deadline: bool,        // Require deadlines on inputs
    slack_tolerance_ns: Option<DeadlineNs>, // Acceptable overshoot
    default_deadline_ns: Option<DeadlineNs>,// Synthesise if message has none
}
```

---

## Robotics Primitives (Planned)

The following contract extensions are planned for robotics and control system
use cases (see [ADR-012](../ADRs/012_ROBOTICS_READINESS.md) and the
[Roadmap](../roadmap.md)):

| Primitive | Description | Status |
|---|---|---|
| **Freshness** | Message expiry (`max_age` / `valid_until`) | Planned (R2) |
| **Input liveness** | Minimum message rate on input edges | Planned (R5) |
| **Node liveness** | Minimum output rate guarantee | Planned (R6) |
| **Urgency** | Priority ordering under contention | Planned (R1) |
| **Criticality** | Safety-critical vs best-effort separation | Planned (R8) |
| **Mailbox** | Overwrite-on-full, `read_latest` semantics | Planned (R4) |

---

## Related

- [Edge Model](edge.md) — how `EdgePolicy` governs admission
- [Node Model](node.md) — how `NodePolicy` controls step behaviour
- [Runtime Model](runtime.md) — how policies feed scheduling decisions
- [Message and Payload](message_payload.md) — header fields used by admission
