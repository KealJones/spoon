# Phase 2 Server and SDK Progress

- [x] Created task documentation and logs directory.
- [x] Ran required instruction-file discovery.
- [x] Read Phase 2 plan, handoff, server, SDK, engine, credit, and adapt surfaces.
- [x] Identified missing engine adaptation/contradiction API as the active integration seam.
- [x] SDK RED tests written and observed failing.
- [x] SDK implementation green.
- [x] Rust RED tests written before server dispatch implementation.
- [x] Rust implementation green.
- [x] Strict package gates green.
- [x] Final API/blocker report prepared; no commit.

## TDD log

- Exploration: current `Engine` exposes graph/episode reads, execution, replay, failure analysis, and cycle APIs. It does not yet expose persisted adaptation plans/applications or contradictions. Direct server access would bypass the intended engine trust boundary, so mutation RPC implementation is waiting on the engine-owned surface while SDK transport work proceeds independently.
- SDK RED: the two new transport tests failed with missing `planAdaptation` and `listContradictions` methods.
- Rust RED attempt: the new endpoint tests were present, but the focused run was temporarily blocked because the in-flight engine declared `mod adaptation` before creating the module file. Once the engine API landed, the server adapter was implemented against the settled signatures.
- GREEN: `adaptation.plan`, `adaptation.get`, and online-only `adaptation.apply` now delegate exclusively to Engine. RPC apply has no offline-capability field and rejects one as invalid params. `contradiction.list` delegates to `Engine::list_held_contradictions`; `contradiction.get` delegates to `Engine::get_contradiction`.
- Wire hardening: new request DTOs deny unknown fields, engine adaptation DTOs enforce nested boundaries, IDs are validated, contradiction output uses explicit camelCase wrappers, and adaptation failures use code `-32020` with `{cause}` data.
- Tests cover plan/apply idempotency, immutable lookup state, exact camelCase fields, online-only apply, recursively rejected unknown adaptation fields, structured errors, missing contradiction lookup, and populated contradiction list/get output.

## Validation

- `cargo fmt -p ekg-server -- --check`: passed.
- `cargo test -p ekg-server`: passed, 19 integration tests plus doc tests.
- `cargo clippy -p ekg-server --all-targets --all-features -- -D warnings`: passed.
- `cargo check -p ekg-server`: passed.
- focused Prettier formatting for changed SDK files: passed.
- `pnpm --filter @ekg/sdk test`: passed, 13 tests including Rust stdio integration.
- `pnpm --filter @ekg/sdk typecheck`: passed.
- `pnpm --filter @ekg/sdk build`: passed.
- `pnpm --filter @ekg/sdk depcheck`: passed with no issues.
- `pnpm --filter @ekg/cli test`: passed, 14 tests.
- `pnpm --filter @ekg/cli typecheck`: passed.
- `pnpm --filter @ekg/cli build`: passed.
- `pnpm --filter @ekg/cli depcheck`: passed with no issues.
- `git diff --check` for owned paths: passed.

## Public surface

- RPC adds `adaptation.applyOffline`, `contradiction.record`,
  `contradiction.refine`, `contradiction.uncertainty`, `credit.get`, and
  `credit.getByKey`; `credit.analyze` accepts an optional transport-level
  `idempotencyKey`.
- SDK adds `applyAdaptationOffline`, `recordContradiction`,
  `refineContradiction`, `getClaimUncertainty`, `getFailureAnalysis`, and
  `getFailureAnalysisByKey`; `EkgClientOptions.adminToken` is injected only
  into privileged methods.
- CLI reads `EKG_ADMIN_TOKEN` and adds offline adaptation plus contradiction
  record/refine/uncertainty commands.
- SDK exports typed adaptation request/plan/action/evidence/reconciliation/receipt and contradiction claim/refinement records.
- `RpcServer::from_engine` supports safe composition/testing without exposing or duplicating stores.

## Remaining notes

- No blocking engine mismatch remains. The settled online apply request is exactly `{planId,idempotencyKey,appliedAt}`.
- Broad/offline capability remains opaque, non-Serde, and local even though the
  controlled maintenance operation is exposed.
- Raw feedback can no longer assert Hard/Consensus trust or independent source
  identity.
- Durable credit reads are available by content-addressed ID and explicit retry
  key, including after server reopen.
- No commit was created.
