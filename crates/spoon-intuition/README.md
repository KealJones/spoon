# spoon-intuition

Phase 3 retrieval, ranking, and representation-learning primitives.

## Owns

- Bounded lexical/semantic recall candidates for concepts, procedures, and episodes.
- Local ranking models trained from retrieval outcomes.
- Held-out ranking/recall evaluation and grounded representation supervision.

## How it works with the system

`spoon-engine` feeds persisted episodes and graph documents into this crate and
uses its results to make search cheaper. The outputs are search-policy artifacts,
not truth claims: intuition may reorder candidates or update representations,
but it cannot mint trust, resolve contradictions, or change graph lifecycle state.
