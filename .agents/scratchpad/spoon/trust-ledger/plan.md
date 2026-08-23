# Trust ledger plan

- [x] Add a durable receipt table keyed by evidence kind and immutable digest.
- [x] Mint receipts after Engine deterministic execution persists an episode.
- [x] Require an exact receipt before strong episode/feedback evidence affects
  the adaptation gate.
- [x] Add an admin-gated authenticated-verifier feedback operation for
  independently verified strong observations.
- [x] Add tests for raw strong-row rejection and durable genuine-execution
  authorization.
- [x] Reconcile legitimate broad-adaptation fixtures with the authenticated
  verifier operation; raw feedback remains untrusted.
