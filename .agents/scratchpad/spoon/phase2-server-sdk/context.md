# Phase 2 Server and SDK Context

## Scope and mode

- Auto mode: no routine user interaction.
- Owned implementation paths for the remediation pass: `crates/spoon-server`,
  `packages/sdk`, and `packages/cli` only.
- Do not modify engine, core, graph, episode, credit, adapt, or workspace
  manifests.
- No commit.

## Requirements

- Expose adaptation planning, idempotent application, and lookup through JSON-RPC.
- Expose held contradiction list and contradiction lookup through JSON-RPC.
- Mirror every RPC in the TypeScript SDK with exact camelCase params and response types.
- Rust request DTOs reject unknown fields recursively where locally controlled.
- Preserve structured JSON-RPC errors: invalid params use `-32602`; engine/domain failures use application codes and machine-readable data when available.
- Never let the server bypass the engine's mutation authority or construct opaque adaptation authorization itself.

## Existing patterns

- `RpcServer` owns `Engine` and dispatches one-line JSON-RPC 2.0 requests.
- Method names use dotted domains (`credit.analyze`, `cycle.begin`).
- Params are decoded through Serde, and selected sensitive DTOs use `rename_all = "camelCase", deny_unknown_fields`.
- `SpoonClient` is a thin typed transport wrapper; tests assert exact method and params.
- The server and SDK already expose feedback and credit analysis.

## Dependency map

`SpoonClient` -> JSON-RPC transport -> `RpcServer` -> `Engine` -> `spoon-adapt`/persistent stores.

The settled engine exports persisted plan/get/online-apply operations and owns the canonical contradiction store. The server delegates through those methods so canonical episode evidence, opaque authorization, persistence, and offline capability rules cannot be bypassed.

## Existing documentation

- `IMPLEMENTATION-PLAN.md` Phase 2 defines narrow evidence-gated correction, non-destructive reconciliation, and first-class contradictions.
- `.agents/scratchpad/spoon/HANDOFF.md` requires explicit persisted plans and idempotent application, with server/SDK mirroring engine operations.
- No repository `CODEASSIST.md` or task-relevant README was found by the required discovery command.

## Settled trust boundary

- `ApplyAdaptationRequest` contains only plan ID, idempotency key, and timestamp. Remote apply is online-only.
- Broad apply requires an opaque, non-Serde engine capability. The only remote
  surface is the admin-gated `adaptation.applyOffline` maintenance operation;
  it issues and consumes the capability inside the server's serialized mutable
  dispatch and never exposes the capability.
- Contradiction list/get are read-only and use the engine-owned store; the server does not open a second store.

## Remediation trust boundaries

- Raw graph CRUD mutations require a configured server-side bootstrap token;
  no configured token means the methods are disabled. The transport-only token
  is removed before strict request decoding.
- Public feedback accepts only an episode, raw observation, and idempotency key.
  The server assigns Deferred trust and its configured source identity.
- Credit analysis retry keys are transport metadata stripped before decoding
  the engine request. Stored analyses are readable by immutable analysis ID or
  explicit idempotency key.
- Contradiction record/refine operations delegate to Engine, which verifies
  canonical stored episode evidence. Claim uncertainty is read-only.
