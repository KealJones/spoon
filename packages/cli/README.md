# `@ekg/cli`

The CLI is the simplest way to talk to a local EKG server.

## Run it from the workspace

Build the Rust server once, then use `tsx` during development:

```bash
cargo build -p ekg-server
EKG_DB=/tmp/ekg-playground.sqlite \
  pnpm exec tsx packages/cli/src/main.ts ask --quiet "what is double 7?"
```

Commands start the server binary from `target/debug/ekg-server` unless
`EKG_SERVER` overrides it. `EKG_DB` selects the SQLite database.

## Chat output modes

```bash
# Full JSON episode/result
pnpm exec tsx packages/cli/src/main.ts ask "what is double 7?"

# Answer only
pnpm exec tsx packages/cli/src/main.ts ask --quiet "what is double 7?"

# Human-readable audit trail
pnpm exec tsx packages/cli/src/main.ts ask --explain "what is double 7?"
```

The explain view includes teacher use and provider/model, proposal and
validation, context assumptions, prediction/observation, evaluation, learned
or reused action, and cost. It does not turn provisional teacher text into
trusted knowledge.

Other useful commands include `concept list`, `procedure list`, `episode list`,
and `primitive observe clock`.

## Teacher selection

The default is Claude via its local CLI. Set `EKG_TEACHER` to `claude`,
`codex`, `openai`, `ollama`, or `human`; set `EKG_TEACHER_MODEL` when required.
The OpenAI adapter reads `OPENAI_API_KEY` from the process environment. Keep
credentials outside the database and do not commit `.env` files.

Admin-only mutations use `EKG_ADMIN_TOKEN`.

## Package checks

```bash
pnpm --filter @ekg/cli test
pnpm --filter @ekg/cli typecheck
```
