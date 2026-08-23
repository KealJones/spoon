# spoon-core

The shared data model for SPOON (the Executable Knowledge Graph).

## Owns

- Concepts, relationships, procedures, contracts, expressions, values, and lifecycles.
- Episodes, evaluations, execution traces, assumptions, and evidence metadata.
- Stable IDs and serde-compatible wire/storage representations.
- Bounded language substrate values: UTF-8 token streams with byte spans, intent
  frames/slots, dialogue moves, response plans, and a grounded no-model renderer.

## How it works with the system

Other crates depend on these types rather than defining parallel domain models.
`spoon-graph` persists graph entities, `spoon-exec` evaluates expressions,
`spoon-episode` stores episodes, and `spoon-engine` coordinates the complete
cognitive cycle. This crate contains no database access, model calls, or I/O.

The language values are neutral data structures only. The renderer can format
evidence-backed claim text that was already supplied to it; it does not infer
meaning, invent facts or sources, call a model, or make host effects.
