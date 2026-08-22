# `@ekg/sdk`

`@ekg/sdk` is the typed TypeScript client for EKG’s newline-delimited
JSON-RPC server. It covers graph and procedure operations, cycles, episodes,
teacher handoff, capabilities, goals, curiosity, consolidation, and metrics.

## Example

```ts
import { EkgClient, StdioTransport } from "@ekg/sdk";

const transport = StdioTransport.spawn("target/debug/ekg-server");
const client = new EkgClient(transport, {
  adminToken: process.env.EKG_ADMIN_TOKEN,
});

try {
  const concepts = await client.listConcepts();
  console.log(concepts);
} finally {
  client.close();
}
```

Use `StreamTransport` when the server process is managed by the host. The SDK
does not silently grant authority: privileged mutations carry the configured
admin token, while imported capabilities remain provisional until local
revalidation and grants.

## Development

```bash
pnpm --filter @ekg/sdk test
pnpm --filter @ekg/sdk typecheck
pnpm --filter @ekg/sdk build
```

The package is currently workspace-private and is consumed directly from its
TypeScript source during local development.
