# Phase 2 context — Credit assignment and adaptation

## Objective

Implement P2.1 through P2.6 from `IMPLEMENTATION-PLAN.md`: identify a failed
episode's responsible element with bounded evidence, apply only the narrowest
justified correction, reconcile dependents without deletion, and refine or hold
contradictions as first-class knowledge.

## Dependencies from Phase 0–1

- Immutable, version-pinned procedure and contract history in `spoon-graph`.
- Complete episode records with losing interpretations, marked assumptions,
  reasoning/contract checks, lossless execution trace, teacher provenance, and
  cost.
- Deterministic episode replay with substitutions.
- Typed dependency reports and lifecycle states.
- Tiered evaluation and surprise signals.

## Planned boundaries

- `spoon-credit`: pure/read-only attribution analysis: contract violations,
  cross-episode statistics, ranked suspects, bounded counterfactual replay,
  attribution evidence/confidence/cost.
- `spoon-adapt`: correction policy and graph mutation: record-only, assumption
  correction, scope narrowing, procedure challenger/replacement, offline
  concept revision, reconciliation, contradiction refinement/holding.
- `spoon-engine`: orchestration only; calls credit then adaptation after a failed
  terminal episode and records the resulting audit trail.

## Non-negotiable invariants

- Statistical correlation is a ranked suspicion, never a conclusion.
- Only one candidate changes per counterfactual replay.
- Replay is top-K and budget bounded; cost is recorded even when inconclusive.
- Historical versions and failed episodes are immutable and never deleted.
- Correction width cannot exceed its evidence threshold or attribution strength.
- Assumption failures do not rewrite a procedure.
- Reconciliation checks alternative justifications before marking dependents.
- Contradictions are never silently averaged or winner-take-all deleted.
- Concept revision remains offline and highly corroborated.

