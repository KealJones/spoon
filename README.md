# Spoon

Spoon is a local, inspectable executable knowledge engine. It records episodes,
evaluates results, learns reusable procedures, and keeps teacher advice
provisional until local checks establish trust.

The repository is transitioning from the historical EKG name. Internal Rust
crate names, npm package names, and `EKG_*` environment variables remain stable
for compatibility with existing scripts.

## Quick start

Requirements: Rust/Cargo, Node.js 24+, and pnpm.

```bash
pnpm install
cargo build -p ekg-server
EKG_DB=/tmp/ekg-playground.sqlite \
  pnpm exec tsx packages/cli/src/main.ts ask --explain "what is double 7?"
```

For a clean answer only:

```bash
EKG_DB=/tmp/ekg-playground.sqlite \
  pnpm exec tsx packages/cli/src/main.ts ask --quiet "what is double 7?"
```

The first run may use the configured teacher. A later run can reuse a trusted
local procedure. `--explain` reports whether a teacher was used, what it
proposed, validation, prediction versus observation, learning/reuse, and cost.

## Packages

- [`@ekg/cli`](packages/cli/README.md) — Spoon's human-facing commands and chat flow.
- [`@ekg/sdk`](packages/sdk/README.md) — Spoon's TypeScript JSON-RPC client.
- [`@ekg/teacher`](packages/teacher/README.md) — Claude, Codex, OpenAI, Ollama,
  and human teacher adapters.
- [`@ekg/inspector`](packages/inspector/README.md) — Spoon's local read-only dashboard
  and “What happened?” episode narratives.
- `crates/` — Rust core, graph, execution, episodes, adaptation, capabilities,
  engine, and JSON-RPC server.

## Dashboard

```bash
cargo build -p ekg-server
EKG_DB=/tmp/ekg-playground.sqlite \
  pnpm --filter @ekg/inspector dev
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
