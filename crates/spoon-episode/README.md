# spoon-episode

Append-oriented storage for SPOON's complete cognitive episodes.

## Owns

- SQLite persistence and querying for episodes, feedback, observed facts, and traces.
- Finalization rules, immutable completed episodes, and idempotent feedback.
- Materialized credit aggregates, regression evidence, and episode indexes.

## How it works with the system

`spoon-engine` records every attempt here before learning or adaptation uses it.
`spoon-credit` consumes the indexed history for attribution, while
`spoon-reason` uses bounded recent context. Failed episodes remain inspectable;
late feedback is appended rather than rewriting the original event.
