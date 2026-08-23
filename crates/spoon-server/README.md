# spoon-server

The newline-delimited JSON-RPC boundary for running SPOON as a local service.

## Owns

- JSON-RPC request decoding, dispatch, response/error envelopes, and stdio transport.
- CamelCase wire types and mapping between external requests and engine APIs.
- Admin authorization at privileged mutation methods.

## How it works with the system

The server owns an `spoon-engine::Engine` and exposes graph, episode, cycle,
credit, adaptation, contradiction, capability, goal, and telemetry operations.
The TypeScript CLI/SDK communicate over stdio; domain rules remain in the Rust
crates so alternate clients cannot bypass the engine's trust and persistence gates.

## Capability host adapters

`capability.invoke` is effectful only when the embedding host installs a real
adapter registry. For scoped files, the host maps a portable binding to a local
directory and fixes the resource ceiling:

```rust
use spoon_capability::ResourceBounds;
use spoon_server::{CapabilityHostAdapters, RpcServer};

let adapters = CapabilityHostAdapters::with_scoped_files(
    "workspace",
    "/srv/spoon/workspace",
    ResourceBounds { max_bytes: 1_048_576, max_steps: 16, max_millis: 2_000 },
)?;
let server = RpcServer::open("spoon.db")?.with_capability_host_adapters(adapters);
```

The RPC request contains only `contentId`, `procedureId`, and typed `input`.
The server re-resolves the stored procedure and its durable grant on every
call. Clients cannot provide grants, permission modes, adapter selection,
filesystem roots, or bounds. Unconfigured primitives return
`capability_adapter_unavailable`; there is no ambient fallback.

The supplied stdio binary installs this adapter only when its host sets
`SPOON_FILE_ROOT`; `SPOON_FILE_BINDING` selects the portable binding name and
defaults to `workspace`. Its public ceiling is fixed at 1 MiB, 16 steps, and
2 seconds. Missing configuration leaves file effects unavailable.

## Bounded response-plan rendering

`language.render` is a read-only deterministic surface renderer, not a general
language-model endpoint. It accepts a typed `ResponsePlan` plus optional
content-free `tone` and `variant` overrides. It preserves only submitted claim
text that has at least one evidence reference, omits explicitly unsupported
claims, and rejects claims without evidence references. Its result exposes the
included/omitted IDs and a redacted audit record.

The endpoint does not resolve client-provided evidence/provenance IDs through
the Engine, so its audit deliberately marks them `caller_supplied_unverified`.
It does not return provenance or evidence fields, grant authority, or generate
new factual wording. The public request envelope is capped at 128 KiB; the core
plan and per-claim limits remain enforced as well.
