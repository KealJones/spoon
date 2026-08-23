# Trust ledger context

## Scope

Phase 2 requires adaptation authority to come from Engine-owned verification,
not from caller-controlled SQLite rows. This slice adds durable, content-bound
receipts for deterministic Engine execution episodes and checks them while
deriving adaptation evidence.

## Integration points

- `spoon-engine/src/engine.rs` persists deterministic execution episodes.
- `spoon-engine/src/adaptation.rs` derives the evidence gate that controls every
  adaptation decision.
- `spoon-episode` remains an append-only persistence component. Writing it
  directly is intentionally outside the Engine trust boundary.

## Security rule

A `Hard` or `Consensus` enum is descriptive data, not authority. A qualifying
receipt must match the exact serialized episode or feedback digest and tier.
Engine execution and the admin-gated authenticated verifier operation are the
only issuers in this slice. Raw/admin insertion and raw feedback cannot mint
receipts.
