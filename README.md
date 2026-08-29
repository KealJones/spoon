# Spoon

A local, inspectable executable knowledge engine. It records episodes, runs
procedures, and keeps teacher advice provisional until local checks land.

The teacher authors **Spoonlang** (a small infix surface language). The engine
compiles that to `pure_expr_v2` IR, admits it, and reuses it. Untagged JSON AST
lessons still compile if you send them by hand.

Implementation honesty: [`STATUS.md`](STATUS.md).

## Requirements

Rust/Cargo, Node.js 24+, pnpm. For teaching from chat or `ask`, a local
[Ollama](https://ollama.com) model. `qwen3.8:27b` follows Spoonlang. Tiny models
copy prompt examples and miss the schema.

```bash
pnpm install
ollama pull qwen3.8:27b
```

## Start the HTTP server (chat UI)

```bash
pnpm serve
```

Open <http://127.0.0.1:4318>. Named sessions restore episode history. The pinned
Global chat has no session and does not recall.

`pnpm serve` is `cargo run -p spoon-server -- --http --port 4318`. It loads the
same `~/.spoon/config.json` as the CLI. Override the port with `--port` on the
binary. Environment variables still override file values. `SPOON_TEACHER_URL` /
`SPOON_OLLAMA_URL` default to `http://localhost:11434`. Without Ollama, unknown
questions abstain.

## CLI

The CLI spawns the same server over stdio. Build once, then:

```bash
cargo build -p spoon-server
SPOON_DB=./spoon.db \
SPOON_TEACHER=ollama \
SPOON_TEACHER_MODEL=qwen3.8:27b \
  pnpm spoon ask --explain "what is twenty five percent of eighty?"
```

```bash
pnpm spoon ask --quiet "what is double 7?"
pnpm spoon teach --explain "extract arr[0].name from a supplied object"
pnpm spoon chat
pnpm spoon config show --sources
pnpm spoon capability list
```

`teach` is the explicit authoring boundary. A normal `ask` does not become a
teach just because the teacher returned a lesson. `--teacher off` checks
retention.

Config stacks `~/.spoon/config.json`, project `.spoon/config.json`, then
`.spoon/config.local.json`.

## Spoonlang (teacher wire)

The JSON envelope is `{ "source": "<spoonlang>", "interpretations": [] }`.
Example source:

```
kind reusable_lesson
concept percent: defeasible_general
  "A proportion of a quantity, expressed as parts per hundred"
proc percent_of(percent: number, of: number)
  name "PERCENT OF"
  (percent * of) / 100
example percent_of(50, 100) => 50
```

Stable facts with no inputs to transform use `kind answer_only`. Effectful work
uses `cap("spoon.native", "web.fetch", { url: url })` with advertised ids only.

## Inspector

```bash
cargo build -p spoon-server
SPOON_DB=./spoon.db pnpm inspect
```

Open <http://127.0.0.1:4317>. Read-only episode narratives.

## Packages

- [`@spoon/cli`](packages/cli/README.md) — commands and stdio chat
- [`@spoon/sdk`](packages/sdk/README.md) — TypeScript JSON-RPC client
- [`@spoon/teacher`](packages/teacher/README.md) — Claude, Codex, Cursor, OpenAI, Ollama, human
- [`@spoon/inspector`](packages/inspector/README.md) — dashboard
- `crates/` — core, graph, exec, engine, HTTP/JSON-RPC server

## Checks

```bash
cargo test --workspace --all-targets
pnpm test
pnpm typecheck
```

Spoon stores data in SQLite. Imported capabilities are quarantined and need
local permission grants.
