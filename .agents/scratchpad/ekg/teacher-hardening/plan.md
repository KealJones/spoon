# Teacher Hardening Plan

## Test Strategy

- A matching unverified proposal proceeds to schema and independent validation.
- Runtime status other than `unverified` is rejected before custom validators run.
- Empty/mismatched source, provider/source mismatch, situation mismatch, question mismatch, malformed provenance timestamp, and empty request id are rejected.
- Object enum/const and `uniqueItems` use semantic JSON equality regardless of property order.
- Inherited properties do not satisfy `required` and inherited enumerable values are not accepted as JSON objects.
- A teacher-created pipeline updates the same reliability state returned by that teacher.
- Command, fetch, and human prompt rejections surface as provider-attributed `TeacherError` values.

## Implementation

- Add envelope/provenance validation to the validation pipeline.
- Replace stringification equality with recursive semantic equality and own-property checks.
- Expose a connected validation-pipeline factory from each teacher.
- Add a shared provider-boundary helper and apply it at external I/O edges.
- Run focused tests, then package test, typecheck, formatting, build, and dependency checks.

