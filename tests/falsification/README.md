# Section 38 falsification harness

These bounded fixtures drive the durable telemetry tests in
`crates/ekg-engine/tests/falsification_telemetry.rs`. They are intentionally a
representative schema/harness corpus, not a benchmark result or evidence that
Spoon has passed the twelve Section 38 metrics. The cases include successful
acquisition, held-out transfer from distinct families, teacher ablation,
failure retention, and abstention/clarification so each boundary is visible in
review.

Each recorded probe must carry its cohort, novelty identity, teacher usage,
cost/trace data, grounding tier, outcome, and optional skill/attribution data.
Teacher-off probes with teacher calls are rejected. Exact repeats require
`repeatOf` and are excluded from acquisition and transfer measurements.
