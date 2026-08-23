# Reason Foundation Context

## Scope

Implement Phase 1.2 interpretation and Phase 1.3 context assembly as the new
`spoon-reason` Rust crate. The work is autonomous and must not edit the engine,
server, or shared product documentation.

## Requirements

- Represent weighted graph-concept interpretations without collapsing ambiguity.
- Treat an unresolved/unknown candidate as an ordinary explicit graph concept.
- Reject empty, duplicate, non-finite, negative, or non-normalized candidates.
- Allow no selected candidate; when selected, preserve chosen and losing meanings
  when converting to `spoon_core::Interpretation` episode records.
- Assemble deterministic bounded working context from interpreted concepts,
  graph relationships, recent episodes, marked assumptions, environment state,
  and remaining budget.
- Traverse only configured relationship kinds and enforce every configured limit.

## Existing Patterns

- Workspace crates use Rust 2024 and shared path dependencies from root
  `Cargo.toml`.
- `spoon-core` owns serialized domain types, including episode interpretation and
  minimal assembled-context records.
- `spoon-graph::KnowledgeStore` exposes directional typed relationships and concept
  lookup.
- `spoon-episode::EpisodeStore` exposes recent episodes in reverse chronological
  order.
- Errors are crate-local enums implemented with `thiserror`.

## Dependency Map

`spoon-reason` -> `spoon-core` for IDs, values, assumptions, and episode conversion

`spoon-reason` -> `spoon-graph` for relevant typed neighborhoods

`spoon-reason` -> `spoon-episode` for recent actions and results

Future `spoon-engine` wiring may consume `InterpreterOutput`, `ContextRequest`, and
`ContextAssembler`, but is outside this slice.

## Decisions

- Unknown is not a magic/null identifier. The teacher or graph supplies the ID
  of the explicit `UNKNOWN` concept, preserving provenance and serialization.
- Candidate order is retained for teacher intent; selected state is represented
  separately and is optional.
- Neighborhood expansion is bidirectional because discussion relevance is not
  equivalent to dependency direction. Each discovered relationship retains its
  direction and kind.
- Bounded output uses stable ordering by hop, interpretation weight, relationship
  kind, and UUID, avoiding storage-order dependence.

## Uncertainty

The core assembled-context type is intentionally minimal. This crate therefore
exposes a richer `KnowledgeContext` and an explicit lossy conversion to the core
episode context for current persistence compatibility.
