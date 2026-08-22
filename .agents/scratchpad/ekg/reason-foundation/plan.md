# Reason Foundation Plan

## Test Strategy

- Valid interpretation: candidates `0.91/0.06/0.03`, including explicit unknown,
  remain in input order and convert to one chosen plus two losing episode rows.
- Unresolved interpretation: no chosen concept is accepted and all episode rows
  remain unchosen.
- Invalid interpretation: empty input, duplicates, missing selection, NaN,
  infinity, negative values, and totals outside tolerance are rejected.
- Tolerance: a small floating point sum error is accepted.
- Context happy path: goal, entities, relevant typed graph neighbors, recent
  action/result, marked assumption, environment, and budget are all present.
- Typed filtering: unrelated relationship kinds do not enter context.
- Determinism: repeated assembly produces the same ordered context.
- Hard bounds: long goals/environment strings and oversized entity,
  relationship, recent-episode, assumption, and environment collections are
  truncated to configured limits.
- Invalid config/input: zero-size required limits and unmarked assumptions are
  rejected.
- Core conversion: rich context maps predictably to `AssembledContext`.

## Implementation Tasks

- [x] Add workspace member/dependency and crate manifest.
- [x] Write all public-behavior tests before implementation.
- [x] Run tests and record expected RED failure.
- [x] Implement interpretation validation and episode conversion.
- [x] Implement context models, limits, validation, graph selection, and history.
- [x] Run crate tests, refactor, format, and run strict linting.
- [x] Run workspace compile/tests when concurrent work permits.
- [x] Report public API without committing.

## Risks and Mitigations

- Concurrent root manifest edits: make a minimal patch and inspect after edits.
- SQLite ordering ties: normalize all assembled output with explicit stable sorts.
- Unbounded input strings: apply character-aware truncation at assembly time.
- Graph cycles: track visited concepts and cap hops and retained relationships.
