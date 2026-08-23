# Reason Hardening Plan

## Test Strategy

- Reject serialized interpretation tolerance above the hard maximum.
- Reject more interpretation candidates than the hard maximum.
- Reject context limits above absolute collection/text/hop ceilings.
- Persist goal reason, interpretations, graph knowledge, relevant procedures,
  recent episodes, assumptions, environment, and remaining budget.
- Prefer entity-linked recent episodes over newer unrelated history.
- Exclude retired relationships, adjacent concepts, and procedures.

## Implementation Tasks

- [x] Add all hardening tests and observe RED.
- [x] Cap interpretation tolerance and candidate count.
- [x] Add absolute context limits and bounded nested values.
- [x] Add typed persisted context categories to `spoon-core`.
- [x] Select relevant procedure metadata and concept-linked history.
- [x] Apply lifecycle filtering and sanitize embedded graph metadata.
- [x] Format, test, and run strict focused linting.

## Risks

- Core episode schema expansion must remain backward-compatible through serde
  defaults.
- Parallel engine edits consume current reason APIs; existing re-exports and
  field names remain stable where possible.
