# Trust ledger progress

## Completed

- Added `engine_trust_receipts`, a durable ledger of Engine-issued receipts
  bound to evidence kind, ID, SHA-256 digest, tier, issuer, and timestamp.
- Engine deterministic execution now mints a receipt only after the episode is
  persisted.
- Adaptation evidence derivation rejects strong episode or feedback evidence
  without an exact Engine receipt; raw evidence is not allowed to become
  authority merely by selecting `Hard` or `Consensus`.
- Added an admin-gated authenticated-verifier feedback operation. It persists
  the feedback first, then mints a receipt bound to the exact stored payload,
  verifier identity, and tier. `admin_append_feedback` remains untrusted.
- Added focused integration coverage for a forged cloned Hard episode and for
  receipt persistence across reopen.

## Verification

- `cargo test -p ekg-engine --test trust_ledger` — 3 passed.
- `cargo fmt --check` — passed.
- `cargo clippy -p ekg-engine --test trust_ledger -- -D warnings` — passed.

## Pending coordination

Root is concurrently migrating the adaptation suite to read-only Engine
facades/admin operations. The verifier fixture updates are complete; root owns
the remaining facade migration and its mutability cleanup.
