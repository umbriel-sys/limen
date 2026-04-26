# Limen Roadmap

## Vision

Limen aims to be the standard framework for portable, contract-enforcing
computation graphs in embedded AI systems. A single pipeline definition should
run — unchanged — from a bare-metal Cortex-M4 to a multi-threaded Linux
server, with inference, scheduling, and safety contracts handled by the
framework.

---

## Roadmap to v0.1.0

v0.1.0 is the first release where the public API is stable enough for
downstream crates to build against without repeated breaking changes.

### Phase Overview

| Phase | Focus | Key Issues | Status |
|---|---|---|---|
| **0** | Guardrails | Feature hygiene, CI matrix, `#[non_exhaustive]` | Done |
| **1** | Currency + Arity | Tensor-first payloads, N-to-M ports, time types | In progress |
| **2** | Trait Surface | Contract stabilisation, metadata completeness | Planned |
| **3** | Robotics Primitives | Freshness, liveness, criticality, deadline ordering, mailbox | Planned |
| **4** | Runtime Semantics | Lifecycle contract, error propagation | Planned |
| **5** | Platform Finalisation | `PlatformBackend` stabilisation | Planned |
| **6** | Quality + Release | Tests overhaul, docs audit, MSRV, CI gates | Planned |

---

### Phase 0: Guardrails (Done)

Established the foundation for stable API evolution:

- Feature flag hygiene (`no_std` default enforced in CI).
- `#[non_exhaustive]` on all public enums and structs.
- Constructors and builders instead of public fields.
- CI matrix: `no_std`, `alloc`, `std` builds all pass.

### Phase 1: Currency + Arity (In Progress)

Finalise what flows through the graph and how ports are addressed:

- **C1 — Tensor-first currency.** Tensor and batch payloads as the primary
  currency for ML workloads. **Complete.**
- **C1a — Zero-copy memory model.** Token-based payload memory management with
  three manager implementations. **Complete.**
- **C2 — N-to-M node arity.** Port model with indexed inputs/outputs,
  per-port readiness rules, and fan-in/fan-out support.
- **R0a — MessageHeader baseline.** Lock `Timestamp` and `Duration` newtypes,
  add `max_age` (producer freshness cap) and `ClockDomainId` fields, remove
  `QoSClass` (scheduling is deadline-driven).

### Phase 2: Trait Surface Stabilisation

Freeze the public trait signatures based on Phase 1 outcomes:

- **C3 — Contract stabilisation.** Finalise `Node`, `Edge`, `MemoryManager`,
  `GraphApi`, and `LimenRuntime` signatures. Sync-first core; async adapters
  above. Lock generated graph construction boundaries, including where node and
  edge policies are supplied.
- **C4 — Metadata completeness.** Port definitions, batch constraints, shape
  metadata, quantisation metadata, and memory-manager capacity contracts. All
  alloc-free by default.

### Phase 3: Robotics Primitives

Add first-class contracts for robotics and control system graphs:

- **R1** — Deadline-driven ordering. `deadline_ns` in `MessageHeader` is the
  sole per-message urgency signal. Unified priority key:
  `(criticality, readiness, deadline, sequence, index)`.
- **R2** — Two-sided freshness. `max_age` on both the message header
  (producer-asserted) and `InputPortPolicy` (consumer-asserted);
  effective age = min of both.
- **R3** — Clock domain identification. `ClockDomainId` in `MessageHeader`
  ensures correct freshness evaluation across split-clock graphs (e.g. MCU +
  Linux). In scope for v0.1.0.
- **R4** — Mailbox and latch semantics. `EdgePolicy::RetentionMode`:
  `Consume` (latest-wins mailbox) and `Sticky` (latched value). Ergonomic
  constructors: `EdgePolicy::mailbox_latest()` / `latch_latest()`.
- **R5** — Per-input liveness policy. `InputPortPolicy.liveness` detects
  stuck sensors; runtime tracks last-push time per edge.
- **R6** — Node liveness policy. `NodePolicy.node_liveness` detects stalled
  nodes; runtime tracks last-step time per node.
- **R7** — `in_peek_deadline` on `StepContext` for node-author inspection of
  per-port head deadlines. (`Edge::peek_header` already exists.)
- **R8** — Criticality classes. `CriticalityClass { SafetyCritical, Operational,
  BestEffort }` on `NodePolicy`. Codegen enforces criticality-flow rules
  (lower-criticality producer cannot feed a `Required` input on a
  higher-criticality consumer).
- **R9** — Standard telemetry events. `TelemetryEvent::Policy(PolicyEvent)`
  routed through the existing `Telemetry` trait. Covers stale messages,
  liveness violations, deadline misses, overruns, failsafe signals, and
  structural drops.
- **R10** — Stale-skip convenience. `ctx.in_read_latest_fresh::<PORT, P>()`
  (compile-time restricted to mailbox/latch ports) and
  `ctx.in_pop_front_fresh::<PORT, P>()` (any FIFO port). In scope for v0.1.0.
- **R11** — Execution overrun measurement. Wired to existing
  `BudgetPolicy.watchdog_ticks`; emits `PolicyEvent::ExecutionOverrun`. In
  scope for v0.1.0.

### Phase 4: Runtime Semantics

Define the execution lifecycle and error propagation contracts:

- **RS1 — Runtime lifecycle.** Five phases: `init` → `start` → `running` →
  `stop` → `reset`/`shutdown`. Soft reset (queue flush, liveness tables
  zeroed) vs hard reset. Runtime tables (`edge_last_push`, `node_last_step`,
  etc.) initialised at `start`.
- **G1 — Error propagation.** `ErrorAction` enum on `NodePolicy`:
  `HaltGraph` (whole-graph stop), `Quiesce` (node stops; downstream reacts
  via readiness), `EmitFailsafeSignal { quiesce }` (writes to declared
  failsafe output port), `Continue` (drop and emit). Defaults derived from
  `CriticalityClass`.

### Phase 5: Platform Finalisation

Stabilise the platform abstraction:

- **P1 — PlatformBackend.** Service provider: `now() -> Timestamp`,
  `local_domain_id() -> ClockDomainId`, `init()`/`shutdown()` lifecycle hooks.
  Not a policy engine; no event-emission hook (telemetry uses the existing
  `Telemetry` trait). Reference implementation for Linux. Cortex-M4 stub.

### Phase 6: Quality + Release

Final gates before tagging v0.1.0:

- **Q1 — Tests overhaul.** Tensor-first, N-to-M graphs, mailbox Consume/Sticky
  modes, two-sided freshness, liveness, criticality ordering, activation policy
  (Periodic coalescing), trigger-based inputs, deadline miss telemetry,
  budget hardcap/softcap behaviour, heap/alloc graph variants, test-runtime
  parity with production dequeue policy, concurrent scheduler invariants, and
  codegen validation errors. Deterministic time injection via mock
  `PlatformBackend`.
- **Q2 — Docs audit.** All public API documented. Feature flag requirements
  noted on every item. `QoSClass` references removed. API freeze rules
  published.
- **Q3 — Release hygiene.** MSRV pinned (≥ 1.65). CI gates: fmt, clippy,
  test, doc. Feature matrix. Changelog. Public codegen entrypoint and builder
  naming settled; unhandled fallible calls cleaned up; experimental queues fixed
  or gated before inclusion in the stable feature matrix. Examples compile under
  all features.

---

## v0.1.0 Definition of Done

All of the following must be satisfied:

1. All leaf issues closed or explicitly deferred with documented rationale.
2. Feature matrix builds clean in CI: `no_std`, `alloc`, `std`.
3. Comprehensive test coverage for tensor payloads, N-to-M graphs,
   mailbox/latch semantics, two-sided freshness, liveness violations,
   criticality ordering, and telemetry events (full S0 §7 test matrix).
4. Public API documented and frozen.
5. At least one platform crate satisfies the final `PlatformBackend` contract.

---

## Beyond v0.1.0

### v0.2.0 — Inference Backends, Platforms, and First Product Slice

- **TFLite Micro backend** — `no_std`, static tensor arena, Cortex-M4 target.
- **Tract backend** — pure Rust, `std`, desktop and server targets.
- **Raspberry Pi platform** — full `PlatformBackend` implementation with
  GPIO, SPI, and I2C support.
- **Cortex-M4 platform** — SysTick/DWT clock, UART telemetry drain.
- **Source and sink node library** — Replay, file, synthetic, IMU, UART, GPIO,
  stdout/log/file sinks.
- **Cross-platform IMU activity recognition demo** — Same graph running across
  desktop/server and embedded targets.

### v1.0 — Production Readiness

- Production `NoAllocRuntime` with full policy enforcement.
- `ThreadedRuntime` with EDF and throughput scheduling.
- Production polish for the IMU reference example and deployment guides.
- External conformance test suite documentation for third-party
  implementations.
- Stability guarantees: semver compliance, deprecation policy.

### Post v1.0 (Stretch Goals)

- **Zero-lock concurrent graphs ([ADR-013](ADRs/013_ZERO_LOCK_ZERO_COPY_CONCURRENT_GRAPHS.md))** —
  lock-free edge and memory manager enabling a single graph type across all
  execution models (no_std single-threaded, std multi-threaded, and no_std
  multi-core).
- Async node adapter for I/O-bound workloads.
- Device memory transfer orchestration (Host ↔ GPU/NPU).
- Dynamic graph reconfiguration (add/remove nodes at runtime).
- Visual graph editor / debugger.
- WASM target support.
