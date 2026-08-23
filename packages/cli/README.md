# Spoon CLI (`@spoon/cli`)

The CLI is the simplest way to talk to a local Spoon server.

## Run it from the workspace

Build the Rust server once, then use `tsx` during development:

```bash
cargo build -p spoon-server
SPOON_DB=/tmp/spoon-playground.sqlite \
  pnpm spoon ask --quiet "what is double 7?"
```

Commands start the server binary from `target/debug/spoon-server` unless
`SPOON_SERVER` overrides it. `SPOON_DB` selects the SQLite database.

## Chat output modes

```bash
# Full JSON episode/result
pnpm spoon ask "what is double 7?"

# Answer only
pnpm spoon ask --quiet "what is double 7?"

# Human-readable audit trail
pnpm spoon ask --explain "what is double 7?"

# Explicitly disable the Teacher for a retention check
pnpm spoon ask --teacher off "what is double 7?"
```

## Developmental benchmarks

The benchmark runner uses the public `ask` command for every experiment. The
machine-readable catalog and starter fixtures are derived from
`ekg-benchmark-suite/`. Each fixture runs Teacher ON acquisition, Teacher-OFF
retention, and (where defined) held-out or novel-value variants. Exact repeats
are retention checks; variants are skipped when retention fails.

```bash
cargo build -p spoon-server
SPOON_DB=/tmp/spoon-benchmark.sqlite \
  pnpm spoon benchmark run benchmarks/fixtures/FOUNDATION-002.json \
  /tmp/spoon-benchmark-report.json
pnpm spoon benchmark report /tmp/spoon-benchmark-report.json

# Run the complete developmental catalog
SPOON_DB=/tmp/spoon-catalog.sqlite \
  pnpm spoon benchmark run benchmarks/catalog.json \
  /tmp/spoon-catalog-report.json
```

The runner writes both JSON and Markdown reports and records accepted probe
measurements in the existing falsification telemetry store. Each ask is a new
CLI/server process. A standalone fixture uses its configured database; catalog
fixtures each receive a fresh temporary database while preserving state across
their own acquisition, optional `teach-*`, retention, and variant phases.

After a fixture completes, a separate **Judge** grades all of its immutable
step evidence in one batch and returns independent verdicts per step. It uses
the same backend adapters as the Teacher
(including Claude/Codex CLI, OpenAI, Ollama, and human), but a distinct Judge
prompt/schema and no Spoon write path. Configure it with `SPOON_JUDGE` and
`SPOON_JUDGE_MODEL`; omit `SPOON_JUDGE` to reuse the configured provider, or
set `SPOON_JUDGE_ENABLED=false` for an explicitly unjudged diagnostic run.

The explain view includes teacher use and provider/model, proposal and
validation, context assumptions, prediction/observation, evaluation, learned
or reused action, and cost. It does not turn provisional teacher text into
trusted knowledge.

Other useful commands include `concept list`, `procedure list`, `episode list`,
and `primitive observe clock`.

## Workspace configuration and sessions

Spoon layers configuration from `~/.spoon/config.json`, the nearest project
`.spoon/config.json`, and the optional uncommitted `.spoon/config.local.json`.
Inspect the effective values and their precedence with:

```bash
pnpm spoon config show --sources
pnpm spoon config validate
```

The local control plane also accepts common settings requests through `ask`,
without asking a Teacher to authorize them:

```bash
pnpm spoon ask --quiet "turn off teacher"
pnpm spoon ask --quiet "use full access"
pnpm spoon ask --quiet "set recall mode to session"
pnpm spoon ask --quiet "use database /tmp/spoon-test.sqlite"
```

These write only a redacted receipt to `~/.spoon/admin-receipts.jsonl` and keep
project-controlled permission modes from elevating themselves. `ask`,
`workspace`, and `full-access` are also available as explicit
`--permission-mode` values; full access still preserves mandatory denials,
declared effects, bounds, contracts, and provenance.

Episodes are globally recalled by default. Use a named global session for
continuity or an isolated session when its episodes must not enter global
recall:

```bash
pnpm spoon session start --name experiment
pnpm spoon chat --session experiment
pnpm spoon chat --isolated --session private-test
```

## Teacher selection

The default is Claude via its local CLI. Set `SPOON_TEACHER` to `claude`,
`codex`, `openai`, `ollama`, or `human`; set `SPOON_TEACHER_MODEL` when required.
The OpenAI adapter reads `OPENAI_API_KEY` from the process environment. Keep
credentials outside the database and do not commit `.env` files.

Admin-only mutations use `SPOON_ADMIN_TOKEN`.

## Package checks

```bash
pnpm --filter @spoon/cli test
pnpm --filter @spoon/cli typecheck
```
