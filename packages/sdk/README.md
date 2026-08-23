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

Once the server host has configured a concrete adapter and the local operator
has revalidated and granted the exact stored procedure, invoke it with:

```ts
const result = await client.invokeCapability(
  contentId,
  procedureId,
  "replacement contents",
);
console.log(result.output, result.episodeId);
```

The SDK exposes no adapter, root, permission-policy, or resource-policy
arguments. The result contains the immediate typed output and a public receipt;
raw host targets and permission scopes are omitted.

## Bounded response-plan rendering

The SDK can also render a typed, already-authored response plan without calling
a language model:

```ts
const rendered = await client.renderResponsePlan({
  dialogueMove: { act: "Inform", relatesToTurn: null },
  claims: [{
    Grounded: {
      id: "answer",
      text: "There are 3 r characters in strawberry.",
      evidence: [{
        id: "episode:letter-count",
        sourceKind: "SelfVerified",
        linkedEpisode: null,
      }],
      provenance: ["procedure:letter-count-v1"],
    },
  }],
  uncertainty: { level: "Certain", disclosure: null },
  tone: "Neutral",
  variant: "Plain",
}, { variant: "Bulleted" });

console.log(rendered.text);
```

This only formats supplied claim text; it cannot infer a fact or write a new
sentence. The server requires an evidence reference but does not independently
verify references supplied through this endpoint, and reports that explicitly
as `caller_supplied_unverified`. Raw provenance is redacted from the result.

## Development

```bash
pnpm --filter @spoon/sdk test
pnpm --filter @spoon/sdk typecheck
pnpm --filter @spoon/sdk build
```

The package is currently workspace-private and is consumed directly from its
TypeScript source during local development.
