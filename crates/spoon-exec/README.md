# spoon-exec

The pure expression and procedure evaluator for SPOON.

## Owns

- Scoped variable environments and execution budgets.
- Evaluation of the neutral `spoon-core::Expr` tree and procedure calls.
- Contract checks and lossless execution traces for replay and diagnosis.

## How it works with the system

`spoon-engine` registers the executable graph and creates a bounded evaluator
per run. `spoon-episode` stores the resulting trace, and `spoon-credit` later
replays or inspects it. This crate does not persist knowledge or perform
ambient side effects.
