# Implementation plan audit

Last audited: 2026-08-22

This is an evidence-based checkpoint, not a claim that every research-grade
exit criterion is finished.

## Phase status

| Phase | Status | Evidence / remaining gap |
| --- | --- | --- |
| P0 Seed | Complete | Core, graph, execution, episode, evaluation, server/CLI, and full tests are present. |
| P1 Teacher | Complete | Teacher abstraction, bounded continuation cycle, validation/provenance, CLI, and provider adapters are present. |
| P2 Credit + adaptation | Complete with recorded audit caveat | Contract/statistical/replay attribution, narrow adaptation, reconciliation, contradictions, durable sagas, and trust receipts are implemented. See `phase2-final-audit.md` for the independent-audit limitation. |
| P3 Intuition | Substantially implemented, not strict-complete | Recall, ranking, held-out ranking evidence, trusted retrieval, grounding/self-supervision, and bounded representation artifacts exist. Retrieval is still deterministic lexical/hash-based; no semantic embedding model or cross-query generalization evidence exists. |
| P4 Consolidation | Substantially implemented, not strict-complete | Repetition/single-success/failure-critic discovery, replay/shadow/live promotion, retirement, compression records, verified-answer regression history, and promoted-skill execution exist. Learned skill ranking/generalized neutral skill IR and continuous compounding/transfer/survival metrics remain. |
| P5 Curiosity/capabilities/self-modification | Foundation implemented, not strict-complete | Curiosity, goal lineage, immutable standing goals, native primitives, typed discovery, quarantine bundles, local trust/revalidation, revocable grants, and admin/offline adaptation exist. Full neutral-IR dependency-DAG reconstruction, generalized sandbox execution, and complete structural self-modification scheduling/rollback remain. |
| P6 Inspector/metrics | Inspector complete; metrics partial | Local dashboard, redacted narratives, raw drill-down, CLI explainability, rung distribution, and durable verified-answer baseline coverage exist. Grounding remains a partial signal; follow-up regression, compounding, transfer, weaning, attribution, ablation, survival, and calibration slots remain uninstrumented. |

## Verification

The latest gates passed on 2026-08-22:

- `cargo test --workspace --all-targets --quiet`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `pnpm test`
- `pnpm build`
- `pnpm typecheck`
- `pnpm depcheck`
- `git diff --check`

The worktree intentionally retains the pre-existing whitespace-only change in
`crates/ekg-engine/src/runtime.rs`. User-owned untracked reference/archive
files and agent scratchpads are not part of the implementation commits.

## Next highest-value work

1. Replace or augment the deterministic recall index with a bounded local
   semantic representation and cross-query held-out evaluation.
2. Add a learned/ranked skill policy and generalized neutral procedure IR,
   while preserving the promoted-skill execution gate.
3. Implement full capability bundle reconstruction (dependency DAG, neutral IR,
   fixtures, and clean-instance round trip) with local execution isolation.
4. Instrument the remaining Section 38 metrics with held-out anti-gaming
   fixtures rather than changing dashboard labels optimistically.
