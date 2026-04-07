# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Limen, please report it privately.

- **Email:** arlo@umbriel-systems.com
- **Subject:** `[SECURITY] Limen vulnerability report`

Please do **not** open a public GitHub issue for security vulnerabilities.

### What to include

To help triage the issue quickly, include:

- A clear description of the vulnerability
- Steps to reproduce (proof of concept if possible)
- Affected version(s) or commit(s)
- Any relevant logs, traces, or screenshots
- Potential impact and suggested mitigations (if known)

## Response Process

- Reports will be acknowledged within **72 hours**
- A fix or mitigation will be developed as quickly as possible
- You may be asked for additional details during investigation
- Once resolved, you may be credited (if desired)

## Disclosure Policy

Please allow time for the issue to be addressed before public disclosure.

Coordinated disclosure is appreciated.

## Project Status

Limen is currently in **pre-release (alpha)**. APIs and internal implementations may change, and not all components have undergone full security review.

In particular:
- Some planned features (e.g. lock-free concurrent structures) may introduce carefully-audited `unsafe` code behind feature flags
- These components will undergo additional validation (conformance testing, Miri, etc.) before being considered production-ready

## Scope

This policy applies to:
- All crates in the Limen workspace
- Generated code via `limen-codegen`
- Example and test graphs

Out of scope:
- Issues caused by incorrect usage of the API
- Hypothetical vulnerabilities without a concrete reproduction path
