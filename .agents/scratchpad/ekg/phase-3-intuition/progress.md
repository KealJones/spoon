# Phase 3 progress

## P3.1 Recall index

- [x] Added `ekg-intuition` with a deterministic fixed-dimensional
  representation and SQLite inverted-term index.
- [x] Candidate generation is term-posting bounded and capped before vector
  scoring; it does not scan all graph/episode rows.
- [x] Concepts, procedures, and finalized episodes are indexed by Engine
  writes and rebuilt on file-backed reopen.
- [x] Normal cycle exact recall now retrieves bounded episode candidates before
  loading/verifying exact episode bytes.

## P3.2 Learned ranking

- [x] Added persisted ranking examples from the Engine API.
- [x] Ranking combines similarity, recency, frequency, activation, and a
  smoothed candidate outcome rate; it changes ordering only.
- [x] Ranking remains separate from trust, lifecycle, and belief mutation.

## P3.3/P3.4 self-supervision

- [x] Added representation supervision tasks with source/target JSON,
  provenance, and grounding status.
- [x] Added epistemic challenge kinds for hidden computation, inverse/round
  trip, contract boundary, and consequence prediction.
- [x] Ungrounded epistemic challenges are rejected; Engine-grounded tasks and
  challenges require an exact trusted source episode receipt and cannot
  promote claims or alter graph state.
- [x] Added grounding-ratio and intuition metrics.
- [x] Added bounded typed relationship activation spread with direction,
  decay, hop/fan-out budgets, lifecycle filtering, and path provenance.
- [x] Wired ranked recall into local interpretation selection and require an
  exact Engine trust receipt before an episode can answer at recall rung.
- [x] Finalized episodes emit ranking examples for considered concepts and
  relevant procedures; ranking outcomes are query-conditioned and bounded.
- [x] Rebuilds purge stale intuition documents; concept/procedure deletes and
  revisions reconcile their retrieval records.

## Verification

- `cargo test --workspace --all-targets` — pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — pass.
- Focused Engine intuition tests: 2 pass; crate unit tests: 3 pass.

Remaining Phase 3 work: add held-out baseline-vs-ranked/rung-distribution
evidence and harden semantic representation training/versioning before
claiming the Phase 3 exit criteria.
