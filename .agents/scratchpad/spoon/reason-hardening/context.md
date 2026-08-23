# Reason Hardening Context

## Scope

Harden the Phase 1 interpretation and context boundary without changing the
engine, server, or TypeScript packages. Preserve provider-neutral teacher
interaction persistence already being added to `Episode`.

## Requirements

- Deserialized interpretation sets cannot widen normalization tolerance.
- Interpretation and context collections have absolute ceilings in addition to
  caller-selected working limits.
- Episode context preserves every Phase 1 working-context category.
- Context history favors episodes connected to active entities.
- Context includes relevant procedure metadata and excludes inactive graph
  relationships, adjacent concepts, and procedures.

## Existing Patterns

- `spoon-core` owns persisted episode data.
- `spoon-reason` owns validation, bounded selection, and graph/history assembly.
- `KnowledgeStore` exposes current graph records and procedure listings.
- `EpisodeStore::find_by_concept` provides indexed, newest-first relevant history.

## Dependency Map

`spoon-reason::KnowledgeContext` -> `spoon_core::AssembledContext` for lossless
episode persistence.

`ContextAssembler` -> graph lifecycle-filtered neighborhoods and procedures.

`ContextAssembler` -> concept-indexed history, with bounded global backfill.

## Decisions

- Public configurable normalization tolerance remains supported, but is capped
  at the largest intended floating-point accommodation.
- Absolute ceilings reject hostile configuration rather than silently granting
  an effectively unbounded context.
- Persisted procedure context is bounded metadata, not executable bodies.
- Active context accepts Active, Validated, Provisional, and UnderReview
  records; Stale, Superseded, Retired, and Invalid records are excluded.
