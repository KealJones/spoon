# Phase 4 progress

- Added a pure `PromotionGate` that evaluates trusted replay rows.
- Correctness is non-negotiable: any challenger regression rejects shadow
  eligibility, while at least one measured compression/search/coverage/transfer
  win is required.
- Existing procedure replacement evidence now delegates to this gate without
  changing admin, trust, or offline-capability boundaries.

Remaining: self-growing regression-test persistence, skill discovery from
repetition/success/failure, shadow execution, and measured compounding/transfer
evidence.
