# EKG implementation handoff

Last updated: 2026-08-22 (Phase 1 clean; commit pending)

This is the durable restart point for completing every phase in
`IMPLEMENTATION-PLAN.md`. Update it whenever ownership, verification state, or
the next executable step changes.

## Mission and authority

- User authorized autonomous implementation of all phases, use of subagents,
  best-judgment decisions, tests, commits, and continuous execution.
- Do not stop for routine clarification. Do not push. Preserve unrelated user
  work. Use `apply_patch` for edits.
- Active goal: implement and verify Phase 0 through Phase 6 completely.

## Completed and committed

- Phase 0 baseline: `e0e9b5b feat: establish EKG seed foundation`
- Phase 0 completion: `0e48cfa feat: complete Phase 0 seed system`
- Phase 0 independent audit: clean. Root gate included 87 Rust tests and 11
  TypeScript tests, strict workspace clippy/build/typecheck/depcheck/format.

## Current phase: Phase 1 — Teacher

Tracking detail:

- `.agents/scratchpad/ekg/phase-1-teacher/context.md`
- `.agents/scratchpad/ekg/phase-1-teacher/plan.md`
- `.agents/scratchpad/ekg/phase-1-teacher/progress.md`
- `.agents/scratchpad/ekg/phase-1-teacher/logs/`

Implemented locally, not yet committed:

- New `packages/teacher`: provider-neutral interface, validation pipeline,
  reliability, Claude/OpenAI/Ollama/human adapters; initial 9 tests green.
- New `crates/ekg-reason`: weighted interpretation and bounded context;
  initial 11 tests green.
- `ekg-engine` resumable `begin_cycle`/`resume_cycle` state machine with
  Recall → Run → Ask → Abstain, single terminal episode, generic concept-name
  matching, scalar binding, teacher provenance, procedure learning, and
  provisional answer handling.
- `crates/ekg-engine/tests/cycle.rs`: 23/23 green. Strict focused engine
  clippy is green.
- Core `Episode.teacher_interaction` field added with serde default.

Latest focused command known green:

```text
cargo fmt --all
cargo clippy -p ekg-engine --all-targets -- -D warnings
cargo test -p ekg-engine --test cycle
```

Workspace tests were attempted during concurrent server edits and stopped on
one transient boxed-outcome conversion error. The transport agent has already
fixed that line; rerun the full workspace gate after agents finish.

## In-flight ownership

- All implementation/hardening agents completed and handed ownership back.
- `/root/phase1_test_audit` completed four audit/fix passes and declared Phase
  1 clean.
- No agent may commit. Root integrates, audits, and creates the Phase 1 commit.

## Current worktree notes

- `.agents/scratchpad/ekg/phase-0-completion/progress.md` has a harmless
  post-commit documentation update; include it in the next documentation or
  Phase 1 commit.
- Expected modified/new areas: root Cargo manifests/lock, core episode model,
  engine cycle, server cycle RPC, new reason crate, new teacher package, SDK,
  CLI, pnpm lock, scratchpads.
- All changes are Phase 1 related; no known unrelated user edits were found.

## Phase 1 remaining checklist

1. Commit the clean Phase 1 work and record the hash.
2. Phase 1 final gate is green:
   - `cargo fmt --all -- --check`
   - `cargo test --workspace`
   - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
   - `cargo build --workspace --all-targets --all-features`
   - root pnpm format/test/typecheck/build/depcheck scripts as defined in
     `package.json`.
3. The real Rust-server ↔ TypeScript-SDK fake-teacher kitchen flow is green:
   first task requires one teacher call; learned paraphrase requires zero.
4. Update Phase 1 progress, commit with a conventional `feat:` message, record
   the hash here, then begin Phase 2.

## Known design decisions

- Rust owns the resumable cognitive state machine; TypeScript only invokes an
  async `Teacher` and resumes with raw output.
- Raw teacher status must be exactly `unverified`; Rust independently validates
  shape, references, provenance situation/source, and deterministic execution.
- Answer-only teacher output remains `Provisional`, with prediction populated
  and no observed result/evaluation.
- Learned procedures are deterministically executed before graph insertion.
- Teacher-authored procedure semantics remain `Provisional`; successful
  execution proves executability, not truth. Claimed answers must match the
  deterministic result before provisional integration.
- Pending continuations are process-local and once-only in Phase 1.
- Engine cycle execution must never call Phase 0's episode-writing public
  execution path; every terminal cycle writes exactly one episode.
- Ambiguity is preserved. Equal top weights are not guessed.
- Matching is domain-generic; never special-case `DOUBLE`.

## After Phase 1

Create per-phase scratchpads matching the Phase 1 structure and keep this file
as the top-level index. Next planned crates/features from the implementation
plan:

- Phase 2: `ekg-credit` + `ekg-adapt`, contract/statistical/replay attribution,
  narrow correction, reconciliation, contradictions.
- Phase 3: intuition, learned retrieval/context, contract-guided composition,
  self-supervision and teacher weaning.
- Phase 4: compression, invariants, promotion gates, forgetting, regression.
- Phase 5: curiosity queues, experiment planning, budgeted background learning.
- Phase 6: inspector plus all twelve measurable/anti-gamed metrics.

For every phase: RED acceptance tests → focused implementation → full gate →
independent audit → fixes → conventional commit → update this handoff.
