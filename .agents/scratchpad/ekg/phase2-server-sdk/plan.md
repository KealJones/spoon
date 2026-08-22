# Phase 2 Server and SDK Plan

## RED scenarios

1. SDK maps `adaptation.plan`, `adaptation.apply`, and `adaptation.get` using exact camelCase params.
2. SDK maps `contradiction.list` and `contradiction.get` using exact camelCase params.
3. Server rejects unknown or snake_case fields on every new request with `-32602`.
4. Planning returns an immutable plan identifier and inspectable decision without applying graph mutation.
5. Applying by plan identifier/idempotency key returns an idempotent structured receipt and does not accept raw caller-forged mutation actions.
6. Adaptation lookup returns the persisted plan/application state or `null` when absent, according to the engine contract.
7. Contradiction list returns held contradictions; get returns one record or `null`.
8. Engine/domain errors retain structured application error data without leaking internal serialization details.

## Implementation sequence

- [x] Add SDK request/result types and exact transport-call tests.
- [x] Run SDK RED test and document expected failure.
- [x] Implement SDK client/export surface and pass focused gates.
- [x] Bind Rust request DTOs and dispatch to settled engine APIs.
- [x] Add Rust happy-path, unknown-field, idempotency, and structured-error tests first.
- [x] Implement minimal server adapters without bypassing engine authority.
- [x] Run focused Rust and TypeScript format/test/typecheck/build/depcheck gates.
- [x] Record any unavoidable engine interface blockers and hand off without commit.

## Security decisions

- JSON-RPC never accepts `AuthorizedCorrection`; authorization remains opaque and engine-owned.
- Application operates on a persisted plan ID plus an idempotency key, not a caller-supplied decision/action.
- Unknown request fields are rejected to avoid silent authorization drift.
- Broad application uses a separately named, admin-gated maintenance method;
  capability issue/consumption is entirely server-internal.
- Contradiction record/refine and uncertainty delegate to engine-owned evidence
  verification and storage.

## Remediation scenarios

1. Disabled, missing, and wrong admin authorization reject raw graph mutation;
   correct authorization succeeds after transport metadata is stripped.
2. Caller-supplied feedback tier/source fields are rejected; accepted raw
   observations receive Deferred/server-owned provenance.
3. Online broad application fails, while authorized maintenance application
   internally consumes the opaque offline capability.
4. Contradiction record/refine/uncertainty use exact camelCase DTOs and verified
   episode evidence.
5. Explicit credit analysis keys retry exactly, conflict on changed input, and
   remain readable across a file-backed server reopen.
6. SDK and CLI inject admin authorization only where privileged and never expose
   an offline capability.
