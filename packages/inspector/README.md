# Spoon Inspector (`@spoon/inspector`)

The inspector is a local, read-only web dashboard for the Spoon server. It shows
knowledge, procedures, episodes, flywheel telemetry, and the twelve Section 38
metric slots with honest measured/partial/uninstrumented status.

## Start

From the repository root:

```bash
cargo build -p spoon-server
SPOON_DB=/tmp/spoon-playground.sqlite \
  pnpm --filter @spoon/inspector dev
```

Open <http://127.0.0.1:4317>. Stop the process with Ctrl-C.

The episode browser links to `GET /api/episodes/:id`, a redacted narrative
that explains the original request, escalation, teacher/provider/model and
proposal, validation, learning/reuse, prediction/observation/evaluation,
cost, abstention, and capability permissions/effects. The raw JSON drill-down
preserves forensic detail while redacting secrets, bearer tokens, cookies, and
environment values.

## Package checks

```bash
pnpm --filter @spoon/inspector test
pnpm --filter @spoon/inspector typecheck
pnpm --filter @spoon/inspector build
```

The dashboard does not mutate the graph, episodes, capabilities, or goals.
