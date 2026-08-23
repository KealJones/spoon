# spoon-graph

SQLite-backed persistence and bounded traversal for SPOON's knowledge graph.

## Owns

- CRUD and immutable version history for concepts, relationships, and procedures.
- Lifecycle changes, dependency reports, relationship evidence, and activation spread.
- Schema creation and graph-specific storage errors.

## How it works with the system

`spoon-engine` uses `KnowledgeStore` for current knowledge and version-pinned
replay. `spoon-reason` asks it for bounded context, while `spoon-adapt` applies
CAS-checked corrections and reconciliation. Reads are intentionally bounded;
graph mutation is explicit and records history rather than silently overwriting it.
