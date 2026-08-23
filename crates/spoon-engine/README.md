# spoon-engine

The SPOON orchestrator and public cognition-cycle service.

## Owns

- The cycle from interpretation and context through execution, evaluation,
  credit assignment, teaching, adaptation, and durable continuation.
- Cross-cutting trust receipts, runtime leases, goals, skills, compression,
  regression suites, telemetry, and capability coordination.
- Admission of Teacher lessons: a `pure_expr_v2` lesson can compose only
  request-advertised aliases for already active/validated, closed pure
  procedures. The engine—not the Teacher—pins their exact stored revisions;
  later revisions cannot silently alter the learned composition.
- The public Rust API and read-only graph/episode views used by integrations.

## How it works with the system

This is the integration boundary: it composes `spoon-reason`, `spoon-graph`,
`spoon-exec`, `spoon-episode`, `spoon-credit`, `spoon-adapt`, `spoon-intuition`,
and `spoon-capability`. It is responsible for evidence and authority boundaries;
lower-level stores do not get to promote caller-provided data on their own.
