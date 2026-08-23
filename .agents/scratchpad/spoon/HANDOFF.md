# Spoon implementation handoff

Last updated: 2026-08-23 (foundation remediation in flight)

## Mission

The active objective is to implement and verify every phase in
`IMPLEMENTATION-PLAN.md`. The plan is authoritative; the historical design
archive is conceptual reference material. The user authorized autonomous implementation,
subagents, commits, tests, and continuous progress. Preserve the user-owned
untracked reference/archive files and the intentional whitespace-only change in
`crates/spoon-engine/src/runtime.rs`.

## Current repository state

### Authoritative active checkpoint

The earlier phase summaries below are historical context and are being audited
under the new Implementation Reality Gate in `IMPLEMENTATION-PLAN.md`. Do not
infer current workspace health or product usability from an older “green” claim.

Active work is tracked in:

- `IMPLEMENTATION-PLAN.md` — Priority Foundation Completion Track and
  seven-rung Implementation Reality Gate;
- `PRIMITIVE-CAPABILITY-INVENTORY.md` — living `[x]/[~]/[ ]` inventory and
  material claim-correction log;
- `.agents/scratchpad/spoon/robust-host-capabilities-language/{context,plan,progress}.md`;
- global Codex skill `~/.codex/skills/implementation-reality-audit`.

Verified current baseline (2026-08-23; still a dirty worktree, not a release):

- versioned pure intrinsic expressions are serialized in `spoon-core`, executed
  in `spoon-exec`, and traversed by `spoon-graph`;
- bounded Unicode normalization and text transforms, JSON/path access, and a
  broader collection/map/conversion intrinsic slice execute; rich `pure_expr_v2`
  Engine tests compile/persist/reuse selected procedures without a Teacher;
- the scoped file bridge is public via `capability.invoke`: temporary-directory
  tests prove real reads/writes, persistent grants, next-call revocation,
  bounds, redaction, symlink-escape denial, and honest unsupported-family
  failure;
- `cargo fmt --all -- --check`, `cargo test --workspace --all-targets --quiet`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and
  `pnpm test`, `pnpm typecheck`, `pnpm build`, `pnpm depcheck` pass.
- a live Codex CLI provider smoke on a clean temporary database now teaches
  `double(7)=14` through the provider-safe JSON envelope and reuses it with
  Teacher disabled for held-out `double(11)=22`. This is a narrow live-provider
  proof, not broad provider parity or language competence.

Current in-flight lanes (preserve partial edits; rerun evidence before claiming):

- `spoon-engine/src/cycle.rs` and cycle tests: exact-version pure-procedure
  dependency composition for `pure_expr_v2` lessons is complete and needs only
  normal regression reruns when adjacent work lands;
- capability/server/SDK: public scoped filesystem effects exist, but real SDK
  invocation coverage and cognitive-cycle selection remain missing;
- `seeds/`: schema-valid declared curriculum designs for language, structured
  data, and programming now exist; they are explicitly not an executable seed
  forge and have no acquisition/Teacher-OFF evidence yet.
- numeric pure intrinsic expansion is now the active bounded standard-library
  lane; do not claim its operations until its tests and a fresh workspace gate
  land.

Known corrected claims:

- file read/write are now locally integrated through public server JSON-RPC,
  not merely helper logic; they are still not selected by learned procedures
  and are not production-deployment evidence;
- sandbox execution is a deterministic fixture and injected adapter boundary,
  not a real operating-system process/container sandbox;
- durable capability grants and direct Rust invocation APIs exist, but cognitive
  cycle and public transport integration were incomplete at audit time.

Do not commit the broad dirty worktree as one batch: it contains multiple prior
user/agent changes that cannot be safely attributed. Do not claim the workspace
green while concurrent edits are active.

Latest substantive commits:

- `bfbdd75` — expose the human-facing `pnpm spoon ...` workspace command.
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

## Historical verification checkpoint (before current remediation edits)

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
- **P5:** native network/file/observe/sandbox requests are typed and policy
  enforced at direct helper/injected-adapter boundaries; discovery/synthesis,
  local validation, revocable grants, direct Rust adapter-backed invocation,
  clean-instance bundle round trips, bounded
  curiosity scheduling, held-out regression-gated representation activation,
  and broad mutation authorization are present. Bundle
  format v2 is deterministic, content-addressed, dependency-closed,
  quarantine-first, locally revalidated, and rejects secrets, local authority,
  malformed reconstruction, and over-permissioned procedures atomically.
  This does not prove public app invocation, cognitive-cycle integration, or a
  real OS sandbox; those are active remediation items above.
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

### Phase 7 checkpoint (uncommitted working tree)

The current working tree contains the Phase 7 implementation slice. Preserve
these edits; they are intentionally not committed yet so the user can review
the complete batch:

- `packages/cli/src/config.ts`, `config.schema.json`, and `admin.ts` implement
  strict hierarchical config, source/shadow diagnostics, atomic layer writes,
  user-layer admin mutations, teacher enablement, permission modes, recall
  defaults, database pointer changes, and redacted admin receipts.
- `crates/spoon-core`, `spoon-episode`, `spoon-reason`, and `spoon-engine`
  carry durable session IDs/visibility/turn indices and global/session/none
  recall policy through public cycles.
- `crates/spoon-server` and `packages/sdk` expose session lifecycle and
  filtered episode APIs; `packages/cli` exposes `session`, `chat`, config
  diagnostics, per-call recall/permission flags, and deterministic natural
  admin requests.
- `packages/cli/src/benchmark.ts` and `benchmarks/benchmark.schema.json`
  validate and run developmental experiment phases through public
  `session`/`ask` subprocesses, including Teacher-ON acquisition and
  Teacher-OFF retention. The source of truth is `ekg-benchmark-suite/`, with
  the executable catalog in `benchmarks/catalog.json` and starter experiments
  in `benchmarks/fixtures/`; the old downloaded seed format is intentionally
  not supported.
- `runBenchmark` now accepts `benchmarks/catalog.json` as well as an individual
  fixture, resolves suite IDs to fixture files, runs each fixture through the
  public path, and writes an aggregate report with one telemetry run ID per
  fixture.
- `packages/cli/src/judge.ts` implements the post-run Judge protocol. It
  reuses the Claude/Codex CLI and OpenAI/Ollama/human structured-output
  adapters through protocol-specific system/prompt overrides, receives only
  immutable redacted benchmark evidence, and has no Spoon write path. Judge
  verdicts are required for a judged benchmark step to pass and persist their
  provider/model/request provenance in the report.
- `crates/spoon-capability` contains local `ask`, `workspace`, and
  `full-access` permission policies. Full access still enforces declared
  effects, bounds, contracts, provenance, quarantine/revalidation, and
  mandatory denials.

Validation checkpoint for this slice:

- `cargo test --workspace --all-targets --quiet`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `pnpm test`, `pnpm typecheck`, `pnpm build`, `pnpm depcheck`
- `git diff --check`

Benchmark validation after the catalog/fixture update:

- `pnpm typecheck`
- `pnpm test`, `pnpm build`, and `pnpm depcheck` after the Judge backend work
- all 13 catalog fixture files parse through `parseBenchmarkFixture`
- catalog references resolve to existing fixture files
- `git diff --check`

All passed after the latest edits. The remaining Phase 7 work is the broader
P7.6 adversarial matrix and richer permission-grant/interactive-confirmation
UX; do not claim those as complete without adding their tests.

### Numeric intrinsic slice (2026-08-23)

The current uncommitted work also adds a bounded version-1 numeric standard
library slice. `spoon-core::IntrinsicOp` and the `pure_expr_v2` Teacher schema /
compiler now expose finite-safe absolute value, sign, min/max, clamp,
floor/ceil/round/truncate, checked integer power, finite float power, and
strict integer quotient/remainder. Integer power uses checked exponentiation by
squaring; negative exponents return the typed `NegativeExponent` error, integer
overflow returns `ArithmeticOverflow`, and non-finite numeric inputs/results or
inverted clamp bounds return typed `InvalidNumber` errors. The evaluator and
Teacher-authored engine paths have focused tests; no logarithm, root,
trigonometric, random, decimal, or rational claims are implied.

Focused validation after this slice:

- `cargo test -p spoon-core --lib`: 3 passed.
- `cargo test -p spoon-exec --lib`: 46 passed.
- `cargo test -p spoon-engine --test cycle`: 42 passed.
- `cargo clippy -p spoon-exec -p spoon-engine --all-targets -- -D warnings`:
  passed.

1. Run the strict-audit items above against current code/tests.
2. Implement only missing invariants with focused adversarial tests, preserving
   the active telemetry/capability commits.
3. Rerun the complete workspace/package gate and update this file plus the
   final audit report.
4. Only after the goal is genuinely complete, call `update_goal(complete)`.
