# Spoon developmental benchmarks

The source of truth for the benchmark ideas is [`../ekg-benchmark-suite`](../ekg-benchmark-suite), not a trivia list. Its probes ask whether Spoon acquired a durable structure that changes later behavior.

The machine-readable inventory is [`catalog.json`](catalog.json). It groups the starter experiments into suites; each experiment lives under [`fixtures/`](fixtures) and is deliberately expected to be red until the underlying capability exists.

Each fixture follows the suite's acquisition/retention/generalization discipline:

1. `acquisition` may use Teacher to establish a capability or relevant experience.
2. `teach-*` phases are optional additional Teacher-ON turns when an experiment must establish more than one independent capability before recall is tested.
3. `retention` repeats or probes the capability with Teacher OFF.
4. Remaining phases are held-out or novel variants and only count as generalization evidence.

Reports preserve failures and record the public answer, disposition, episode action, Teacher usage, rung, trace/cost summary, confidence, grounding, and telemetry. A correct exact repeat is regression evidence—not proof of learning. Criteria that require semantic or human judgment remain visible in the report instead of being silently converted into a guessed score.

After Spoon completes a fixture, the separate **Judge** grades the fixture's
immutable, redacted step evidence in one batch. Each response is still an
independent per-step verdict, but batching gives the Judge the fixture context
and avoids one provider call per phase. Judge is a post-run protocol: it cannot
write to Spoon, create episodes, teach a capability, or affect a Teacher-OFF
result. It reuses the configured provider backend (Claude CLI, Codex CLI,
OpenAI, Ollama, or a human adapter) with a Judge-specific prompt and strict
verdict schema—not the Teacher lesson-authoring protocol. Set
`SPOON_JUDGE` and optionally `SPOON_JUDGE_MODEL` to choose it independently;
set `SPOON_JUDGE_ENABLED=false` only for an explicitly unjudged diagnostic run.

Run one experiment through the public entrypoint:

```bash
cargo build -p spoon-server
SPOON_DB=/tmp/spoon-benchmark.sqlite \
  pnpm spoon benchmark run benchmarks/fixtures/FOUNDATION-002.json \
  /tmp/spoon-benchmark-report.json
pnpm spoon benchmark report /tmp/spoon-benchmark-report.json
```

Run the complete catalog when you want the developmental chart and one
aggregate report:

```bash
SPOON_DB=/tmp/spoon-catalog.sqlite \
  pnpm spoon benchmark run benchmarks/catalog.json \
  /tmp/spoon-catalog-report.json
```

Catalog execution resolves suite fixture IDs, runs each fixture through the
same public path, aggregates the results, and preserves one telemetry run ID
per fixture. Each catalog fixture receives its own fresh temporary database;
its acquisition, optional `teach-*` turns, retention, and held-out phases
share that one store. This makes catalog results controlled acquisition tests.
Use a dedicated fixture such as `INTERF-001` to test interference deliberately,
with the intended competing procedures established inside the same fixture.

For the manual Bar Test, use [`../ekg-benchmark-suite/10-bar-test.md`](../ekg-benchmark-suite/10-bar-test.md) and attach the human ratings to the saved report. It is not reducible to an answer key.
