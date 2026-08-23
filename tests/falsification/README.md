# Section 38 falsification harness

These tiny fixtures drive the durable telemetry test in
`crates/ekg-engine/tests/falsification_telemetry.rs`. They are intentionally a
schema/harness demonstration, not a benchmark result or evidence that EKG has
passed the twelve Section 38 metrics.

Each recorded probe must carry its cohort, novelty identity, teacher usage,
cost/trace data, grounding tier, outcome, and optional skill/attribution data.
Teacher-off probes with teacher calls are rejected. Exact repeats require
`repeatOf` and are excluded from acquisition and transfer measurements.
