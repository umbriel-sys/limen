# Funding

Limen is an independent open-source project. This document describes what
funding supports, the licensing commitments for funded work, and how the
project plans to sustain itself long-term.

---

## What Funding Supports

Limen is working toward a production-ready v1.0 release. Funding
accelerates the path from the current alpha to a stable, battle-tested
framework that downstream projects can depend on.

The v1.0 roadmap includes:

- **Contract stabilisation** — freezing the public trait surface (`Node`,
  `Edge`, `GraphApi`, `LimenRuntime`, `MemoryManager`) with semver
  guarantees.
- **Robotics primitives** — freshness, liveness, criticality, urgency, and
  mailbox semantics for real-time control system graphs.
- **Production runtimes** — `NoAllocRuntime` with full policy enforcement
  and `ThreadedRuntime` with EDF and throughput scheduling.
- **Inference backends** — TFLite Micro (Cortex-M4) and Tract
  (desktop/server) behind the `ComputeBackend` trait.
- **Platform implementations** — Raspberry Pi and Cortex-M4 reference
  platforms with clock, GPIO, and I/O support.
- **Conformance and testing** — comprehensive test suite covering tensor
  payloads, N-to-M graphs, mailbox semantics, staleness detection, liveness
  violations, and telemetry events.
- **Documentation and release hygiene** — full API documentation, feature
  flag audit, CI gates, changelog, and cross-platform reference examples.

See the [Roadmap](docs/roadmap.md) for the full phased plan.

---

## Licensing Commitment for Funded Work

All deliverables produced under accepted public grants — including code,
tests, documentation, and example artifacts delivered under funded
milestones — will be released publicly under the Apache License, Version 2.0.

These deliverables will remain available under Apache-2.0. The Apache
License is irrevocable: once a version is released under Apache-2.0, that
version stays under Apache-2.0 permanently. This is a property of the
licence itself, not a policy that can be changed.

The goal is to deliver a production-ready v1.0 under Apache-2.0 as a
public good.

---

## Future Versions and Sustainability

Future major versions of Limen may be distributed under different or
additional licence terms (for example, dual-licensing for commercial
sustainability). This is a standard approach used by many open-source
projects and does not affect the Apache-2.0 availability of earlier
releases.

In concrete terms: if v1.0 is released under Apache-2.0 with grant
funding, v1.0 remains Apache-2.0 forever. Any future versions developed
independently may ship under different terms — but v1.0 and its source
code remain freely available under the original licence.

The long-term aspiration is to keep Limen free and open-source. If the
project achieves significant adoption, moving it to a non-profit
foundation for community stewardship is a possibility the maintainer is
open to.

---

## Contributor Licence Agreement

Contributions to Limen are accepted under a lightweight
[Contributor Licence Agreement](CLA.md) (CLA). The CLA grants the project
maintainer the right to sublicence and relicence contributions. This
flexibility supports long-term sustainability while keeping the open-source
core accessible.

The CLA does not affect the licensing commitments for funded work described
in this document.

---

## How to Support Limen

If you are interested in funding, sponsoring, or collaborating on Limen,
please open an issue or contact the maintainer directly.

- **Public grants and innovation funds** — Limen is actively seeking
  support from open-source and public innovation programmes.
- **Corporate sponsorship** — partnerships with companies in robotics,
  embedded systems, and edge AI.
- **Community support** — GitHub Sponsors (coming soon).

---

Copyright © 2025–present Arlo Louis Byrne (idky137)
