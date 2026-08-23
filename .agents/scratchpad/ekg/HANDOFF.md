# Spoon implementation handoff

Last updated: 2026-08-22 (full workspace gate + Section 38 boundary audit)

## Mission

The active objective is to implement and verify every phase in
`IMPLEMENTATION-PLAN.md`. The plan is authoritative; `WHAT-IS-EKG-v3.md` is
conceptual reference material. The user authorized autonomous implementation,
subagents, commits, tests, and continuous progress. Preserve the user-owned
untracked reference/archive files and the intentional whitespace-only change in
`crates/ekg-engine/src/runtime.rs`.

## Current repository state

Latest substantive commits:

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

The only intentional tracked worktree change is the pre-existing formatting
delta in `crates/ekg-engine/src/runtime.rs`. Do not stage it unless the user
explicitly asks for that unrelated change. Do not stage `WHAT-IS-EKG-v3.md`,
`ekg-benchmark-suite.zip`, or `.agents/scratchpad/ekg/*`.

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
  regression suite; compression is non-destructive and preserves failures;
  managed-skill replay evidence is source-bound; retirement requires a promoted
  successor with live verification.
- **P5:** native network/file/observe/sandbox primitives are typed and policy
  enforced; discovery/synthesis, local validation, revocable grants, actual
  adapter-backed invocation, bounded curiosity scheduling, and broad mutation
  authorization are present. Bundle format v2 is deterministic,
  content-addressed, dependency-closed, quarantine-first, locally revalidated,
  and rejects secrets, local authority, malformed reconstruction, and
  over-permissioned procedures atomically.
- **P6.1:** inspector exposes bounded graph/relationship/procedure/episode/
  contradiction/dependency/replay views and a redacted human-readable episode
  narrative; CLI `ask --explain` has parity for teacher, prediction,
  observation, validation, learning, cost, and capability status.
- **P6.2:** Section 38 has a durable falsification API and dashboard projection.
  All twelve slots are always present and explicitly marked measured or
  insufficient; held-out, teacher-off, exact-repeat, failure, abstention, and
  clarification handling is persisted and tested.

## Remaining strict-audit work

Do not mark the active goal complete until direct evidence proves every plan
exit criterion. In particular, audit whether:

1. skill promotion is fully engine-derived rather than accepting any caller-
   supplied challenger result (procedure replacement has an engine-owned
   replay path; managed skills still need a final authority review);
2. compression/forgetting records an explicit known-gap when information is
   ever intentionally omitted, and retirement evidence demonstrates behavioral
   subsumption rather than only a promoted successor;
3. capability discovery is exercised end-to-end from an authorized interface
   observation through typed synthesis, sandbox tests, local revalidation,
   grant, invocation, and failure receipts;
4. structural search-policy changes have the same slow/offline regression gate
   as broad procedure/concept changes;
5. Section 38 fixtures cover representative held-out domain transfer,
   teacher ablation, attribution faults, calibration, and longitudinal
   compounding strongly enough to make a thesis-level call rather than merely
   proving the storage/API path.

## Next recovery steps

1. Run the strict-audit items above against current code/tests.
2. Implement only missing invariants with focused adversarial tests, preserving
   the active telemetry/capability commits.
3. Rerun the complete workspace/package gate and update this file plus the
   final audit report.
4. Only after the goal is genuinely complete, call `update_goal(complete)`.
