# Progress

## Audit

The selected gap is goal-bound learning provenance. A generic `parent_id` did not distinguish
external goals from derived goals and did not record the curiosity gap that authorized learning.

## TDD cycles

- RED: `cargo test -p ekg-engine --test goal_boundaries` failed because the protected
  derivation and provenance APIs did not exist.
- GREEN in progress: implement a transactional goal/provenance insert and explicit external-goal
  boundary.
- GREEN: three boundary tests pass, including invalid derivation rollback and file-backed reopen.
- REFACTOR: changed curiosity-gap replacement into an in-place upsert so foreign-key-backed
  provenance remains valid when a gap is updated.

## Validation

- `cargo test -p ekg-engine`: pass.
- `cargo clippy -p ekg-engine --tests -- -D warnings`: pass.
- `cargo build -p ekg-engine`: pass.

## Commit

- Focused commit prepared with only goal-boundary source, tests, and track documentation;
  concurrent representation-trainer and RPC changes remain unstaged.
