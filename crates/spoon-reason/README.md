# spoon-reason

Bounded interpretation and working-context assembly for SPOON.

## Owns

- Weighted interpretation candidates with ambiguity preserved.
- Context assembly from graph relationships, procedures, and recent episodes.
- Collection, text, value-depth, and graph-hop limits for predictable reasoning.

## How it works with the system

`spoon-engine` supplies a situation, goal, environment, and remaining budget.
This crate asks `spoon-graph` and `spoon-episode` for relevant material and
returns a bounded, serializable context. It ranks and assembles information;
it does not execute procedures or mutate beliefs.
