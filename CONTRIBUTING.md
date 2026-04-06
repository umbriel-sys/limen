# Contributing to Limen

Thank you for your interest in contributing to Limen. This document covers how
to contribute and the terms under which contributions are accepted.

---

## Getting Started

Limen is currently in pre-release development. The repository will be made
public alongside the v0.1.0 release.

If you are interested in early access, collaboration, or have a use case you
would like to discuss, please open an issue or reach out directly.

### Development Setup

See the [Development Guide](docs/dev_guide.md) for build instructions, testing,
code style, and project structure.

---

## Submitting Changes

1. Fork the repository and create a feature branch.
2. Make your changes following the code style described in the
   [Development Guide](docs/dev_guide.md).
3. Run the local CI script before submitting:

   ```bash
   ./dev_utils/run-local-ci.sh
   ```

   This runs the full CI pipeline locally: fmt, build, test, and clippy across
   all feature flag combinations (`no-default-features`, `alloc`, `std`,
   `spsc_raw`). See `./dev_utils/run-local-ci.sh --help` for options.

4. Open a pull request with a clear description of the change.

---

## Contributor Licence Agreement

We use a lightweight Contributor Licence Agreement (CLA) to ensure the project
can remain sustainable and flexible, including future open-source and commercial
use.

**By submitting a pull request, you indicate your agreement to the
[CLA](CLA.md).**

Please read it before contributing. It is short and written in plain language.
