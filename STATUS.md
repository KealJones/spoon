# Spoon Implementation Status

Last audited: 2026-08-23

This is the human-facing source of truth for what Spoon actually does today.
It deliberately does not treat a type, stub, fixture, mock, passing unit test,
or public method as proof that a subsystem is fully implemented.

## Status meanings

- **FULL** — the documented scope is publicly reachable, integrated into the
  real Spoon workflow, failure/adversarial tested, and production-real wherever
  it performs host effects.
- **PARTIAL** — useful real behavior exists, but one or more required public,
  integration, safety, or completeness paths are missing.
- **SCAFFOLD** — declarations, schemas, fixtures, mocks, or isolated machinery
  exist, but the claimed behavior is not usable end to end.
- **MISSING** — no meaningful implementation exists yet.
- **IN FLIGHT** — currently being changed; never infer completion until fresh
  evidence is recorded here.

Evidence levels are: **D** declared, **C** compiled, **U** unit-executed, **R**
publicly reachable, **I** integrated, **A** adversarially tested, **P**
production-real. See the Implementation Reality Gate in
[`IMPLEMENTATION-PLAN.md`](IMPLEMENTATION-PLAN.md).

## Honest summary

- Fully implemented against the complete implementation plan: **no major
  subsystem yet**.
- Actually useful today: neutral values/expressions, SQLite knowledge and
  episode stores, deterministic procedure execution and replay, a working
  Teacher cycle for bounded learned procedures, several CLI/server/SDK paths,
  capability bundle lifecycle machinery, a broader bounded Unicode/text/
  collection/JSON intrinsic slice, a locally integrated scoped-file bridge, and
  a core-only grounded language-structure/renderer slice.
- Not actually present today: broad autonomous capability acquisition, a real
  OS sandbox, secret references, an executable seed forge, general language
  meaning/intent competence, general programming knowledge, or a complete pure
  standard library.

## System status

| System part | Status | Fully implemented? | Highest proven level | What is real now | What blocks FULL |
| --- | --- | --- | --- | --- | --- |
| Neutral value model | PARTIAL | No | I | Null, bool, signed integer, float, text, list, and string-keyed map persist and execute across core/graph/engine paths. | Bytes, exact decimal/big integer, tagged result/option/error, richer type/schema semantics. |
| Portable expression IR | PARTIAL | No | I/A | Literals, variables, arithmetic/logic, calls, conditionals, lexical bindings, lists, index/field, map/filter/reduce, versioned pure intrinsics, and exact-version `CallExact` dependency calls execute and serialize. Teacher aliases resolve only from a bounded engine snapshot and persist the dependency revision. | Computed map construction, richer collection forms, recoverable errors, and independent allocation/depth/output budgets. |
| Pure text/collection/JSON/numeric library | PARTIAL | No | U/I (engine-only focused tests) | Bounded Unicode normalization, trim variants, grapheme substring/split, search/count/repeat/concat, list/map copy transforms, deterministic sort/unique/flatten/zip/range, JSON parse/stringify, conversions, strict/optional paths, and finite-safe numeric abs/sign/min/max/clamp/rounding/power/strict integer quotient/remainder execute in the evaluator. Rich `pure_expr_v2` Engine tests author and reuse selected intrinsics without a Teacher. | Public rich-lesson RPC/SDK proof; case folding, regex, spans/tokens, schemas, encodings, hashes, exact decimal/rational arithmetic, transcendental math, and a complete standard-library surface. |
| Procedure evaluator | PARTIAL | No | I | Scoped deterministic execution, contracts, budgets, traces, nested calls, failures, and version-pinned replay work. | Call-depth/allocation/time cancellation limits, richer error handling, complete intrinsic library, effect dependency bridge. |
| SQLite knowledge graph | PARTIAL | No | I/A | Versioned concept/relationship/procedure storage, atomic bundles, traversal, dependency detection, lifecycle checks, and recovery tests are real. | Language/programming schemas, richer indexes/query planning, complete migration/performance/adversarial exit evidence. |
| Episode/session memory | PARTIAL | No | I | Durable episodes, query/update, observed facts, session visibility, recall modes, feedback, and recovery machinery exist. | Full current workspace revalidation, broader long-horizon retention/forgetting/reconciliation evidence, complete session UX/adversarial matrix. |
| Context and reasoning | PARTIAL | No | I | Context assembly, activation/ranking paths, ambiguity preservation, assumptions, and ladder traces exist. | Broad learned semantic interpretation, compositional goal reasoning, robust clarification/reference/dialogue behavior, stronger held-out evidence. |
| Cognitive cycle | PARTIAL | No | I | Interpret/context/run/ask/evaluate/persist paths work for current bounded scenarios; unknown tasks can ask or abstain. | Autonomous capability selection/acquisition, full credit/adapt loop integration on every path, effectful procedure execution, broader recovery and benchmark evidence. |
| Teacher adapters | PARTIAL | No | R/I (Codex only) | Claude CLI, Codex CLI, OpenAI, Ollama, and human transport/protocol code has mocked-transport tests. A live Codex CLI run now crossed the provider boundary through the public CLI after a provider-safe JSON envelope was added for recursive schemas. | Real end-to-end evidence for Claude/OpenAI/Ollama/human, resilience/timeout/cancellation parity, and cost/telemetry completeness. |
| Learned lesson admission | PARTIAL | No | R/I (Codex CLI smoke + Engine tests) | `pure_expr_v2` tests compile, execute, persist, reject unsafe drafts, compose bounded exact-version pure dependencies, and reuse with Teacher disabled. A clean live Codex CLI smoke learned `double`, answered 14, then with Teacher disabled answered held-out `double 11` as 22. | Public SDK rich-lesson coverage, broader real-backend/generalization cases, multiple declared/generated tests, and promotion criteria. |
| Credit assignment/replay | PARTIAL | No | I/A | Version-pinned replay, trace-based attribution, failure analysis, and counterfactual-related structures/tests exist. | Complete causal attribution across effectful/composed skills, calibrated evidence, broader adversarial and long-horizon validation. |
| Adaptation/reconciliation | PARTIAL | No | I/A | Planned mutations, lifecycle controls, contradiction/refinement handling, regression gating, and recovery machinery exist. | Complete autonomous candidate lab, broader rollback/recovery proofs, general learned procedure repair and dependency migration. |
| Intuition/local learning | PARTIAL | No | I | Local ranking/representation artifacts, activation, training/evaluation APIs, and telemetry exist. | Broad semantic competence, robust generalization across domains, calibrated model selection, sustained Teacher-reduction evidence. |
| Capability bundle format | PARTIAL | No | I/A | Typed procedures/dependencies/tests/schemas/provenance, deterministic content IDs, import/export, quarantine, reconstruction, local revalidation, and grant non-transfer are real. | Publisher signatures/registry, compatibility resolver/cache/lockfile, repair/migration/rollback, independent seed-forge publication workflow. |
| Capability discovery/acquisition | SCAFFOLD | No | R | Supplied interface descriptions can produce typed candidates through public RPC/SDK paths; fixture validation machinery exists. | Cognitive-cycle gap detection, real interface inspection, multiple candidate synthesis, generated tests, atomic candidate lab, autonomous admission/promotion. |
| Network primitive | SCAFFOLD | No | U | Exact-host policy, bounds, receipts, and injected adapter contract are tested. | Complete HTTP model, a configured real transport, public invocation path, egress protections, timeouts/redirects/streaming/adversarial integration. |
| File primitives | PARTIAL | No | R/I/A (local integration) | `capability.invoke` uses a server-configured scoped adapter for real temporary-directory read/write. Tests prove persistent grants, revocation, bounds, receipt redaction, symlink-escape denial, and unsupported-family failure; the current workspace gate passes. | Learned-procedure/cognitive-cycle selection, broader filesystem operations, real SDK-invocation integration, and production deployment. |
| Observation primitive | PARTIAL | No | R | A hard-coded `clock` observation is exposed through RPC and emits a redacted receipt. | Durable local grants, ordinary learned-procedure integration, and randomness/monotonic time/environment/platform/resource/user/device observation families. |
| Sandboxed execution | SCAFFOLD | No | U fixture | Policy, receipt, adapter boundary, and deterministic fixture executor exist. It explicitly spawns no process. | A real OS/container/WASI sandbox, executable identity, mounts/network/env/secrets/resource enforcement, public integration, escape/timeout tests. |
| Secrets/identity | MISSING | No | D | The plan and bundle rejection rules recognize that secrets must not transfer. | Opaque secret references, JIT adapter-only resolution, redaction enforcement, scopes/expiry/rotation, signing/verification identities. |
| Permission/grant system | PARTIAL | No | I/A through direct API | Local grants/revocation, ask/workspace/full-access policy, mandatory denials, and invocation-time checks exist in direct Rust paths. | Complete public invocation/config UX, cognitive-cycle use, secret-aware permissions, fresh end-to-end revocation evidence through real adapters. |
| Server JSON-RPC | PARTIAL | No | R/I | Knowledge, episodes, cycles, sessions, metrics, capability lifecycle, observation, scoped-file `capability.invoke`, and deterministic `language.render` paths are implemented and tested. The renderer accepts bounded typed plans and returns redacted audit/omission metadata; it does not independently verify caller evidence references. | Learned capability invocation/cognitive selection, evidence-backed Engine response-plan construction, all new grammar/schema integration, transport hardening, and production proof. |
| TypeScript SDK | PARTIAL | No | R/I | Typed client methods include capability invocation and bounded response-plan rendering; focused mapping tests and a real Rust-stdio renderer integration test pass. | Complete parity for every public server feature, current rich lesson coverage, real SDK capability-invocation process integration, and packaging release proof. |
| CLI | PARTIAL | No | R/I | Ask/chat/session/config/admin/benchmark flows and Teacher/Judge routing exist with tests. | Full current end-to-end matrix, capability invocation/acquisition UX, seed forge commands, richer explanations and stable packaging. |
| Inspector | PARTIAL | No | R | Bounded graph/episode/telemetry projections and inspector server machinery exist. | Complete operational UI, capability/seed/candidate lab visibility, current production integration and accessibility/performance evidence. |
| Benchmark system | PARTIAL | No | U | Catalog/fixture schemas and an unverified runner define acquisition/retention/generalization phases, Teacher/Judge adapters, and report shapes. | An executed public benchmark report, real Judge evidence, broader catalog coverage, stricter structural learning metrics, seed-curriculum runner, and stable longitudinal comparisons. |
| Seed curricula | SCAFFOLD | No | D | Strict schema-valid language, structured-data, and programming curriculum manifests define demonstrations, counterexamples, held-out gates, learned structures, privacy, and clean-import policy. | Seed-forge runner, clean-instance teaching, actual Teacher-OFF evidence, privacy filtering execution, export, second-clean-instance reconstruction/revalidation. |
| Seed forge | MISSING | No | D | Architecture and workflow are specified. | Every executable step: curriculum runner, structural inspection, Teacher ablation, safe export, independent import/revalidation, publication report/signing. |
| Language meaning and intent | PARTIAL | No | U | Core has bounded serializable UTF-8 token streams with byte-accurate spans, typed intent frames/slots/scope/ambiguity values, and dialogue moves. Five focused tests cover Unicode offsets, round-trip serialization, and bounds. | No semantic parser, learned surface-to-intent mapping, entity/reference resolution, conversational state, clarification policy, curriculum runner, or Teacher-OFF language evidence. |
| Conversational generation without LM | PARTIAL | No | I | Core plus public Server/SDK `language.render` accept typed response plans and content-free format/tone options. The deterministic renderer preserves supplied evidence-referenced claim text, omits unsupported claims, rejects evidence-free claims, and returns redacted audit metadata. It explicitly marks supplied evidence as unverified by this endpoint and never returns raw provenance. | No grammar/renderer procedures, natural varied generation, response-plan construction in Engine, dialogue-state integration, server-side evidence resolution, or grounding benchmark. |
| Programming knowledge/coding | MISSING | No | D | Architecture and curriculum targets include a bidirectional semantic Spoon-IR ↔ typed-code bridge; generic file/sandbox machinery is incomplete. | Repository/source/AST/symbol knowledge, parser/toolchain adapters, IR lowering/code lifting and differential equivalence, safe patch/test loops, grounded explanations, acquisition and Teacher-OFF benchmarks. |
| Configuration and sessions | PARTIAL | No | R/I | A CLI hierarchical resolver plus session RPC/SDK paths exist; the server consumes projected environment settings. | Full adversarial matrix, migration/atomicity validation after current changes, interactive grant UX, shared server/SDK hierarchy, and stable release proof. |
| Admin/security controls | PARTIAL | No | I/A | Authenticated admin mutations, lifecycle checks, trust receipts, grant denials, bundle quarantine, and several adversarial tests exist. | Complete threat-model closure, secrets/identity, production adapter hardening, fuzzing and current full-gate proof. |
| Documentation | PARTIAL | No | R | Architecture/plan, crate READMEs, benchmark docs, primitive inventory, handoff, and this status file exist. | Keep claims synchronized automatically with evidence; finish user guides, executable examples, migration/release docs. |
| Full build/release gate | IN FLIGHT | No | Current workspace checks + provider smoke | `cargo fmt --check`, `cargo test --workspace --all-targets`, strict full-workspace clippy, and TypeScript test/typecheck/build/depcheck pass after the new dependency/language/adapter edits. A live Codex teach/Teacher-OFF reuse smoke also passes. | An executed benchmark/report and a clean release package; new in-flight numeric work will require a fresh rerun. |

## Current in-flight work

These are not complete until their results are merged into the table above and
fresh checks are recorded:

1. Rich `pure_expr_v2` Teacher-authored procedure integration and cross-package validation.
2. Public permissioned capability invocation with a concrete scoped-file host adapter.
3. Continued pure standard-library completion beyond the first intrinsic slice.

## Related sources of truth

- [`IMPLEMENTATION-PLAN.md`](IMPLEMENTATION-PLAN.md) — target architecture,
  sequence, and exit criteria.
- [`PRIMITIVE-CAPABILITY-INVENTORY.md`](PRIMITIVE-CAPABILITY-INVENTORY.md) —
  operation-level checklist and reality-audit correction log.
- [`.agents/scratchpad/spoon/HANDOFF.md`](.agents/scratchpad/spoon/HANDOFF.md) —
  current recovery instructions and in-flight ownership.
- `~/.codex/skills/implementation-reality-audit` — reusable audit workflow that
  prevents declared/compiled/mock-only machinery from being reported as usable.

## Update rule

Any change that claims a subsystem or phase became implemented must update this
file in the same workstream with:

1. the highest contiguous evidence level actually demonstrated;
2. the public entrypoint and real adapter/workflow involved;
3. exact tests or benchmark proof;
4. remaining gaps; and
5. a downgrade when later evidence disproves the claim.

When evidence conflicts, the weaker status wins.
