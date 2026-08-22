# Reason Hardening Progress

- [x] Inspected current reason/core/store APIs and Phase 1 requirements.
- [x] Documented hardening acceptance criteria and dependency map.
- [x] RED tests written and observed failing.
- [x] Interpretation boundary hardened.
- [x] Context selection and persistence hardened.
- [x] Focused tests and strict linting pass.

## Setup

Automatic mode is active. No project `CODEASSIST.md` or matching development
guide was present. This delegated slice will not create a commit.

## TDD Cycles

- RED: malicious tolerance, oversized candidate/config, lossless persistence,
  relevance ranking, and inactive filtering tests failed on missing behavior.
- GREEN: tolerance is capped at `1e-6`, interpretations at 64 candidates,
  context collections at 1024 items, text at 65,536 characters, graph traversal
  at 16 hops, and nested values at depth 64.
- GREEN: `AssembledContext` now stores all Phase 1 categories with serde defaults
  for Phase 0 compatibility.
- GREEN: entity-indexed history outranks unrelated recency; relevant procedure
  metadata is selected deterministically; inactive graph material is excluded.

## Validation

- `cargo test -p ekg-core -p ekg-reason`: 18 passed, 0 failed.
- `cargo clippy -p ekg-core -p ekg-reason --all-targets --all-features -- -D warnings`: passed.
- `cargo check --workspace`: passed.
- `cargo test --workspace`: passed, including engine cycle and server RPC suites.
- `git diff --check`: passed.
