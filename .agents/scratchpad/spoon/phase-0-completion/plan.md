# Phase 0 completion plan

## Acceptance tests

1. Contracts reject unmet executable preconditions and failed promises while recording checks in traces.
2. A stored execution can replay against the exact procedure version with substituted inputs.
3. Deterministic, consensus/inverse, and deferred observations produce Tier 1/2/3 evaluations with surprise signals.
4. Updating a procedure preserves queryable procedure and contract history.
5. JSON-RPC over stdio supports graph CRUD, execution, and episode queries with structured errors.
6. TypeScript SDK and CLI can define and run `DOUBLE` through the server.
7. Kitchen integration test executes `DOUBLE(7)`, records/evaluates the episode, and replays with a substitution.

## Implementation sequence

- Pin stable serialization and versioned execution records in core.
- Add graph version history and transactional database access.
- Add contract-aware execution and deterministic replay.
- Add evaluation and episode replay services.
- Wire the engine orchestration boundary.
- Implement JSON-RPC server, TypeScript SDK, and CLI.
- Add end-to-end kitchen tests and validate the entire workspace.

## Risks and mitigations

- Historical replay drift: snapshot versioned procedures and resolve calls by version.
- Contract expressions need result access: expose reserved contract bindings explicitly.
- Rust/TypeScript schema drift: exercise the real stdio protocol in integration tests.
- Parallel edit collisions: delegate by crate and integrate shared manifests centrally.
