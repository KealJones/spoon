# Spoon

> Current implementation reality: see [`STATUS.md`](STATUS.md). The
> implementation plan describes intended scope; `STATUS.md` distinguishes
> fully integrated behavior from partial, scaffolded, missing, and in-flight work.

Spoon is a local, inspectable executable knowledge engine. It records episodes,
evaluates results, learns reusable procedures, and keeps teacher advice
provisional until local checks establish trust.

## Quick start

Requirements: Rust/Cargo, Node.js 24+, and pnpm.

```bash
pnpm install
cargo build -p spoon-server
SPOON_DB=/tmp/spoon-playground.sqlite \
  pnpm spoon ask --explain "what is double 7?"
```

For a clean answer only:

```bash
SPOON_DB=/tmp/spoon-playground.sqlite \
  pnpm spoon ask --quiet "what is double 7?"
```

The first run may use the configured teacher. A later run can reuse a trusted
local procedure. `--explain` reports whether a teacher was used, what it
proposed, validation, prediction versus observation, learning/reuse, and cost.

Configuration is inherited from `~/.spoon/config.json` through project
`.spoon/config.json` to an optional local `.spoon/config.local.json`:

```bash
pnpm spoon config show --sources
pnpm spoon ask --quiet "turn off teacher"
pnpm spoon ask --quiet "use full access"
```

Global episodic recall is the default. Use `spoon chat` for a durable named
conversation, or `--isolated` when a session must stay out of global recall.

To test whether a Teacher answer became durable knowledge, use the benchmark
runner. It performs Teacher-ON acquisition, exact Teacher-OFF retention, then
only runs paraphrase/novel-value Teacher-OFF variants when retention passes:

```bash
SPOON_DB=/tmp/spoon-benchmark.sqlite \
  pnpm spoon benchmark run benchmarks/teacher-retention-starter.json \
  /tmp/spoon-benchmark-report.json
pnpm spoon benchmark report /tmp/spoon-benchmark-report.json
```

## Packages

- [`@spoon/cli`](packages/cli/README.md) — Spoon's human-facing commands and chat flow.
- [`@spoon/sdk`](packages/sdk/README.md) — Spoon's TypeScript JSON-RPC client.
- [`@spoon/teacher`](packages/teacher/README.md) — Claude, Codex, OpenAI, Ollama,
  and human teacher adapters.
- [`@spoon/inspector`](packages/inspector/README.md) — Spoon's local read-only dashboard
  and “What happened?” episode narratives.
- `crates/` — Rust core, graph, execution, episodes, adaptation, capabilities,
  engine, and JSON-RPC server.

## Dashboard

```bash
cargo build -p spoon-server
SPOON_DB=/tmp/spoon-playground.sqlite \
  pnpm --filter @spoon/inspector dev
```

Open <http://127.0.0.1:4317>. The dashboard is read-only. Select an episode to
see the redacted narrative; raw JSON remains available as a forensic drill-down.

## Development checks

```bash
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
pnpm test
pnpm typecheck
pnpm build
pnpm depcheck
```

Spoon stores data in SQLite. Imported capabilities are quarantined, do not carry
secrets or trust, and require local permission grants and trusted revalidation.
