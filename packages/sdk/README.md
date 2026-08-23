# Spoon SDK (`@spoon/sdk`)

`@spoon/sdk` is Spoon’s typed TypeScript client for its newline-delimited
JSON-RPC server. It covers graph and procedure operations, cycles, episodes,
teacher handoff, capabilities, goals, curiosity, consolidation, and metrics.

## Example

```ts
import { SpoonClient, StdioTransport } from "@spoon/sdk";

const transport = StdioTransport.spawn("target/debug/spoon-server");
const client = new SpoonClient(transport, {
  adminToken: process.env.SPOON_ADMIN_TOKEN,
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
pnpm --filter @spoon/sdk test
pnpm --filter @spoon/sdk typecheck
pnpm --filter @spoon/sdk build
```

The package is currently workspace-private and is consumed directly from its
TypeScript source during local development.
