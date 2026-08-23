# SPOON implementation context

## Objective

Implement every phase and exit criterion in `IMPLEMENTATION-PLAN.md`. Work runs in auto mode and uses phase-level commits as checkpoints.

## Existing foundation

- Rust 2024 workspace with six crates: core, graph, exec, episode, engine, and server.
- `spoon-core` defines the neutral expression IR and knowledge/episode data model.
- `spoon-graph` and `spoon-episode` persist JSON-rich records in SQLite with indexed lookup fields.
- `spoon-exec` evaluates pure expressions with lexical scopes, bounded steps, and procedure-call traces.
- `spoon-engine` and `spoon-server` are stubs.
- No TypeScript workspace exists yet.
- Baseline: workspace build succeeds; 32 tests pass.

## Requirement gaps at takeover

- Phase 0: executable contract checks, replay, evaluation, version history, server, SDK/CLI, and kitchen integration test.
- Phases 1-6: not started.
- The historical design archive is not part of the committed tree;
  `IMPLEMENTATION-PLAN.md` is the controlling local specification.

## Dependency map

`spoon-core` is dependency-free within the workspace. `spoon-graph`, `spoon-exec`, and `spoon-episode` consume core types. `spoon-engine` orchestrates those services. `spoon-server` exposes the engine. The TypeScript SDK owns JSON-RPC transport; CLI, teacher, and inspector build on the SDK and protocol types.

## Working conventions

- Test-first development for each bounded feature.
- Preserve failed episodes and immutable historical evidence.
- Prefer exact procedure-version snapshots for deterministic replay.
- Keep SQLite as one logical database where atomic cross-store work is required.
- No remote push; phase-level conventional commits only after build and tests pass.
