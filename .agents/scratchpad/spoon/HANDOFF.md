# Spoon implementation handoff

Last updated: 2026-08-22 (complete Spoon namespace migration)

## Mission

The active objective is to implement and verify every phase in
`IMPLEMENTATION-PLAN.md`. The plan is authoritative; the historical design
archive is conceptual reference material. The user authorized autonomous implementation,
subagents, commits, tests, and continuous progress. Preserve the user-owned
untracked reference/archive files and the intentional whitespace-only change in
`crates/spoon-engine/src/runtime.rs`.

## Current repository state

Latest substantive commits:

- `2a3d25a` — rename Rust crates, TypeScript packages, binaries, environment
  variables, SQLite identifiers, docs, tests, and tracked scratchpad paths to
  Spoon; ignore local archives and scratchpads.

- `24a1765` — derive managed-skill challenger replays from trusted source
  traces and an exact newer procedure revision.
- `0ad1d21` — require additional successor behavior evidence and persist
  execution-shape subsumption records for retirement.
- `76500ef` — gate representation/search-policy activation on durable held-out
  regression evidence and expose the evaluation through RPC/SDK.
- `a303603` — record compression known gaps, add clean-instance capability
  round-trip evidence, and expand falsification fixtures.
- `41fc76d` — align inspector examples with the public Spoon rename.

- `d2431a1` — bind managed-skill replay evidence to source episodes and require
  promoted, live-verified retirement successors.
- `8969a01` — reject held-out training and teacher-grounding leakage in Section
  38 telemetry.
- `ab5a52f` — expose the v2 capability bundle wire format as typed SDK models.
- `4083f2a` — secure bundle v2 provenance, reconstruction metadata, secret and
  local-authority rejection, and immutable redacted failure receipts.
- `d1efc1d` — durable Section 38 falsification runs/measurements, all twelve
  metric slots, explicit insufficient-evidence states, anti-gaming checks, RPC,
  SDK, inspector cards, and fixture harness.
- `73f6f95`, `46f193c`, `c006452`, `bf13080`, `501a0e8`, `0ab9b48`, `6b330ce`,
  `19d4fde`, `a38c555`, `d4ac9db`, `0195178`, `0cb123d`, `dbe8f32` — prior
  Phase 3–6 implementation slices.

## Verification checkpoint

Green after the latest integration:

- `cargo test --workspace --all-targets --quiet`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo fmt --all -- --check`
- `pnpm test`
- `pnpm build`
- `pnpm typecheck`
- `pnpm depcheck`
- focused telemetry, capability, consolidation, SDK, inspector, and RPC tests
- `git diff --check`

The rename migration now uses Spoon names throughout the source tree. User-owned
reference/archive files and agent scratchpads are ignored and are not part of
the implementation commits.

## Phase evidence

- **P0–P2:** foundational data/graph/execution/episodes/evaluation, teacher
  cycle, credit assignment, replay, adaptation, reconciliation, contradiction
  refinement, durable trust and recovery are implemented and covered by the
  workspace and adversarial suites.
- **P3:** bounded recall, typed activation spread, locally fitted ranking,
  held-out ranking evidence, representation artifacts that affect retrieval,
  and grounded self-supervision/replay are implemented. The ranker is a small
  local artifact, not an external embedding service.
- **P4:** repetition/single-success/failure-critic discovery now emits
  executable evidence artifacts; broad adaptation runs a durable full
  regression suite; compression is non-destructive, preserves failures, and
  records explicit known gaps; the strict managed-skill challenger path derives
  replay evidence from trusted source traces and an exact newer revision;
  retirement requires a promoted successor with live verification, additional
  behavior evidence, and persisted execution-shape coverage.
- **P5:** native network/file/observe/sandbox primitives are typed and policy
  enforced; discovery/synthesis, local validation, revocable grants, actual
  adapter-backed invocation, clean-instance bundle round trips, bounded
  curiosity scheduling, held-out regression-gated representation activation,
  and broad mutation authorization are present. Bundle
  format v2 is deterministic, content-addressed, dependency-closed,
  quarantine-first, locally revalidated, and rejects secrets, local authority,
  malformed reconstruction, and over-permissioned procedures atomically.
- **P6.1:** inspector exposes bounded graph/relationship/procedure/episode/
  contradiction/dependency/replay views and a redacted human-readable episode
  narrative; CLI `ask --explain` has parity for teacher, prediction,
  observation, validation, learning, cost, and capability status.
- **P6.2:** Section 38 has a durable falsification API and dashboard projection.
  All twelve slots are always present and explicitly marked measured or
  insufficient; held-out, teacher-off, exact-repeat, failure, abstention, and
  clarification handling is persisted and tested.

## Residual research-grade work

The explicit exit criteria in `IMPLEMENTATION-PLAN.md` now have direct
implementation and regression evidence. The following are deliberately
follow-on research tasks rather than unverified claims of completion:

1. generalized neutral-IR sandbox execution remains bounded to approved
   native mappings rather than foreign code;
2. Section 38 fixtures cover representative held-out domain transfer, teacher
   ablation, attribution faults, calibration, and longitudinal compounding
   strongly enough to make a thesis-level call rather than merely proving the
   storage/API path.

## Next recovery steps

1. Run the strict-audit items above against current code/tests.
2. Implement only missing invariants with focused adversarial tests, preserving
   the active telemetry/capability commits.
3. Rerun the complete workspace/package gate and update this file plus the
   final audit report.
4. Only after the goal is genuinely complete, call `update_goal(complete)`.
