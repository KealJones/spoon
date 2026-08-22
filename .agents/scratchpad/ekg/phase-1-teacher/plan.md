# Phase 1 implementation plan

## Acceptance tests

1. All four teacher adapters produce the same unverified proposal schema and preserve source provenance.
2. Malformed proposals are rejected; deterministically checkable proposals may be verified; weak proposals remain provisional.
3. Source reliability updates from validated/rejected outcomes without collapsing to an unsupported scalar truth claim.
4. Interpretation preserves multiple candidates, rejects invalid weights, and records losing candidates in episodes.
5. Context assembly includes the goal, graph neighborhood, recent outcomes, explicit assumptions, environment, and remaining budget under a hard bound.
6. The cycle resolves known procedures at RUN, returns an ASK continuation when teacher help is needed, and records ABSTAIN when help is unavailable or invalid.
7. “what is double 7?” succeeds end to end through a teacher proposal and then succeeds without the teacher once the validated procedure exists.

## Sequence

- Build provider-neutral teacher types, validation, reliability, and injected adapters.
- Add Rust interpretation and context services.
- Add resumable cycle states and episode recording.
- Expose cycle JSON-RPC and SDK methods.
- Wire CLI teacher selection and end-to-end tests.
- Run independent exit audit and complete verification gate.
