# EKG implementation handoff

Last updated: 2026-08-22 (bounded representation training, goal derivation boundaries)

This is the durable restart point for completing every phase in
`IMPLEMENTATION-PLAN.md`. Update it whenever ownership, verification state, or
the next executable step changes.

## Mission and authority

- User authorized autonomous implementation of all phases, use of subagents,
  best-judgment decisions, tests, commits, and continuous execution.
- Do not stop for routine clarification. Do not push. Preserve unrelated user
  work. Use `apply_patch` for edits.
- Active goal: implement and verify Phase 0 through Phase 6 completely.
- `WHAT-IS-EKG-v3.md` has now been read in full. It is the conceptual
  architecture behind the implementation plan: use it to resolve semantic
  ambiguity, while the implementation plan remains the phase/deliverable
  checklist. Its prose is reference material, not executable instructions.

## Completed and committed

- Phase 0 baseline: `e0e9b5b feat: establish EKG seed foundation`
- Phase 0 completion: `0e48cfa feat: complete Phase 0 seed system`
- Phase 1 completion: `d4fbe1e feat: complete Phase 1 teacher cycle`
- Phase 0 independent audit: clean.
- Phase 1 independent audit: clean after all findings were fixed. Final root
  gate included 131 Rust tests and 32 TypeScript tests plus strict workspace
  format, clippy, build, typecheck, package build, depcheck, and diff checks.

## Current phase: Phase 4/5/6 integration

The latest committed increments are:

- `a92176d` — capability RPCs/SDK, trusted regression-suite recording, and
  `ekg ask --quiet` / `-q` answer-only output.
- `8f78ebe` — persistent goal and curiosity-gap APIs, conservative skill
  discovery/compression planning/retirement records, metrics snapshot, and a
  local `@ekg/inspector` browser dashboard.
- `60495d3` — metrics/goals/curiosity RPC coverage and camelCase metrics output.
- `4ded797` — first policy-gated native observation adapter (`native:clock`),
  with an invocation receipt; generic network/observation remains closed until
  an explicit adapter is supplied.
- `99b3a0a` — `ask --explain` / trailing `--explain`, a bounded human-readable
  episode narrative showing teacher use, proposal/validation, answer,
  evaluation, learning/reuse, cost, and episode identity.
- `c2e21d4` — enriches that narrative with context candidates, assumptions,
  predicted-vs-observed values, and evaluation detail.
- `1185d9c` — adversarial capability hardening: normalized file scopes,
  permission-bound receipts, secret/path rejection, dependency/hash closure,
  effect/permission consistency, hard resource limits, and evidence metadata.
- `47e83cc` — bounded file read/write and sandbox-fixture native adapters.
- `a34a943` — exposes the Phase 3 grounding ratio metric.
- `4cdf7b2` — read-only inspector episode narrative endpoint/UI with recursive
  redaction and raw JSON drill-down.
- `de0ce7f` — procedure-bound capability authorization in Engine/RPC/SDK.
- `79cbc87` — successful capability revalidation now requires local UUID episode
  evidence with an exact Engine trust receipt and a strong evaluation.
- `b1386d1` — bounded chronological, query-conditioned held-out ranking
  evaluation with persisted search-win evidence and metrics.
- `f48a0fd` — durable skill candidate/shadow/promoted/retired lifecycle with
  receipt-gated registration, replay gate, authenticated live-win promotion,
  and reconstructible retirement.
- `ff867ba`, `48139f4` — consolidation lifecycle RPC/SDK surfaces, including
  registration, shadow evaluation, live promotion, listing, and retirement.
- `9e09d4c` — bounded, versioned representation-training artifacts with
  held-out coverage, explicit activation, intuition/engine/RPC/SDK facades, and
  public SDK exports.
- `318c7ce`, `240523e`, `2a99bdf` — goal-bound learning provenance, storage-level
  standing-goal immutability, derived instrumental-goal provenance, and RPC/SDK
  APIs for both derived-goal paths.

The first usable chat command is now:

```text
EKG_DB=/tmp/ekg-playground.sqlite pnpm exec tsx packages/cli/src/main.ts ask --quiet "what is double 7?"
```

For the bounded human-readable episode summary (teacher use, proposal,
validation, learning/reuse, evaluation, and cost), use `ask --explain`.

The dashboard can be opened with:

```text
cargo build -p ekg-server
EKG_DB=/tmp/ekg-playground.sqlite pnpm --filter @ekg/inspector dev
```

Then visit `http://127.0.0.1:4317`. It is local and read-only; stop it with
Ctrl-C. The full JSON CLI output remains available by omitting `--quiet`.

Capability acquisition now has typed discovery, canonical deterministic
bundles, quarantine imports, local validation, revocable grants, primitive
policy checks, a safe clock observation adapter, RPC and SDK methods. Goals and
curiosity gaps are durable, bounded, and standing goals are immutable. Phase 4
skill lifecycle records are durable and promotion-gated; compression and
single-success/failure discovery remain conservative planning/report paths.

Capability limitations are explicit: network transport remains adapter-injected,
sandbox execution is currently a bounded fixture boundary rather than arbitrary
neutral-IR execution, and the bundle still needs a full dependency-DAG
reconstruction runner. Engine-side local validation is now trust-receipt-bound;
the low-level capability store remains usable for isolated bundle tests.

Representation training is intentionally a bounded term-weight artifact and
does not yet replace the deterministic lexical/hash retrieval index with a
semantic embedding model. Skill lifecycle records are durable and gated, but
skills are not yet executable/ranked procedures. These are open implementation
gaps, not claims of completion.

The inspector narrative is available at `GET /api/episodes/:id` and from the
episode detail view. It explains teacher/provider/model/proposal/validation,
learning or reuse, prediction/observation/evaluation/cost, escalation,
abstention, and capability authority while redacting sensitive fields.

## Pause checkpoint (2026-08-22)

- Phase 2 is complete and committed at `0e8e999` (`feat: complete Phase 2
  credit and adaptation`).
- The full gate passed immediately before the pause: workspace Rust tests and
  strict Clippy, TypeScript tests, typecheck, build, and depcheck.
- Phase 2 audit status is documented in `phase2-final-audit.md`; its explicit
  limitation is that the independent subagent audit was unavailable after the
  delegated agents exhausted their usage. Root remediation and adversarial
  coverage are recorded with that limitation rather than overstated.
- Phase 3 is now in progress. `ekg-intuition` has bounded inverted-term recall,
  persisted outcome-aware ranking, trusted recall, and a chronological,
  query-conditioned held-out ranking evaluator with durable search-win metrics.
- Phase 3 progress and remaining work are recorded in
  `.agents/scratchpad/ekg/phase-3-intuition/progress.md`. Typed activation
  spread, ranked local interpretation, trusted recall, stale-index cleanup,
  and larger-corpus evidence are now implemented; the phase still needs a real
  semantic embedding/trainer lifecycle and causal cross-query/rung evidence.

- Phase 4 now has a pure replay promotion gate: correctness regressions reject
  shadow eligibility, and a measurable compression/search/coverage/transfer
  win is required before shadowing. Existing replacement replay delegates to
  this gate without changing trust or admin boundaries.
- Phase 5 capability foundation is implemented in `ekg-capability`: typed
  native primitive policy, explicit discovery/synthesis, canonical content-
  addressed bundles, quarantine import, local sandbox revalidation, and
  separate revocable local grants. Engine and RPC/SDK facades are available;
  network transport and a full neutral-IR reconstruction runner remain next.

## Current restart point (authoritative)

- Phase 2 is committed and its remediation gate is green. The earlier agent
  entries below are historical; independent audit capacity was unavailable,
  so that limitation remains recorded rather than overstated.
- The plan now contains an explicit cost-routing policy: Luna for coordinator,
  routine edits, fixtures, and focused checks; Terra for normal substantive
  implementation/integration; Sol only for named adversarial/security/
  concurrency architecture and final high-risk audits.
- Current root fixes, all focused-green:
  - exact trusted regression filtering now has an Engine-level forged-success
    test; raw cross-engine successes cannot authorize a narrow adaptation;
  - maintenance recovery never treats another live Engine's lease as its own,
    and idempotent receipt retries never clear another owner's lease;
  - episode, trusted fact, pending-teacher, and authenticated-feedback
    persistence all use durable recoverable sagas;
  - `ObservedFact` now has stable fact IDs, source episode, verifier, evidence
    tier, scope digest, and independently verifiable fact receipts;
  - authenticated external observations are admin-gated local operations and
    conflicting verified observations enter the contradiction store.
- Focused verification most recently green: `durability` (3), `trust_ledger`
  (5), `adaptation` (16), `cycle` (33), plus strict Clippy for `ekg-engine`
  and `ekg-core`.
- Next concrete work: run the complete Rust/TypeScript gate and perform a
  requirement-by-requirement audit of every plan exit criterion. Treat the
  semantic embedding/retrieval lifecycle, executable skill integration, full
  capability reconstruction runner, and all-twelve-metric instrumentation as
  explicit remaining gaps unless the code and tests provide direct evidence.
  Exclude `WHAT-IS-EKG-v3.md` and `ekg-benchmark-suite.zip` from staging.

## Current live checkpoint (2026-08-22, adversarial remediation in flight)

- Phase 2's first complete workspace gate was green, but the independent audit
  in `phase2-final-audit.md` correctly rejected exit. Phase 2 is not complete or
  committable yet.
- Root removed the unsafe startup deletion of every `running` cycle and added a
  two-runtime regression test. Durable stale-cycle recovery still needs an
  owner/heartbeat-aware design; fail-closed is the current behavior.
- Raw Hard/Consensus auxiliary evidence is now filtered through exact Engine
  trust receipts for online-regression authorization and reconciliation
  alternative support. Adversarial Engine-level tests are still due.
- Contradiction mutation methods are now explicit admin operations; offline
  maintenance authority requires admin plus an existing broad plan; simulated
  replay issuance requires admin and simulated evidence is non-actionable.
- SDK contradiction mutations now carry the configured admin token.
- Cycle context now consumes transitive claim uncertainty. Dependency edges
  require two recorded/canonical claim identifiers. Automatic contradiction
  refinement now splits on exactly one demonstrated differing scope feature and
  deliberately holds ambiguous multi-feature cases.
- `/root/phase2_persistence` owns expired-lease recovery and crash-safe episode /
  trust / contradiction / pending-teacher sagas. `/root/metric8_scaling` owns
  honest metric-8 materialization/cost accounting and a genuinely varied
  metric-7 corpus. The shared workspace may be temporarily uncompilable while
  those edits are in flight.
- After both workers finish: resolve overlaps, add forged auxiliary-evidence and
  authenticated external-fact tests, run focused gates, repeat the full gate,
  and obtain a new independent adversarial audit.
- The Engine exposes read-only `GraphView`/`EpisodeView`; all raw writes require
  explicit Engine admin authority backed by a durable database-bound secret.
  Raw caller-inserted Hard/Consensus rows do not gain authority.
- `engine_trust_receipts` binds the exact immutable episode/feedback digest.
  Deterministic Engine evaluation and authenticated verifier feedback are the
  only strong receipt issuers. Adaptation and contradiction gates verify exact
  receipts rather than trusting caller enums.
- Failed local attempts persist before escalation; teacher continuations are
  durable and once-only; active cycles and broad maintenance mutually exclude
  across Engine instances through SQLite leases bound to exact requests.
- Episodes now carry canonical predicate-bound `ObservedFact` values and
  indexed scope. Conflicting trusted facts create held contradictions
  automatically. Held uncertainty enters cycle context and makes local output
  provisional. Persisted refinements select the procedure supported by the
  matching demonstrated scope; unseen scopes safely abstain. Recall is exact
  on both situation and environment.
- Metric 7 uses a nested held-out injected-fault corpus with correlated decoys,
  multiple fault locations, non-replayable cases, and honest abstention.
  Metric 8 now uses transactional materialized evidence/co-occurrence indexes;
  cost is history-size invariant and reaches 0.490 at the required five-step
  scale without changing the denominator.
- First complete gate: every Rust workspace/all-target test passed, strict
  workspace Clippy and rustfmt passed, all-target build passed; TypeScript
  Prettier, teacher 19 tests, SDK 13 tests, CLI 15 tests, typecheck, build, and
  depcheck passed; `git diff --check` passed.
- The authoritative audit is `.agents/scratchpad/ekg/phase2-final-audit.md`.
- Exact next action after a clean re-audit: update Phase 2 progress, repeat the
  complete gate, stage all intended Phase 2/architecture/scratchpad files while
  excluding `WHAT-IS-EKG-v3.md` and `ekg-benchmark-suite.zip`, inspect staged
  diff, and create the conventional Phase 2 commit.

## Newly accepted architectural scope (queued without interrupting Phase 2)

- The user explicitly requested first-class capability acquisition and sharing.
- `WHAT-IS-EKG-v3.md` section 32 now specifies a minimal policy-enforced native
  substrate (network, scoped files, observation, sandboxed execution), typed
  effect/permission contracts, discovery, synthesis, sandbox testing, and local
  promotion.
- `IMPLEMENTATION-PLAN.md` now adds `ekg-capability`, P5.3 capability
  acquisition, P5.4 reconstructible bundles, and moves structural
  self-modification to P5.5. Bundles transfer structure, tests, schemas,
  dependencies, and provenance—but never trust, secrets, grants, or ambient
  environment assumptions. Imports are atomic, quarantined, Provisional, and
  locally revalidated.
- Continue the current Phase 2 gate work first. Treat these Phase 5 additions as
  required for the full-plan completion goal, not as permission to weaken the
  Phase 2 trust boundary or to add domain-specific integrations.

## Latest root progress after Terra resume

- Teacher bootstrap is exit-clean at the focused level. Codex 0.149 strict
  schema compatibility was fixed (typed `const`, `anyOf`), bounded malformed
  lesson retry is enabled, lexical inflection matching is general rather than
  DOUBLE-specific, and the real default-Codex smoke proved teacher DOUBLE(7)=14
  followed by local DOUBLE(9)=18 with the same procedure and no second teacher.
- A real Terra teacher remained conservative but did not author a valid RPN
  lesson after the bounded retry; it returned only a provisional answer and
  installed nothing. This is provider quality, not a trust-boundary failure.
- Durable cycle/runtime remediation is implemented and focused-green: failed
  local attempts persist before escalation, pending continuations recover after
  reopen, stale engine instances cannot consume a claimed continuation, and
  SQLite-wide active-cycle/maintenance exclusion works across two Engine
  instances. Offline leases bind the exact request digest and are released only
  after the durable adaptation receipt.
- Focused status: cycle 33 tests, adaptation 15, server RPC 20, teacher 19,
  CLI 15, and SDK 13 passing; strict scoped Clippy and relevant TS typechecks
  pass. Full workspace gates have not yet been rerun.
- Next: implement read-only Engine facades plus durable trust receipts, then
  semantically bound observed facts/automatic contradiction consumption, then
  metric 7/8.

Tracking detail:

- `.agents/scratchpad/ekg/phase-2-credit/context.md`
- `.agents/scratchpad/ekg/phase-2-credit/plan.md`
- `.agents/scratchpad/ekg/phase-2-credit/progress.md`

Implemented locally, not yet committed:

- Root workspace scaffold adds `ekg-credit` and `ekg-adapt` members and their
  dependency entries. `Cargo.lock` is updated.
- Both new crates have placeholder manifests and source files; their real
  implementations are currently agent-owned as listed below.
- Phase 2 acceptance matrix is recorded in the phase scratchpad.
- `ekg-credit` P2.1-P2.3 domain layer is implemented with 9 focused tests:
  one-pass contract attribution, correlation-aware statistical suspicion, and
  bounded single-change counterfactual replay. Root corrected metric-8 total
  cost semantics and added explicit deterministic/simulated replay provenance;
  focused tests are green.
- Append-only `EpisodeFeedback` and idempotent persistence are implemented;
  episode and graph focused suites are green (22 + 31 tests). Graph revisions
  now have expected-version CAS and immutable procedure/concept/relationship
  history.
- Root added `feedback.record` through Rust RPC and TypeScript SDK with exact
  camelCase fields and idempotent coverage. Procedure execution failures now
  return structured `episodeId` and `cause` fields. Focused server/SDK tests,
  clippy, formatting, and SDK typecheck are green.
- `ekg-adapt` P2.4-P2.6 is implemented with 25 focused tests: evidence-gated
  narrow correction, executable scope conditions, CAS-pinned application,
  dependency reconciliation without deletion, persistent contradictions,
  demonstrated scope refinement, and inherited uncertainty. Root reran focused
  format/test/strict-clippy/diff gates successfully.
- Root hardened `ekg-credit` after independent audit: decisive replay now
  requires a failed evaluated source plus non-empty source/mutation identity and
  mode-matching verifier provenance; unverified success is inconclusive.
  Statistics deduplicate episode IDs, skip legitimate untraced history, and
  tier-weight Hard/Consensus/Deferred evidence. Nine focused tests and strict
  clippy are green.
- Engine credit integration now includes immutable SQLite-backed completed
  analyses with automatic and explicit idempotency. Analyses survive reopen;
  exact retries return canonical stored output, key/payload conflicts fail,
  incomplete work reserves no key, and evidence changes produce a fresh
  automatic identity. It also joins late feedback immutably, enforces
  version/lifecycle/pure-mutation boundaries, rejects no-op/noncausal patches,
  and preserves replay provenance and raw cost.
- Root exposed `credit.analyze` through Rust RPC and the TypeScript SDK. Request
  and response fields are recursively camelCase, snake_case transport params
  are rejected, and focused server/SDK format/test/typecheck gates are green.
- Persistence audit fixes are implemented: explicit draft→final episode flow,
  immutable completed episodes, feedback rejected for drafts, committed-row
  graph snapshots, created-at drift rejection, hardened legacy updates,
  deletion-aware current-version lookup, lifecycle-filtered dependencies, and
  versioned relationship dependencies. Episode/graph suites have 50 tests;
  cross-domain run had 87 green tests. Atomic mixed-entity reconciliation is
  the remaining graph persistence task and is now in flight.

Integration design received so far:

- Failure diagnosis is automatic/read-only. Mutation is explicit and only from
  a persisted, immutable, evidence-gated adaptation plan.
- Engine surface should center on `record_feedback`, `analyze_failure`,
  `plan_adaptation`, and idempotent `apply_adaptation`; server/SDK/CLI mirror
  those operations with camelCase wire fields.
- Add append-only episode feedback, persisted credit analyses, immutable plans,
  atomic/idempotent receipts, and append-only contradiction events. Never edit
  an episode to attach later real-world feedback.
- Safe replay loads exact historical procedure snapshots, validates the full
  trace registry, applies exactly one typed ephemeral mutation, and records
  source/mutation hashes. Mutant traces must not masquerade as persisted
  procedure versions.
- Graph application needs expected-version CAS, monotonic versions, immutable
  before/after audit snapshots, and reconciliation in one transaction. Concept
  and relationship mutations also need reconstructible history.
- Metric 8 reports raw contract/statistics/replay costs and replay fraction;
  do not collapse steps, tokens, wall time, and money into one fake unit.
- Engine mode gates broad mutations: narrow online-safe changes may apply while
  concept revision and structural contradiction splits are offline-only.

Phase 2 prerequisite fixes discovered by read-only design review:

- Add append-only late/external feedback instead of rewriting episodes. (done)
- Enforce lifecycle eligibility in every execution lookup, not only cycle
  recall; Invalid/Retired/UnderReview procedures must not execute.
- Return structured episode IDs for execution failures at the RPC boundary.
  (done)
- Persist Phase 2 audits/idempotency and make graph mutation/reconciliation
  atomic within SQLite. Graph and episode stores currently use separate
  connections, so do not claim cross-store atomicity without addressing it.

## Independent domain/persistence audit and re-audit

The first read-only Phase 2 audit found material blockers even though focused
tests passed. Do not commit Phase 2 until all are re-audited:

- Forged attribution/evidence/action types could directly authorize mutation;
  concept revision did not enforce mutability or a trusted offline capability.
  This is fixed with opaque authority minted from canonical stored evidence;
  broad changes remain blocked pending trusted engine capabilities.
- Replay certainty trusted caller enums/default provenance; root fixed this in
  `ekg-credit` and added downgrade coverage.
- Statistics overcounted duplicate IDs, aborted on untraced history, and treated
  weak evidence like Hard evidence; root fixed these and added tier weights.
- Feedback is joined through a selected immutable view without rewriting the
  episode; duplicates and conflicts cannot inflate evidence.
- Episode update still rewrote final history; graph snapshots could capture
  caller-altered immutable fields; unsafe legacy updates bypassed explicit CAS;
  current-version reads survived deletion. `/root/phase2_persistence` fixed
  these in episode/graph.
- Reconciliation was multi-transaction/non-resumable and alternative support
  was too optimistic; contradiction writes lacked idempotency/CAS/evidence
  verification. These are fixed with an atomic idempotent lifecycle batch,
  conservative relationship-aware support, and transactional contradiction
  identity/refinement with stored-evidence validation.

The fresh cross-audit rejected Phase 2 exit again despite the green root gate.
Open findings are authoritative work, not optional hardening:

- Real cycle failures can disappear into an in-memory teacher continuation or
  be folded into an overall successful episode that credit refuses to analyze.
  Persist immutable failed attempts and durable pending cycles; wire credit to
  failed attempts.
- `Engine::graph()` / `episodes()` expose writable stores, allowing in-process
  callers to bypass every evidence/adaptation gate and forge Hard evidence.
  Replace them with read-only facades and engine-owned trusted evidence/admin
  capabilities.
- Offline authority is instance-local, not process-wide. Add a durable database
  maintenance lease/epoch and durable active-cycle registry so multiple Engine
  instances cannot overlap broad mutation with reasoning.
- Contradiction evidence matches only a Boolean/value, not the semantic
  predicate; recording/refinement is caller-driven and reasoning never consumes
  held uncertainty. Bind verified observed facts to canonical predicates,
  detect/refine through Engine, and propagate uncertainty into cycles.
- Metric 7 needs a seeded held-out injected-fault corpus with decoys,
  correlations, interactions, nonreplayable faults, and honest abstention—not
  one pancake case. Metric 8 must include digest/cache/history work and prove
  scaling below a non-dominating (<0.5) cost ratio.
- Simulated replay needs an engine-minted, content-bound simulator receipt and
  must remain non-decisive model evidence.
- Persistence cross-audit found false alternative support from endpoint
  co-mention, missed calls in contract checks, destructive procedure deletion,
  evaluated-draft visibility, non-durable no-op reconciliation keys, untyped
  dependency direction, and lifecycle-blind graph traversal.
- SDK relationship reconciliation typing is fixed locally. Admin CAS revision
  surfaces and stable structured adaptation/contradiction subcodes remain due.

## In-flight ownership

- The pre-audit root gate was green (246 Rust tests; 45 TypeScript tests;
  strict Clippy/Rustfmt/Prettier/typecheck/build/depcheck/diff), but the fresh
  audits found the blockers above. Phase 2 remains explicitly not exit-clean.
- `/root/phase2_persistence` is remediating metric 7/8 accounting/corpus and
  trusted simulated replay receipts.
- `/root/teacher_hardening` is remediating independently found episode/graph/
  reconciliation gaps, then admin CAS/structured wire errors.
- `/root/phase2_persistence` completed persistence/reconciliation remediation:
  feedback semantic conflicts, evidence-backed alternative support,
  relationship-aware atomic CAS change sets, exact recovery receipts, and
  immutable history. Focused episode (23), graph (31), and adapt (35) suites
  plus strict formatting and Clippy are green.
- Engine-owned durable credit analyses and adaptation plan/get/apply receipts,
  opaque offline authority, restart recovery, durable assumption corrections,
  and canonical contradiction ownership are implemented and root-verified.
- Raw graph RPC mutation is disabled without admin/bootstrap authorization;
  external feedback receives server-assigned Deferred trust; offline apply
  consumes an internal capability; contradiction mutation/refinement and
  durable credit reads have exact RPC/SDK/CLI surfaces with structured errors.
- Root added a real Codex CLI teacher (`EKG_TEACHER=codex`): current Codex
  CLI, ephemeral isolated read-only exec, JSON-schema output, provenance, and
  a verified real smoke returning 14 provisionally without an API key. The
  stale global npm Codex copy was removed. The native updater confirms the
  installed `/Users/kealjones/.local/bin/codex` 0.149.0 is current. CLI
  auto-loads gitignored `.env`.
- A real user smoke exposed a remaining teacher-bootstrap usability gap:
  answer-only output is safe for live observations such as current time, but an
  empty graph does not give Codex enough structured authoring information to
  reliably propose reusable DOUBLE knowledge. `/root/phase1_cycle_design` is
  implementing a safe structured bootstrap path and real Codex acceptance
  proof without weakening Rust validation.
- Root owns manifests, later cross-crate integration, full gate, audit, commit.
- No agent may commit. Root integrates, audits, and creates the Phase 2 commit.

## Current worktree notes

- Expected Phase 2 changes: root Cargo manifests/lock, `crates/ekg-credit`,
  `crates/ekg-adapt`, later engine/server/SDK/CLI integration, and scratchpads.
- Phase 1 and Phase 2 progress documentation has post-commit bookkeeping that
  should be included in the Phase 2 commit.
- `WHAT-IS-EKG-v3.md` is an untracked pre-existing/user-side document. Preserve
  it and do not include it in implementation commits. It is now a consulted
  design reference, not an implementation artifact.

## Exact resume procedure

1. User reported only 2% Sol usage remaining. All three active subagents were
   deliberately interrupted after their latest checkpoint. The shared worktree
   currently passes `cargo check --workspace`; no commit was made. Switch this
   task to GPT-5.6 Terra, then resume the existing agents with follow-up tasks
   rather than restarting or discarding their edits.
2. Resume `/root/phase1_cycle_design`: finish the SHA-256 deterministic lesson
   identity, durable exact graph+episode lesson saga, bounded multi-turn CLI
   retry, fake-teacher integration, real Codex empty-DB DOUBLE(7)->DOUBLE(9)
   smoke, and strict gates. Cycle tests were 29/29 and teacher tests 19/19 before
   the final saga/CLI work; current workspace compiles.
3. Resume `/root/phase2_persistence`: finish the structurally diverse hidden
   metric-7 corpus. Trusted persisted SHA-256 simulated receipts and exact cost
   accounting were at 24/24 credit tests. Metric-8 honestly measured 0.829 and
   therefore still fails the required <0.5 threshold; coordinate with the
   episode owner to implement transactional evidence revision/indexed
   per-procedure/co-occurrence aggregates rather than changing the denominator.
4. Resume `/root/teacher_hardening`: complete independently found persistence
   fixes (finalized-only reads already landed; contract-call dependencies,
   versioned safe retirement, typed dependency direction, lifecycle traversal
   were landing), claim-specific alternative support, durable no-op receipts,
   then admin CAS RPCs, admin contradiction mutation, and structured subcodes.
5. Activate `/root/phase0_exit_audit` for the separate plan in
   `phase2-remediation/trust-boundary-plan.md`: read-only Engine facades and
   trust ledger; immutable failed attempts + durable pending cycles; database
   active-cycle/maintenance leases; semantically bound observed facts,
   automatic contradiction/refinement, and uncertainty in reasoning.
6. Cross-audit every remediated scope again, repeat complete Rust/TS gates,
   then commit Phase 2
   conventionally (excluding `WHAT-IS-EKG-v3.md`), update this file, then create
   Phase 3 scratchpads.

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
- Contract violations establish definite misuse, not necessarily a unique
  cause. Statistical attribution only ranks. Deterministic replay is decisive
  only after the engine binds a precommitted immutable Hard/Consensus oracle
  and reproduces the exact unmodified trace. Caller `expected` values are
  suggestions and are explicitly ignored. Simulated replay is inconclusive
  until an engine-issued trusted simulator receipt exists. Every attribution
  retains its mechanism, confidence, provenance, and raw cost.
- Prefer the narrowest correction supported by evidence. Statistical suspicion
  never mutates knowledge. Broad/conceptual restructuring is offline only.
- Historical episodes remain immutable; reconciliation revises current
  understanding and follows dependencies without cascade deletion, checking
  alternative support first.
- Unresolved contradictions are durable first-class objects whose uncertainty
  propagates. Never silently average them into one confidence scalar.
- The immutable core includes evaluation, credit rules, mutability classes,
  promotion gates, primitives, and goals. Runtime learning must not rewrite it.

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
