# Teacher Hardening Context

## Scope

Harden `packages/teacher` without changing Rust, SDK, CLI, or server code.

## Requirements

- Reject forged or stale proposals whose runtime envelope is not an unverified proposal bound to the validating request.
- Require source and provider provenance to agree.
- Provide a default validation path whose reliability updates are visible through the teacher that produced the proposal.
- Compare JSON values semantically, independent of object key insertion order, and treat required properties as own properties.
- Normalize provider transport, prompt, and command failures as provider-attributed `TeacherError` instances.

## Existing Patterns

- Provider adapters use injected I/O for deterministic tests.
- `ProposalValidationPipeline` owns schema and independent validation.
- `SourceReliabilityTracker` uses a conservative Beta prior.
- Tests use Node's built-in test runner through `tsx`.

## Dependency Map

Provider adapter -> raw `TeacherProposal` -> validation pipeline -> validated proposal

Teacher-owned reliability tracker -> teacher-created validation pipeline -> reliability visible from `teacher.reliability()`

## Decisions

- Keep standalone pipelines for multi-source callers, while adding `validationPipeline()` on each teacher as the safe, connected default.
- Treat provenance binding as a first validation check before schema or independent validators.
- Preserve arrays as ordered JSON values while comparing object keys structurally.
- Wrap only boundary failures; preserve an existing `TeacherError` unchanged.

