# Phase 2 final adversarial audit

Date: 2026-08-22
Scope: the current workspace diff against `IMPLEMENTATION-PLAN.md` P2.1-P2.6,
the P2 exit criteria, and
`.agents/scratchpad/ekg/phase2-remediation/trust-boundary-plan.md`.

## Verdict (historical first pass)

**Not exit-clean.** The findings below are retained as the initial adversarial
pass and audit trail. They are not a statement of the current tree; the
remediation re-audit is recorded at the end of this file.

## Findings

### P0 / critical — opening a second Engine deletes a live cycle and can permit broad maintenance to overlap it

`Engine::open` unconditionally calls `discard_abandoned_running_cycles`
(`crates/ekg-engine/src/engine.rs:77-100`). That operation deletes **every** row
whose state is `running`, without an owner lease, heartbeat, age check, or process
liveness check (`crates/ekg-engine/src/runtime.rs:91-97`). A cycle is registered as
`running` before its synchronous reasoning/execution begins
(`crates/ekg-engine/src/runtime.rs:130-155`). Therefore, while Engine A is still
executing, Engine B can open the same database, delete A's live registration, and
then pass the zero-active-cycle check used to acquire broad-maintenance authority
(`crates/ekg-engine/src/runtime.rs:248-267`). Engine A subsequently either overlaps
the broad mutation or fails when it tries to save its continuation.

This violates the process-wide exclusion acceptance in the remediation plan in
the dangerous ordering. The existing cross-engine test only opens the second
Engine after the first cycle has reached `pending_teacher`, a state the deletion
query does not remove (`crates/ekg-engine/tests/adaptation.rs:673-740`), so it does
not exercise the live `running` window.

### P1 / high — raw Hard success still authorizes auxiliary adaptation and reconciliation evidence

The main failed evidence gate correctly requires an exact Engine trust receipt
(`crates/ekg-engine/src/adaptation.rs:1254-1339`), but two auxiliary evidence paths
bypass that ledger:

- Online narrowing's required admitted regression scans raw episodes and accepts
  any successful Hard/Consensus row with a matching trace and condition
  (`crates/ekg-adapt/src/policy.rs:651-688`), and the graph authorizer calls it
  directly (`crates/ekg-adapt/src/policy.rs:467-477`). An admin/raw-store-inserted
  Hard success can therefore complete authorization for an otherwise genuine
  failed episode and plan.
- Alternative support likewise accepts raw strong successful episodes and their
  claim-shaped context, without an Engine receipt
  (`crates/ekg-adapt/src/reconciliation.rs:79-110`). Engine reconciliation uses
  that provider during planning and stale-plan refresh
  (`crates/ekg-engine/src/adaptation.rs:852-860` and
  `crates/ekg-engine/src/adaptation.rs:1128-1133`). A forged Hard row can preserve
  dependent knowledge as “alternatively supported.”

This directly fails the remediation RED case that caller-created Hard rows must
not authorize narrow or alternative-support work. The trust-ledger tests cover a
forged **analyzed failure** (`crates/ekg-engine/tests/trust_ledger.rs:83-118`), not a
forged auxiliary successful regression or alternative justification.

### P1 / high — durable episode, trust receipt, contradiction, and teacher continuation are not one atomic operation

Engine episode persistence is three independent database operations: insert the
episode, mint its trust receipt, then detect contradictions
(`crates/ekg-engine/src/engine.rs:461-466`). A crash or error after insertion but
before receipt creation leaves a genuine strong Engine episode permanently
indistinguishable from a raw admin row. Startup reconciles contradictions but has
no safe receipt-repair pass (`crates/ekg-engine/src/engine.rs:92-100`). The same
split exists for authenticated feedback: append first, receipt second
(`crates/ekg-engine/src/engine.rs:256-268`).

There is a second crash window on teacher escalation. The failed attempt is
persisted, then the pending continuation is only built in memory
(`crates/ekg-engine/src/cycle.rs:1545-1580`); the durable `pending_teacher` update
happens later (`crates/ekg-engine/src/cycle.rs:737-754`). A crash between those
steps retains the analyzable failure but loses the resumable continuation, and
startup deletes the leftover `running` row. This fails the acceptance requirement
that the failed attempt and its resumable continuation survive the crash.

### P1 / high — expired staged broad work can make every Engine reopen fail

Startup automatically resumes every adaptation stage lacking a final receipt
(`crates/ekg-engine/src/engine.rs:92-99`,
`crates/ekg-engine/src/adaptation.rs:1090-1097`). For broad work, a recovered stage
loads its exact stored maintenance lease and immediately validates it
(`crates/ekg-engine/src/adaptation.rs:965-980`).
`maintenance_for_request` returns a matching lease even when it is expired
(`crates/ekg-engine/src/runtime.rs:329-365`), while `validate_maintenance` rejects
that lease (`crates/ekg-engine/src/runtime.rs:295-326`). Expired leases are cleared
only by beginning a new cycle or acquiring new maintenance
(`crates/ekg-engine/src/runtime.rs:130-136`,
`crates/ekg-engine/src/runtime.rs:248-258`, and
`crates/ekg-engine/src/runtime.rs:380-390`), neither of which is reachable when
`Engine::open` fails first. Reopening repeats the same failure indefinitely.

There is also a false-negative completion window: the receipt is durably inserted
before lease release (`crates/ekg-engine/src/adaptation.rs:1073-1086`), so lease
expiry at that point can return an error even though the requested adaptation has
already succeeded and been receipted.

### P1 / high — metric 8 hides O(history) work inside “constant row” payloads and does not account for index maintenance

The materialized element row stores episode IDs, scanned feedback IDs, used
feedback IDs, and conflict IDs as ever-growing JSON arrays. Every episode/feedback
update parses, mutates, serializes, and rewrites those full sets
(`crates/ekg-episode/src/store.rs:555-674`). Thus ingestion is O(history) for a
frequently used element, not constant-time materialization.

Analysis then reads and parses those same history-sized arrays
(`crates/ekg-episode/src/store.rs:1023-1097`), clones and deduplicates their contents
(`crates/ekg-credit/src/statistical.rs:268-315`), iterates them for provenance cost
metadata (`crates/ekg-engine/src/credit.rs:1543-1575`), and hashes the entire
snapshot (`crates/ekg-engine/src/credit.rs:1288-1306`). Nevertheless, cost counts
each aggregate row as one unit regardless of its byte/ID cardinality
(`crates/ekg-engine/src/credit.rs:273-281` and
`crates/ekg-credit/src/statistical.rs:312-322`), reports zero history episodes
scanned, and omits all materialized-index maintenance from both original and
attribution cost (`crates/ekg-engine/src/credit.rs:1390-1435`). This relocates and
hides history work rather than measuring it honestly.

The 3x3 acceptance test only requires the three five-step points to be below 0.5,
explicitly requires at least one point to remain at or above 0.5, and never checks
payload bytes or ingestion work (`crates/ekg-engine/tests/credit_cycle.rs:250-343`).
The recorded current curve is 0.667, 0.576, and 0.490 by trace length, repeated
across history sizes
(`.agents/scratchpad/ekg/metric8-scaling/progress.md:16`). The apparent
history-size invariance is an artifact of charging rows rather than the work
inside them. Metric 8 cannot support the Phase 2 exit claim in this form.

### P1 / high — contradiction refinement is manual, inherited dependency uncertainty is not consumed by cycles, and trusted external facts cannot enter the fact model

Automatic fact conflict handling only records a held contradiction
(`crates/ekg-engine/src/engine.rs:485-506`). It does not search the two recorded
scopes and apply a demonstrated discriminator, even when one is already present;
the pancake exit test manually calls `refine_contradiction`
(`crates/ekg-engine/tests/phase2_pancake.rs:434-503`). This does not implement the
P2.6 “search, then split if found” behavior.

The store can calculate transitive uncertainty through `claim_dependencies`
(`crates/ekg-adapt/src/contradiction.rs:335-353` and
`crates/ekg-adapt/src/contradiction.rs:436-475`), but cycle assembly never queries
that graph. It only checks the direct predicates of currently interpreted concepts
(`crates/ekg-engine/src/cycle.rs:1766-1807`). Consequently, the acceptance case
“dependent reasoning reports inherited uncertainty” is only demonstrated by a
direct API query, not by reasoning/cycle behavior
(`crates/ekg-engine/tests/adaptation.rs:1186-1215`).

Finally, `ObservedFact` contains only predicate, value, and raw scope
(`crates/ekg-core/src/episode.rs:153-184`): there is no immutable fact ID, source
attempt, verifier, evidence tier, environment digest, or fact-level trust receipt.
Claims point back to episodes rather than fact IDs
(`crates/ekg-engine/src/engine.rs:524-530`). Authenticated verifier feedback can
mint only a feedback receipt and has no operation to create/promote an exact
observed fact (`crates/ekg-engine/src/engine.rs:256-268`). This leaves the external
fact path required by the trust-boundary plan unimplemented.

### P1 / high — SDK/CLI contradiction mutations cannot satisfy the server's admin contract

The server correctly marks `contradiction.record` and `contradiction.refine` as
admin-only (`crates/ekg-server/src/lib.rs:559-574`). The SDK sends both requests
without `withAdminToken`, even when the client was configured with an admin token
(`packages/sdk/src/client.ts:234-244`). The CLI delegates directly to those SDK
methods (`packages/cli/src/main.ts:80-87`), so both advertised CLI commands fail
against the real server even with `EKG_ADMIN_TOKEN` configured.

The tests mask the incompatibility in opposite ways: SDK recording tests expect
no token and even label the group “read-only”
(`packages/sdk/test/client.test.ts:331-378`), while the Rust RPC helper injects the
admin token independently (`crates/ekg-server/tests/rpc.rs:21-40`).

### P2 / medium — embedding admin/manual-operation boundaries do not match the remediation design

The RPC layer gates manual record/refine, but the public Engine methods do not call
`require_admin` (`crates/ekg-engine/src/adaptation.rs:1139-1213`). In particular,
`add_claim_dependency` lets any embedding caller add arbitrary support edges
without proving either claim exists (`crates/ekg-engine/src/adaptation.rs:1163-1171`),
which can corrupt inherited uncertainty queries.

`issue_offline_capability` is also public and validates neither that the plan
exists nor that it is broad before reserving database-wide maintenance
(`crates/ekg-engine/src/adaptation.rs:904-913`). An ordinary embedding caller can
therefore acquire a five-minute lease for a fabricated request and block all
cycles. The test suite itself acquires such a lease using a nil plan ID
(`crates/ekg-engine/tests/adaptation.rs:713-740`). Admin authority is a reusable
boolean on the Engine handle (`crates/ekg-engine/src/engine.rs:147-160`), not the
opaque one-shot operation authority and durable admin receipts specified by the
remediation plan.

### P2 / medium — simulated replay receipts are content-bound but the “engine-selected simulator” boundary is caller-controlled and the intended strong-evidence path is inert

`issue_simulated_replay_receipt` is public and accepts an arbitrary caller-supplied
`SimulatedReplayModel` implementation (`crates/ekg-engine/src/credit.rs:991-1005`).
The caller chooses the reported model identity and simulation result; the Engine
does not resolve that identity through a configured/approved simulator registry
before minting the immutable receipt (`crates/ekg-engine/src/credit.rs:1098-1148`).
The receipt is exact and tamper-resistant after issuance, but “Engine-minted” is
not evidence that the simulator was Engine-selected.

This is not currently a forged graph-mutation path: promotion caps simulated
evidence at Medium and non-decisive (`crates/ekg-engine/src/credit.rs:1675-1720`),
while `AttributionStrength::from` recognizes replay evidence only at High/Certain
(`crates/ekg-adapt/src/policy.rs:45-67`). The consequence is instead architectural:
simulated replay can neither be trusted as described nor reach the “strong
evidence” role promised by P2.3. It is currently a stored advisory observation.

### P2 / medium — metric 7's reported 100% is a contract-pointer benchmark, not a held-out composed-credit benchmark

All six localized corpus cases use one code template and inject the ground-truth
fault by placing an explicit rejecting contract on leaf, middle, or root
(`crates/ekg-engine/tests/attribution_metrics.rs:104-192`). The “held-out families”
change names, values, and placement but not task mechanism or generation template
(`crates/ekg-engine/tests/attribution_metrics.rs:298-369`). The scanner can read the
culprit directly from the violated contract, so the asserted 6/6 top-1 result does
not measure localization of body/operator faults, statistical-only faults, or
replay-resolved faults. The seventh case measures one honest abstention for a
repeated-call ambiguity (`crates/ekg-engine/tests/attribution_metrics.rs:194-242`).

The pancake test does exercise one deterministic body patch, but it is a single
in-sample scenario whose candidate is explicitly described as the known hidden
oracle (`crates/ekg-engine/tests/phase2_pancake.rs:176-192`). Metric 7 should not be
reported as general attribution accuracy until the corpus includes genuinely
held-out task/mechanism families and publishes failures separately.

## What is working

- P2.1 contract inspection is trace-bounded and the scanner covers requires,
  promises, and triggered conditions.
- Statistical evidence is labeled suspicion and is not directly decisive.
- Deterministic replay is version-pinned, one-patch, top-K/budget bounded, and
  provenance-bound.
- The primary adaptation evidence gate rejects raw strong analyzed failures.
- Graph history/version snapshots are preserved, and lifecycle reconciliation
  applies compare-and-swap change sets without deletion.
- Exact predicate/value matching and demonstrated environment checks prevent the
  old unrelated-Boolean contradiction forgery.
- Read-only Engine graph/episode facades and strict JSON wire decoding are in
  place.

Those controls are valuable, but they do not neutralize the blockers above.

## Verification performed

All checks were read-only with respect to implementation code:

- `cargo test --workspace` — passed.
- `pnpm test` — passed (SDK 13, teacher 19, CLI 15).
- `pnpm typecheck` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.

The green baseline confirms that the findings are missing adversarial coverage or
architectural gaps, not ordinary compile/test failures.

## Remediation re-audit (2026-08-22)

The root agent rechecked each original blocker against the current sources and
added regression coverage. This is explicitly a root re-audit, not an
independent subagent sign-off; all delegated agents were unavailable after
exhausting their usage.

| Original finding | Current evidence | Status |
| --- | --- | --- |
| Live `running` cycles could be deleted on open | Startup no longer deletes live registrations; `opening_a_second_runtime_cannot_erase_a_live_running_cycle` proves a second runtime cannot acquire maintenance while the cycle is live. | Resolved, fail-closed stale-cycle policy |
| Raw Hard success authorized regression/alternative support | `trusted_strong_episode_ids` gates both paths; `raw_success_cannot_supply_a_trusted_regression_for_narrow_adaptation` crosses separate stores and proves the raw success is rejected. | Resolved for Engine mutation paths |
| Episode/trust/teacher continuation crash windows | `engine_episode_sagas`, atomic pending-cycle staging, and `durability` recovery tests cover insertion-before-receipt and failed-attempt continuation. | Resolved |
| Authenticated feedback crash window | `engine_feedback_sagas` plus `startup_finishes_an_authenticated_feedback_saga_after_insert_before_receipt` cover restart recovery. | Resolved |
| Expired or newer-owner maintenance lease handling | Staged recovery reacquires expired leases; valid foreign owners are rejected; owner-scoped receipt retry cannot clear a newer owner. | Resolved |
| Metric 8 hid history-sized JSON work | Normalized scalar aggregates, feedback summaries, and explicit maintenance-work counters are used by online analysis; cost tests cover history/trace scaling. | Resolved for online path; explicit provenance reads remain intentionally materialized |
| Contradiction refinement/dependency consumption | Automatic single-feature refinement, held ambiguity, transitive dependency lookup in cycle context, and adaptation tests cover both refinement and inherited uncertainty. | Resolved |
| External facts lacked identity/provenance/trust | `ObservedFact` carries stable ID, source episode, verifier, tier, environment digest; `TrustEvidenceKind::Fact` receipts and authenticated-observation tests cover promotion. | Resolved |
| SDK/CLI admin contradiction mutations lacked auth | SDK wraps privileged contradiction calls with the configured admin token; server/SDK tests pass. | Resolved |
| Metric 7 was only contract-pointer localization | Held-out deterministic body/operator fault and explicit failure-family reporting were added alongside contract, replay, and honest abstention cases. | Improved; current corpus is still small and should grow in P3/P4 |

Current verification evidence:

- `cargo test --workspace --all-targets` — pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — pass.
- `pnpm test`, `pnpm typecheck`, `pnpm build`, and `pnpm depcheck` — pass.
- Focused durability (3), trust ledger (5), adaptation (16), cycle (33), and
  attribution metric tests pass.

Phase 2 can be checkpointed as implementation-complete with the explicit
limitation that the independent subagent audit was unavailable. The remaining
high-risk concern intentionally carried forward is simulator selection: caller
provided simulated replay remains advisory/non-decisive, not authority for
mutation or promotion.
