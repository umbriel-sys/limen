# ADR-012: Robotics Readiness Primitives

**Status:** Proposed
**Date:** 2026-01-15

## Context

Robotics and control system graphs have requirements that general-purpose
dataflow frameworks do not address:

- **Multi-rate execution.** A 1 kHz control loop and a 10 Hz perception
  pipeline share the same graph.
- **Deterministic fault detection.** If a sensor stops producing data, the
  system must detect this within bounded time — not after an unbounded queue
  drain.
- **Priority under contention.** Safety-critical messages must take precedence
  over best-effort telemetry.
- **Freshness guarantees.** Stale sensor data must not reach an actuation node.

These are not logging concerns — they are runtime-enforced contracts.

## Decision

Add first-class primitives to `limen-core` for robotics readiness. All
primitives are pure metadata, policy extensions, or edge API additions.
No scheduler logic enters `limen-core`.

### Planned Primitives

| ID | Primitive | Location | Description |
|---|---|---|---|
| R1 | Urgency | `MessageHeader` | Priority field for deterministic ordering |
| R2 | Freshness | `MessageHeader` + `NodePolicy` | `max_age` / `valid_until` expiry |
| R4 | Mailbox | `Edge` API | `peek_latest` / `read_latest` with overwrite-on-full |
| R5 | Input liveness | `NodePolicy` | Minimum message rate on input edges |
| R6 | Node liveness | `NodePolicy` | Minimum output rate guarantee |
| R7 | Edge peek | `Edge` API | `peek_header` / `peek_urgency` for schedulers |
| R8 | Criticality | `NodePolicy` + `MessageHeader` | Safety-critical vs best-effort classes |
| R9 | Standard events | `TelemetryEvent` | Stale message, liveness violation, overrun |

### Optional (may defer to v0.2.0)

| ID | Primitive | Rationale for deferral |
|---|---|---|
| R3 | Clock domains | Useful for multi-clock systems; not required for single-clock MCU |
| R10 | Stale-skip | Convenience API; can be built from R2 + R4 |
| R11 | Execution measurement | Requires runtime instrumentation; R9 events suffice initially |

### Ordering Semantics

Under contention, messages are ordered by:
1. **Criticality** — safety-critical before best-effort.
2. **Deadline** — earliest deadline first.
3. **Sequence** — FIFO within equal priority.

This ordering is enforced by schedulers and priority queues, not by
`limen-core` directly. Core provides the metadata; runtimes enforce the
ordering.

## Rationale

- **Contracts, not logging.** A stale message should not reach a node. An
  input liveness violation should trigger a detectable event within bounded
  time. These are runtime-enforced guarantees.
- **Core enables; core does not embed.** The primitives (header fields, policy
  extensions, edge APIs) are data and trait definitions. Enforcement logic
  lives in runtimes and schedulers.
- **`no_std` compatible.** All primitives compile without heap. Bitfields,
  enums, and policy structs only.
- **Incremental adoption.** Each primitive is independently useful. A system
  can use freshness without urgency, or mailbox without criticality.

## Alternatives Considered

| Alternative | Pros | Cons | Why Rejected |
|---|---|---|---|
| External supervisor process | No core changes | Adds latency, requires IPC, not `no_std` | Cannot enforce at scheduling boundary |
| ROS2-style QoS profiles | Familiar to robotics developers | Too coarse; doesn't support fine-grained per-edge policies | Limen's policy model is more expressive |
| Post-hoc telemetry analysis | No runtime overhead | Detection is after the fact; cannot prevent stale data from reaching nodes | Does not enforce contracts |

## Consequences

### Positive
- Safety-critical control loops can express their requirements declaratively.
- Multi-rate graphs support mailbox/latest semantics natively.
- Liveness violations are detectable from standard telemetry events alone.
- All primitives are `no_std` compatible.

### Trade-offs
- Adds fields to `MessageHeader` (increases per-message overhead by a few
  bytes).
- Policy surface grows — more configuration options for users to understand.
- R3/R10/R11 scope decision is still open.

## Dependencies

- **R0a** — `MessageHeader` baseline and time type semantics (must land first).
- **C2** — N-to-M port model (ports must be stable before per-input liveness).
- **C3** — Trait surface stabilisation (traits must be frozen before adding
  robotics extensions).

## Related ADRs

- [ADR-008](008_NODE_CONTRACT.md) — node policy extensions
- [ADR-007](007_EDGE_CONTRACT.md) — edge API extensions (mailbox, peek)
- [ADR-006](006_MEMORY_MODEL_MANAGER_CONTRACT.md) — header metadata for ordering
