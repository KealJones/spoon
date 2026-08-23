# Implementation plan audit

Last audited: 2026-08-22 (Spoon rename + Section 38 cohort hardening)

This is an evidence-based checkpoint, not a claim that every research-grade
exit criterion is finished.

## Phase status

| Phase | Status | Evidence / remaining gap |
| --- | --- | --- |
| P0 Seed | Complete | Core, graph, execution, episode, evaluation, server/CLI, and full tests are present. |
| P1 Teacher | Complete | Teacher abstraction, bounded continuation cycle, validation/provenance, CLI, and provider adapters are present. |
| P2 Credit + adaptation | Complete with recorded audit caveat | Contract/statistical/replay attribution, narrow adaptation, reconciliation, contradictions, durable sagas, and trust receipts are implemented. See `phase2-final-audit.md` for the independent-audit limitation. |
| P3 Intuition | Substantially implemented, not strict-complete | Recall has bounded deterministic co-occurrence semantic expansion, ranking, held-out ranking and cross-query recall evidence, trusted retrieval, grounding/self-supervision, and bounded representation artifacts that affect retrieval. It is intentionally not an external embedding service; evaluation measures candidate coverage and search evidence, not truth. |
| P4 Consolidation | Substantially implemented, not strict-complete | Repetition/single-success/failure-critic discovery emits executable evidence artifacts; broad procedure/concept changes run a durable regression suite; managed-skill replay inputs are bound to source episodes; retirement requires a promoted, live-verified successor; compression is non-destructive. A fully engine-derived managed-skill challenger runner and behavioral subsumption evidence remain open. |
| P5 Curiosity/capabilities/self-modification | Strong foundation, not strict-complete | Curiosity, goal lineage, immutable standing goals, native primitives, typed discovery, quarantine bundles, dependency-DAG reconstruction, atomic import/revalidation, local trust, revocable grants, adapter-backed invocation, and admin/offline adaptation exist. A generalized neutral-IR sandbox runner and complete structural search-policy scheduling/rollback remain open. |
| P6 Inspector/metrics | Inspector and telemetry paths implemented; thesis-level measurement not claimed | The inspector and CLI expose redacted narratives and a Section 38 projection. Durable falsification runs expose all twelve metric slots with sample sizes and explicit insufficient-evidence states. Held-out family separation, teacher-off leakage, exact-repeat exclusion, failures, abstentions, and clarifications are enforced. The fixture corpus is still deliberately small, so no claim is made that the flywheel is empirically turning. |

## Verification

The latest gates passed on 2026-08-22:

- `cargo test --workspace --all-targets --quiet`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `pnpm test`
- `pnpm build`
- `pnpm typecheck`
- `pnpm depcheck`
- `git diff --check`

The public product name is now Spoon. Internal `ekg-*`, `@ekg/*`, and `EKG_*`
identifiers remain compatibility names and are documented as such.

The worktree intentionally retains the pre-existing whitespace-only change in
`crates/ekg-engine/src/runtime.rs`. User-owned untracked reference/archive
files and agent scratchpads are not part of the implementation commits.

## Next highest-value work

1. Add a fully engine-derived managed-skill challenger runner so promotion
   cannot accept caller-supplied challenger outcomes as authority.
2. Add explicit known-gap records for any intentionally omitted compression
   information and behavioral subsumption evidence for retirement.
3. Exercise capability discovery end-to-end from authorized interface
   observation through typed synthesis, sandbox tests, revalidation, grant,
   invocation, and failure receipts on a clean instance.
4. Gate structural search-policy changes with the same slow/offline regression
   authority used for broad procedure and concept changes.
5. Expand `tests/falsification` into a representative benchmark corpus before
   making a thesis-level compounding/transfer/weaning/ablation claim.
