# Tiered Evaluation Context

## Requirements

- Produce `ekg_core::Evaluation` values for deterministic checks, independent-method consensus, and inverse round trips.
- Represent Tier 3 outcomes without treating a pending judgment as a failed judgment.
- Calculate a bounded surprise signal from predicted and observed values.
- Capture caller-proposed, checkable subgoals for weak goals; do not claim automatic semantic decomposition.
- Keep all implementation and artifacts inside `crates/ekg-engine`.

## Existing Patterns

`ekg-core` owns `Value`, `Evaluation`, and `VerifiabilityTier`. `Evaluation` carries a tier, a success verdict, details, and an optional surprise score. The engine crate is currently a stub and already depends on `ekg-core`.

## Dependency Map

Callers -> `ekg_engine::evaluation` -> `ekg_core::{Value, Evaluation, VerifiabilityTier}`.

No `CODEASSIST.md` or repository instruction file was found. The root implementation plan is the task-specific source of requirements.

