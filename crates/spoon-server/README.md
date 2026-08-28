# spoon-server

JSON-RPC over stdio, plus an HTTP chat UI.

## HTTP chat

```bash
SPOON_DB=./spoon.db \
SPOON_TEACHER_MODEL=qwen3.8:27b \
  pnpm serve
```

That runs `cargo run -p spoon-server -- --http --port 4318`. Open
<http://127.0.0.1:4318>. `SPOON_TEACHER_URL` / `SPOON_OLLAMA_URL` default to
`http://localhost:11434`.

## Stdio JSON-RPC

Default (no `--http`): newline-delimited JSON-RPC on stdin/stdout. The CLI and
SDK spawn this binary. Domain rules stay in the engine crates.

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

For network capabilities, set `SPOON_WEB_FETCH_HOSTS` to a comma-separated
allowlist such as `api.example.com`. This installs the bounded `web.fetch`
adapter with a 1 MiB request/response ceiling, 16 steps, and a 10 second
timeout. URLs must be HTTP(S), contain no embedded credentials, and match an
allowlisted host exactly; redirects and credential-bearing headers are denied.
The adapter remains unavailable when the variable is absent. File and network
adapters can be enabled together.

After configuring a network host, an administrator can register its concrete
capability procedure with `capability.provisionWebFetch`. This makes the
host-backed `web.fetch` procedure visible to teaching without granting the
network permission; execution still requires the normal local permission
decision. `capability.list` reports all imported procedures and every native
boundary together with its adapter availability.

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
