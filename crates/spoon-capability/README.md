# spoon-capability

Policy-enforced capability acquisition and portable capability bundles.

## Owns

- The small native primitive vocabulary: network, scoped files, observation,
  and sandboxed execution.
- Typed capability procedures, schemas, contracts, effects, permissions, bounds,
  tests, dependencies, provenance, and local validation state.
- Deterministic bundle export/import and invocation authorization.

## How it works with the system

Discovery and synthesis produce candidates; local sandbox tests and permissions
must validate them before promotion. Imported bundles are provisional and
content-addressed. They transfer structure, tests, and provenance—not secrets,
trust receipts, grants, ambient paths, or environment assumptions. Host adapters
receive only an authorized invocation, never arbitrary bundle code or ambient I/O.

File capability manifests use portable logical bindings such as
`workspace/reports/result.json`, not host paths. `ScopedFileAdapter` maps one
explicit binding to one canonical local directory and supplies the stricter
host bounds. It supports regular-file reads and writes only, re-checks the
resolved path beneath that directory, rejects symlink targets/escapes, and
fails honestly for network, observation, or sandbox requests.
