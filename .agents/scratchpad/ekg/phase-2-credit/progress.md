# Phase 2 progress

- [x] Objective, boundaries, invariants, and RED acceptance matrix drafted
- [x] Phase 1 committed at `d4fbe1e` and dependency surface frozen
- [x] Core attribution data model
- [x] P2.1 contract violation detection
- [x] P2.2 statistical attribution
- [x] P2.3 bounded counterfactual replay
- [x] Exit-audit credit hardening: canonical immutable Hard/Consensus oracle,
  exact baseline reproduction, engine-only decisive promotion, repeated-call
  rejection, suggestion-only replay candidates, conservative late-feedback
  joins, content-addressed retry identity, and raw scale-sensitive cost
- [x] Core adaptation data model
- [x] P2.4 evidence-gated narrow adaptation
- [x] P2.5 knowledge reconciliation
- [x] P2.6 contradiction refinement/holding
- [x] Hardened evidence authority, immutable persistence, atomic reconciliation,
  conservative alternatives, and transactional contradiction refinement
- [x] Trusted engine adaptation plan/apply/get orchestration
- [x] Server, SDK, and CLI adaptation/contradiction surfaces
- [x] Flat-pancake injected-fault kitchen test (metric 7 top-1/rank/MRR,
  metric 8 cost ratio, immutable feedback/history, narrow correction,
  simulated/noncausal controls, and contradiction refinement)
- [x] Full Rust/TypeScript gate and strict static checks
- [x] Adversarial remediation re-audit by root against the original findings
- [ ] Independent subagent audit (unavailable: all delegated agents exhausted
  their usage); retain this limitation in the handoff rather than implying one
  occurred
- [ ] Phase 2 commit

## Fresh-audit remediation in progress

- [x] Failed local attempts are immutable episodes before teacher escalation.
- [x] Pending teacher continuations survive reopen and are claimed by one
  engine instance.
- [x] Active cycles and broad maintenance exclude one another transactionally
  across Engine instances using the shared SQLite runtime registry.
- [x] Broad maintenance leases bind database identity, owner, epoch, expiry,
  and the exact apply-request digest; staged recovery retains the lease until
  its durable receipt exists.
- [x] Read-only Engine graph/episode facades and durable trust receipts.
- [x] Semantically bound observed facts and automatic contradiction handling,
  including held-uncertainty propagation, scoped refinement selection, and
  abstention outside demonstrated scopes.
- [x] Honest metric-7 held-out corpus and metric-8 indexed scaling below 0.5.

## 2026-08-22 complete gate checkpoint

- Full Rust workspace tests, strict Clippy, rustfmt, and all-target build pass.
- Full TypeScript Prettier, tests (teacher 19, SDK 13, CLI 15), typecheck,
  build, and depcheck pass.
- `git diff --check` passes.
- Root's adversarial re-audit is recorded below. A separate subagent audit could
  not be obtained because all delegated agents exhausted their usage.

## 2026-08-22 remediation re-audit

- The old audit findings were rechecked against current code. Live running
  cycles are fail-closed across engine opens; staged broad work reacquires only
  expired leases and refuses another live owner; receipt retries are owner
  scoped; episode/teacher, authenticated-feedback, and fact-receipt sagas are
  restartable.
- Raw strong success rows are excluded from regression authorization, and
  external authenticated observations receive local episode and fact receipts;
  raw rows cannot create contradiction authority.
- Metric 7 now includes a held-out deterministic body/operator fault and emits
  a separate failure-family report. Metric 8 uses scalar aggregate state and
  reports feedback/index maintenance work separately.
- Current complete gate: `cargo test --workspace --all-targets`, strict
  workspace Clippy, `pnpm test`, `pnpm typecheck`, `pnpm build`, and
  `pnpm depcheck` all pass. `git diff --check` remains required before commit.

## Credit hardening verification

- `cargo test -p ekg-credit --test credit`: 10 passed.
- `cargo test -p ekg-engine --test credit_cycle`: 21 passed.
- Scoped rustfmt checks pass for credit source/tests and engine credit source/test.
- Strict scoped Clippy passes with `--no-deps`; a normal strict run also passed
  after concurrent `ekg-adapt` lint reconciliation.
- Durable immutable analysis persistence is now implemented in Engine-owned
  SQLite tables. Explicit idempotency keys survive reopen, exact retries return
  the stored canonical analysis, same-key/different-request retries conflict,
  and failed computation reserves no rows. Automatic keys bind request plus
  canonical evidence state, so newly appended evidence creates a fresh
  analysis while unchanged retries reuse storage. The engine credit suite now
  has 21 passing tests.
