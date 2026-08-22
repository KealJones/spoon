# Phase 5 capability progress

## Implemented foundation

- `ekg-capability` models native primitives, typed schemas, contracts,
  effects, permissions, bounds, dependency pins, tests, and provenance.
- Interface discovery synthesizes neutral network procedures from an explicit
  description and fixture; sandbox validation never performs ambient I/O.
- Bundles are canonical JSON with SHA-256 content identity and deterministic
  export/import. Secret-bearing keys are rejected.
- Imports are stored in SQLite quarantine, remain Quarantined until local
  sandbox revalidation, then become Provisional. Grants are separate local
  rows, admin-gated through Engine, revocable, and never exported.
- Engine exposes discovery, bundle import/export, local revalidation, and
  grant/revoke/permission checks.
- Phase 4 now has a pure promotion gate requiring no correctness regression and
  at least one measured win before shadow eligibility.

## Remaining before Phase 5 exit

- Add policy-enforced file/observation/sandbox invocation adapters and bounded
  effect receipts.
- Persist validation provenance/environment digests separately from bundle
  identity and add dependency DAG closure/acyclicity checks.
- Add JSON-RPC/SDK/CLI surfaces and a full local revalidation-to-promotion
  path; promotion must remain governed by the Phase 4 gate.
- Expand adversarial tests for path/redirect escape, schema/dependency bombs,
  malformed bundles, and atomic rollback.
