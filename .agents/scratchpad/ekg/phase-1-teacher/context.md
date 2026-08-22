# Phase 1 context

## Objective

Implement P1.1-P1.4: provider-independent teaching, weighted interpretation, bounded context assembly, and the recall/run/ask/abstain reasoning cycle with complete episodes.

## Phase 0 boundary

Phase 0 is complete at `0e48cfa`. The Rust engine owns graph, execution, evaluation, replay, and episodes. The Rust server exposes newline-delimited JSON-RPC. The TypeScript SDK and CLI own interaction and process transport.

## Dependency map

- `@ekg/teacher` owns provider adapters and proposal validation; it must never mutate graph state directly.
- `ekg-engine` owns interpretation/context/cycle state and treats teacher output as unverified input.
- `ekg-server` exposes begin/resume cycle methods.
- `@ekg/sdk` transports teacher proposals and cycle results.
- `@ekg/cli` selects a teacher, performs ASK, and resumes the engine.

## Provider guidance

- Claude: `claude -p` process adapter with injected runner and structured JSON prompt.
- OpenAI: Responses API with Structured Outputs, using the current official API shape and injected fetch/client boundary.
- Ollama: local HTTP adapter with injected fetch.
- Human: injected readline/prompt boundary.
- Every proposal includes provenance and begins as unverified.

## Constraints

- Ambiguity remains weighted and losers are recorded.
- Teacher output is never automatically true.
- Context is bounded and assumptions are explicitly marked.
- Every terminal attempt records an episode, including abstention.
- External-provider availability and credentials are not required for unit tests.
