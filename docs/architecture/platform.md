# Platform Model

The platform layer abstracts hardware-specific capabilities behind portable
traits. A strict design boundary ensures nodes remain portable: **only
`PlatformClock` enters `StepContext`**. Timers, CPU affinity, and I/O
configuration are runtime concerns, not node concerns.

---

## Design Boundary

| Enters StepContext | Does NOT enter StepContext |
|---|---|
| `PlatformClock` | `Timers` |
| | `Affinity` |
| | Transport / I/O configuration |

This preserves:
- **Portability** — nodes compile on any target with any clock
- **`no_std` viability** — no std-dependent types in the node interface
- **Determinism** — tests inject a `NoopClock` for reproducible behaviour
- **Separation** — scheduling policy stays in the runtime

---

## PlatformClock

```rust
pub trait PlatformClock {
    fn now_ticks(&self) -> Ticks;
    fn ticks_to_nanos(&self, ticks: Ticks) -> u64;
    fn nanos_to_ticks(&self, ns: u64) -> Ticks;
}
```

The epoch is implementation-defined. Monotonicity is required. Conversions
allow runtimes to expose a fast tick domain while providing nanosecond
correlation for telemetry and deadline enforcement.

### Implementations

| Implementation | Target | Notes |
|---|---|---|
| `NoopClock` | Testing | Always returns `Ticks(0)`, identity conversion |
| `NoStdLinuxMonotonicClock` | Linux / Raspberry Pi | `CLOCK_MONOTONIC` via `libc` |
| *(planned)* | Cortex-M4 | SysTick / DWT hardware timer |

`()` also implements `PlatformClock` (equivalent to `NoopClock`).

---

## Span Helper

Timing span for measuring elapsed time:

```rust
let span = Span::start(&clock);
// ... work ...
let elapsed_ns: u64 = span.end_ns();
```

---

## Timers (P1/P2)

```rust
pub trait Timers {
    fn sleep_until(&self, ticks: Ticks);
    fn sleep_for(&self, ticks: Ticks);
}
```

Not passed to nodes. If a node requires timer access, the host should inject
a handle at construction time out-of-band.

---

## Affinity (P2)

```rust
pub trait Affinity {
    fn pin_to_core(&self, core_id: u32);
    fn set_numa_node(&self, node_id: u32);
}
```

Core/NUMA placement is a runtime concern. The P2 concurrent runtime uses
`Affinity` to pin worker threads to specific cores for deterministic latency.

---

## Planned Platforms

| Platform | Clock | I2C | UART | Status |
|---|---|---|---|---|
| Linux / Raspberry Pi | `CLOCK_MONOTONIC` | Yes | Yes | Implemented (clock) |
| Cortex-M4 | SysTick / DWT | Yes | Yes | Planned |
| Cortex-M0 | — | Compile-only | Compile-only | Planned |

---

## Related

- [Runtime Model](runtime.md) — the runtime that owns platform resources
- [Node Model](node.md) — how nodes access the clock via `StepContext`
- [Telemetry](telemetry.md) — nanosecond correlation via clock conversions
