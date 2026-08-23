# Spoon Implementation Plan

This is the phased implementation plan for building Spoon's executable
knowledge engine as described in the historical design document retained in the
local archive. It maps that architecture to concrete engineering work.

The public product and all current crate, package, binary, and environment
identifiers are named Spoon.

## Decisions Made

- **Runtime**: Rust core engine + TypeScript interaction shell
- **Teacher**: Abstract provider interface. Claude via `claude -p` CLI (SSO auth),
  OpenAI via API, Ollama for local/cheap self-supervision, human via CLI
- **Bootstrap domains**: Math/logic (Tier 1 signal), programming (execution
  grounding), then general multi-domain
- **Goal**: Usable open-source product
- **Persistence**: SQLite for graph + episodes (simple, embeddable, inspectable)
- **Protocol**: JSON-RPC over stdio between Rust server and TS shell
- **Capability substrate**: A deliberately small, policy-enforced native
  primitive set (network request, scoped file access, observation, and
  sandboxed execution). Everything richer is acquired as typed, contracted,
  decomposable knowledge rather than added as a privileged one-off tool.
- **Capability portability**: Reconstructible, content-addressed bundles may
  carry procedures, dependency graphs, schemas, tests, and provenance. They
  never carry trust, ambient authority, secret values, or unverified
  environment assumptions. Imports enter quarantine as Provisional and must
  pass local permission checks and local revalidation before promotion.
- **Configuration**: Spoon uses versioned, hierarchical configuration. User
  defaults come from `~/.spoon/config.json`; directory-level
  `.spoon/config.json` files are inherited from shallowest parent to the
  working directory; machine-local overrides, environment variables, and CLI
  flags are applied afterward. Configuration may select behavior and narrow
  authority, but repository-controlled files can never grant permissions,
  transfer secrets, or relax a local safety ceiling.
- **Episodic memory**: Recall is global by default. A named session is normally
  a continuity and ranking hint, not a memory wall. Isolation is explicit and
  durable: episodes created in an isolated session are available only inside
  that session and never enter global recall. Recall can be disabled for a
  request without disabling immutable episode recording.

### Execution model routing and spend policy

The coordinator should normally run on **GPT-5.6 Luna**. It owns the durable
handoff, selects the next bounded task, runs focused checks, and delegates only
when the risk or novelty warrants it. This keeps the long-running project
moving cheaply without treating a low-cost model as the final authority on
safety-critical design.

| Work type | Default model / effort | Escalate when | Completion standard |
| --- | --- | --- | --- |
| Task triage, repository reading, scratchpad/handoff updates, formatting, focused test runs, fixture generation, mechanical TypeScript/Rust edits | Luna / low–medium | The edit changes a security, concurrency, persistence, or public-contract invariant | Local test plus a diff review by the coordinator |
| Normal feature implementation, multi-file refactors, integration tests, debugging ordinary failures, API/schema design | Terra / medium–high | Two failed repair attempts, cross-crate invariants, or unclear plan/spec interaction | Focused suite and strict static checks |
| Threat modeling, adversarial audit, durable recovery/concurrency design, capability sandbox/permission policy, mutation authorization, ambiguous architecture decisions | Sol / high | Always use for the named risk areas; do not use merely for a larger mechanical task | Written invariant analysis plus adversarial regression tests |
| Independent final review of a phase | Terra / high, then Sol / high only for Phase 4–5 security/self-modification gates | A finding is disputed or the phase changes trust/authority boundaries | Requirement-by-requirement evidence, not only a green suite |

**Delegation protocol.** Luna may keep several independent cheap tasks moving,
but it should use at most one Terra or Sol task per shared mutable area at a
time. Before any handoff it records: exact files owned, invariant being proved,
commands to run, and the next recovery point in `.agents/scratchpad/spoon/HANDOFF.md`.
The coordinator integrates changes and reruns the affected tests; a subagent's
claim of success is never final evidence on its own.

**Phase routing.**

| Phase | Luna owns | Terra owns | Sol use (strictly limited) |
| --- | --- | --- | --- |
| P2 remaining gate | regression execution, metric fixtures, docs, mechanical API wiring | cross-crate integration and test repair | final trust/durability/adaptation audit |
| P3 intuition/self-supervision | corpus preparation, benchmark runners, data plumbing | ranking/retrieval implementation and evaluation | only if learned ranking could alter authority or promotion decisions |
| P4 consolidation/skill discovery | regression fixtures, reporting, routine package work | promotion gates, discovery pipeline, reconciliation | adversarial promotion and rollback review |
| P5 curiosity/self-modification and capability acquisition/sharing | bundle fixtures, docs, import/export round trips, routine tests | native primitive implementations, typed procedure generation, local revalidation | sandbox escape, permissions/effects, secrets, provenance, and self-modification authorization |
| P6 inspector/metrics | dashboard data transforms, snapshots, visual/test maintenance | server/SDK/dashboard integration and performance work | only for a security-sensitive exposure of evidence or authority |
| P7 configuration/session memory | config fixtures, schemas, CLI help, migration fixtures, conversational benchmark cases | resolver, native administration capability, session lifecycle, recall policy, SDK/server integration | config/grant authority lattice, isolation non-leakage, secrets, and migration audit |

The practical switch policy is: start a bounded task on Luna; retry once there
if the failure is mechanical; move to Terra for substantive code or a second
failed repair; reserve Sol for a named high-risk invariant or final adversarial
review. Do not use GPT-5.5 for this workflow: it is not the cost-efficient
choice relative to Terra for the same class of work. The model picker’s live
credit estimate remains authoritative for this account; this routing is a
quality/cost policy, not a guarantee of app-specific credit consumption.

## Repository Structure

```
  spoon/
  crates/
    spoon-core/          # data model: concepts, relationships, contracts,
                       # mutability classes, confidence, scope, evidence
    spoon-graph/         # persistent knowledge graph (SQLite-backed)
    spoon-exec/          # procedure execution engine
    spoon-episode/       # episode recording, storage, replay
    spoon-credit/        # credit assignment: contracts, replay, statistics
    spoon-reason/        # reasoning engine: contract-guided composition
    spoon-adapt/         # adaptation + knowledge reconciliation
    spoon-capability/    # native primitives, interface discovery, capability
                       # validation, portable bundle import/export
    spoon-engine/        # orchestrator: the full cycle from section 11
    spoon-server/        # JSON-RPC server exposing the engine

  packages/
    @spoon/cli/          # TUI + REPL for interacting with Spoon
    @spoon/teacher/      # teacher abstraction + provider adapters
    @spoon/inspector/    # web dashboard: graph viewer, episode browser,
                       # metrics dashboard (section 38)
    @spoon/sdk/          # TypeScript client for the spoon-server compatibility API

  tests/
    kitchen/           # the running example from the doc, as integration tests
    math/              # math/logic domain bootstrap tests
    programming/       # programming domain bootstrap tests
    falsification/     # section 38 metric measurement harness
```

---

## Implementation Reality Gate

The high-level current audit is [`STATUS.md`](STATUS.md); the operation-level
inventory is [`PRIMITIVE-CAPABILITY-INVENTORY.md`](PRIMITIVE-CAPABILITY-INVENTORY.md).
Update both when evidence materially changes a completion claim.

Every phase, capability, benchmark, README, and handoff must distinguish these
evidence levels instead of using “implemented” as an ambiguous umbrella:

1. **Declared** — a type, schema, enum, trait, command, or document names it.
2. **Compiled** — the declaration has an implementation that builds.
3. **Unit-executed** — focused tests execute useful logic and assert outcomes.
4. **Publicly reachable** — a supported engine/server/SDK/CLI path can invoke it.
5. **Integrated** — the real cognitive workflow selects, invokes, records, and
   evaluates it with production configuration and lifecycle rules.
6. **Adversarially tested** — denial, malformed input, resource exhaustion,
   revocation, dependency drift, and relevant attack paths have evidence.
7. **Production-real** — a non-mock adapter or environment performs the claimed
   effect end to end; fixture success is labeled fixture success.

Status language is strict:

- **Implemented** requires levels 1–4 for pure operations and 1–5 for workflow
  behavior.
- **Production-ready** requires all applicable levels through 7.
- Types, stubs, TODO branches, simulated receipts, deterministic fixtures,
  mocks, and injected closures are useful scaffolding but cannot alone support
  either claim.
- Every `[x]`, phase exit claim, and readiness summary cites the executable
  entrypoint and meaningful test or benchmark evidence. Missing evidence
  downgrades the claim; it is never filled in from intent.
- Audits inspect callers, configuration, feature flags, disabled branches,
  default adapters, error propagation, and persistence—not only defining files.

The living operation inventory is
[`PRIMITIVE-CAPABILITY-INVENTORY.md`](PRIMITIVE-CAPABILITY-INVENTORY.md). Run an
implementation-reality audit before closing each milestone and after any status
inventory rewrite.

---

## Priority Foundation Completion Track: Host-Capable Procedures, Language, and Programming

**Implementation status**: Active remediation. The core value/expression model
already contains lists, maps, indexing, field access, collection iteration, and
procedure calls, but the Teacher-admissible `pure_rpn_v1` lesson format exposes
only scalar arithmetic and boolean operations. That format is a safe bootstrap
subset, not the intended ceiling of learned behavior.

**Goal**: Complete the missing P0.3/P1.2/P5.3 substrate before treating later
capability, language, or programming benchmarks as meaningful. Spoon should be
able to learn rich, decomposable procedures over data and to request any host
behavior through typed, permissioned, locally authorized capability adapters.

The auditable operation-by-operation status and candidate backlog live in
[`PRIMITIVE-CAPABILITY-INVENTORY.md`](PRIMITIVE-CAPABILITY-INVENTORY.md). A
checkbox is complete only when the operation is publicly executable and tested;
declaring an enum variant or adapter contract counts as partial at most.

### Native machinery versus seeded knowledge

Native machinery is the smallest trusted substrate that cannot be usefully
expressed as ordinary Spoon knowledge. Seeded knowledge is normal inspectable,
versioned, testable knowledge that happens to ship with Spoon.

| Native / built in | Seeded or acquired knowledge |
| --- | --- |
| Value and expression semantics, budgets, tracing, deterministic pure intrinsics | Concepts, procedures, contracts, tests, language rules, domain workflows |
| Permission enforcement and typed network/file/observation/sandbox mechanisms | Dictionary clients, repository workflows, API integrations, spell checking |
| Cannot be mutated by ordinary learning | May be revised, superseded, retired, exported, and relearned |
| Changes require a runtime release | Changes through ordinary evidence and promotion gates |
| May exercise explicit local grants at the host boundary | Cannot create, widen, or transfer authority |

If a behavior can be transparently composed from existing primitives, prefer a
seeded/acquired procedure over another privileged native operation. Shipped
seed bundles do not inherit ambient trust: reconstruct and run their deterministic
bootstrap tests locally before marking them Validated.

### P0F.1 - Versioned Portable Procedure IR

Preserve `pure_rpn_v1` for compatibility and add a richer, versioned portable
expression grammar. Reuse `spoon-core::Expr` rather than creating a parallel VM.

The new grammar must express, with hard depth/node/item limits:

- literals for every neutral value type;
- variables, lexical bindings, conditionals, blocks, and immutable construction;
- list and map construction;
- indexing, strict field/path access, and optional access;
- bounded map, filter, reduce, find, count, any/all, sort, unique, flatten,
  zip, and slice;
- calls to exact engine-resolved procedure identities and versions;
- calls to a versioned pure intrinsic vocabulary;
- executable contract expressions and multiple declared test cases.

Teacher output refers to existing dependencies by stable engine-supplied keys,
never by Teacher-minted IDs. The engine resolves and pins identities before
execution or storage. Persisted legacy expressions remain readable.

**Deliverable**: a Teacher can propose a bounded, recursive, decomposable pure
procedure over text, lists, maps, and JSON, and Spoon can compile, trace, replay,
serialize, and reject it deterministically.

### P0F.2 - Complete Pure Standard Library

Add versioned, budget-charged intrinsics with explicit semantics and errors.

**Text and Unicode**

- byte length, Unicode scalar length, and grapheme-cluster length are separate;
- normalize, case-fold, lower/upper, trim, concatenate, split, join, search,
  replace, substring, prefix/suffix, and bounded non-backtracking regular expressions;
- split-on-empty operates on grapheme clusters rather than silently exposing
  UTF-8 bytes;
- token/span operations retain source offsets and normalization provenance.

**Collections and objects**

- length, keys, values, entries, contains, count, map, filter, reduce, find,
  any/all, sort, unique, flatten, zip, slice, and bounded range generation;
- immutable get/set/delete/update operations;
- strict access distinguishes a missing key/index from a present null;
- optional access returns null only for absence, not malformed paths or type errors.

**JSON and structured data**

- bounded parse and deterministic stringify;
- dot and bracket paths such as `user.profile.name`, `items[0].id`, and
  `["keys.with.dots"]`, plus a standards-based JSON Pointer operation;
- parse/stringify and property traversal preserve JSON structure and numeric
  validity, with byte/depth/segment limits;
- schema/type checks, conversions, coalesce, and deterministic hashing/encoding.

This layer is authority-free. Clocks, randomness, environment variables,
network, files, processes, and secrets are observations/effects and never pure
intrinsics.

**Deliverable**: `count_letter("strawberry", "r")`, nested JSON extraction,
and nontrivial collection transforms are expressible and testable without a
Teacher or host effect after their procedures have been learned.

### P0F.3 - Typed Host Effect Bridge

Anything the host can do should be exposable, but only through the existing
capability authority boundary:

```text
learned procedure
  -> typed capability dependency and effect request
  -> exact local capability/version resolution
  -> schema, contract, bounds, and grant checks
  -> injected host adapter
  -> redacted receipt, observation, evaluation, and trace
```

Complete the native mechanism families:

- scoped network requests with method/host/path/body/response budgets;
- scoped file read, write, list, metadata, and atomic patch operations;
- identified observation/sensor calls with time and provenance;
- sandboxed execution with declared executable identity, arguments, inputs,
  outputs, environment keys, filesystem/network policy, timeout, memory, and
  output limits;
- secret references resolved only at invocation, never stored in procedures,
  prompts, bundles, traces, or receipts.

Every invocation re-checks current local grants and mandatory denials. Pure
evaluation cannot reach these adapters directly. Full-access mode removes
routine prompts but not declarations, bounds, quarantine, receipts, or
operating-system limits.

**Deliverable**: a locally revalidated learned procedure can compose pure
steps with authorized network/file/observation/sandbox steps; revocation takes
effect before the next invocation.

### P0F.4 - Candidate Laboratory and Admission

Wire capability acquisition into the cognitive cycle:

1. Detect an explicit missing operation, interface, dependency, or language mapping.
2. Search existing pure procedures and locally available capabilities first.
3. Inspect an operator-authorized schema, help document, fixture, source tree,
   or observed exchange.
4. Synthesize several typed candidates with contracts, effects, dependencies,
   tests, and provenance.
5. Compile in quarantine and run declared examples, generated boundary cases,
   counterexamples, and selected regressions under budgets.
6. Admit a successful candidate as Provisional with no automatic grants.
7. Promote only after locally trusted evidence satisfies the Phase 4 gate.
8. Persist rejected candidates and failure traces without partially mutating
   active knowledge.

The laboratory may ask a Teacher to propose candidates, but candidate creation,
testing, comparison, and admission are first-class Engine states. A later phase
must let Spoon originate and revise candidates before Teacher escalation.

**Deliverable**: gap -> candidate -> local test -> provisional capability ->
Teacher-OFF reuse is visible as one durable public workflow.

### P0F.5 - Language Substrate

Language is a first-class capability domain over the same values and procedures,
not a privileged answer generator.

Add explicit representations for:

- normalized text, tokens, graphemes, spans, entities, and references;
- weighted intent frames, slots, scope, ambiguity, and clarification choices;
- dialogue acts, corrections, conversational state, and user preferences;
- response plans containing grounded claims, requested action, uncertainty,
  provenance, tone, and disclosure requirements;
- semantic-to-text generation procedures and a deterministic no-model renderer.

Learn mappings from surface forms to intent frames and from response plans to
utterances. Purpose-built fuzzy/neural interpreters and renderers are allowed,
including small local models, but they propose structure or wording; they do
not contain the authoritative facts, create effects, or mutate knowledge. A
renderer is constrained to the immutable response plan and checked for claim
and provenance preservation.

Seed only the linguistic substrate needed to bootstrap acquisition: core
token/span operations, a small intent/dialogue ontology, deterministic grammar,
and validation fixtures. Vocabulary, paraphrases, domain language, repair
strategies, and stylistic preferences should grow as ordinary knowledge.

**Initial bounded slice (implemented, bounded lexical and rendering paths):** UTF-8 token streams preserve
byte offsets and are exposed to `pure_expr_v2` as bounded `text_tokenize`
records (`kind`, exact source text, `startByte`, `endByte`); serializable intent frames/slots/scope/ambiguity and dialogue moves
carry neutral structure; response plans carry evidence-backed claims, provenance,
uncertainty, tone, and a formatting variant. The no-model renderer preserves
claim text verbatim, omits explicitly unsupported claims, rejects claims without
evidence references, and varies only plain versus bullet formatting. A bounded
`language.render` Server/SDK path accepts typed plans and content-free tone/format
overrides, redacts raw provenance, and labels caller evidence references
unverified rather than elevating them. This is deliberately not semantic
interpretation, server-side evidence verification, or natural-language
generation. Its current proof is five focused `spoon-core` tests plus server and
real-stdio SDK tests for bounds, omission, redaction, rejection, and a
Teacher-OFF tokenized word-count procedure. Tokenization remains lexical only;
it does not infer intent, entities, or syntax trees.

**Deliverable**: Spoon learns a general letter-count intent and procedure,
handles paraphrases with Teacher OFF, asks when literal versus canonical
spelling matters, and renders a grounded conversational answer without relying
on a canned exact phrase.

### P0F.6 - Programming Knowledge and Coding Capabilities

Programming is the first broad grounded domain because compilers, tests,
parsers, and repositories manufacture strong feedback.

**Repository knowledge**

- scoped file/tree observations and language detection;
- manifests, configuration formats, modules, files, symbols, types, imports,
  references, dependencies, call edges, tests, diagnostics, and ownership;
- AST and source-span provenance through typed parser capabilities;
- incremental invalidation when file fingerprints change.

**Acquired programming capabilities**

- compiler, interpreter, formatter, linter, test-runner, package-manager,
  documentation, Git-read/status/diff, patch, and build-system interfaces;
- exact executable/version/environment fingerprints and reconstructible tests;
- read/observe, sandbox execution, network, and mutation effects kept distinct;
- safe workflows such as inspect -> hypothesize -> patch in scope -> run focused
  checks -> evaluate -> retain/revise.

**Bidirectional semantic IR/code bridge**

Treat language constructs as evidence about shared semantic operations, not as
textual aliases. Maintain a versioned many-to-many mapping among Spoon
intrinsics/procedures/contracts/effects and target-language AST/type constructs.

- **Lowering**: translate an exact Spoon procedure and its dependency closure
  into typed TypeScript, Rust, Python, or another target plus source maps,
  runtime shims, declared imports, effects, and build/test instructions.
- **Lifting**: parse authorized source through a typed AST capability, recognize
  representable semantic regions, and propose neutral concepts/procedures with
  source-span provenance. Unrepresentable or effectful regions remain opaque,
  typed capability dependencies rather than invented pure semantics.
- **Equivalence**: execute the neutral IR and generated code against declared,
  boundary, generated, and held-out inputs in separate sandboxes; compare
  outputs, typed failures, effects, and resource envelopes before admission.
- **Semantic false friends**: require explicit target shims or reject lowering
  when language behavior differs. For example, JavaScript `split("")` operates
  on UTF-16 code units and is not Spoon's grapheme split; optional chaining is
  not strict path access; floating-point, overflow, object ordering, null/missing,
  exception, async, and Unicode semantics must remain explicit.
- **Artifact policy**: neutral IR, contracts, tests, dependency identities, and
  provenance remain authoritative. Generated source/binaries are reproducible
  cached artifacts, never a transfer of authority or a substitute for local
  compilation and validation.

Seed a small cross-language semantic ontology—expression, binding, function,
call, collection transform, record/property, result/error, module/import,
effect, async task, contract, test, and type relationships—then acquire target
syntax, libraries, idioms, and compiler constraints as ordinary knowledge.

**Deliverable**: Spoon lowers one nontrivial learned data procedure to at least
TypeScript and Rust, proves differential equivalence on held-out cases, imports
an equivalent hand-written implementation back into a neutral candidate, and
rejects a deliberately misleading syntax-level translation.

**Grounded explanation and conversation**

- build a response plan from inspected files, symbols, dependencies, tests,
  and observed runtime behavior;
- link every factual repository claim to evidence;
- explain at the user's altitude and retain conversational corrections without
  allowing prose fluency to substitute for code evidence.

**Deliverable**: on an authorized fixture repository, Spoon can learn the
local toolchain, build an inspectable code knowledge graph, answer “what is this
repo doing?” conversationally from evidence, and acquire a reusable tested
coding workflow without ambient shell or file authority.

### P0F.7 - Foundation Benchmarks and Exit Gate

Add isolated public fixtures that prove behavior rather than one-shot answers:

- JSON parse/stringify and strict/optional nested property access;
- Unicode letter/grapheme counting, including `strawberry` and held-out words;
- learned list filtering/reduction and procedure composition;
- fake dictionary interface discovery, permission denial, local validation,
  revocation, offline fallback, and provenance;
- paraphrase-to-intent retention and clarification behavior;
- deterministic and varied response generation from one immutable response plan;
- fixture-repository indexing, grounded explanation, test workflow acquisition,
  safe patching, and Teacher-OFF reuse;
- adversarial depth/size/budget, schema, path, regex, secret, effect, and sandbox cases.

Each fixture gets a fresh database to prevent cross-fixture knowledge leakage.
Acquisition, retention, and held-out variants share only that fixture database.
Reports count step-level acquisition/retention/generalization failures directly
and include the learned procedure/capability structure needed by the Judge.

### P0F.8 - Seed Forge and Curated Learning Curricula

Seed knowledge is produced through the same observable learning machinery used
in normal operation, not inserted as opaque privileged state. A seed curriculum
is a versioned set of concepts, demonstrations, counterexamples, exercises,
held-out variants, expected structural observations, and promotion criteria.

The seed-forge workflow is:

1. Start a clean Spoon with only native machinery and an empty knowledge store.
2. Run a named curriculum with explicitly selected Teacher and capability grants.
3. Inspect the acquired concepts, procedures, contracts, dependencies, tests,
   failures, and provenance rather than accepting answer accuracy alone.
4. Remove Teacher access and run retention, composition, and held-out
   generalization experiments.
5. Export only reconstructible knowledge; omit episodes or examples that would
   leak private data, secrets, ambient grants, machine paths, or Teacher state.
6. Import into a second clean Spoon as Provisional, resolve dependencies, and
   rerun deterministic tests plus the curriculum's Teacher-OFF validation set.
7. Sign and publish the bundle manifest only after independent reconstruction
   succeeds. Installation never transfers trust or authority; each target
   performs its own local validation and promotion.

Initial designed curricula:

- **language kernel**: graphemes, tokens, spans, compositional meaning, intent,
  slots, reference, ambiguity, clarification, dialogue acts, and response plans;
- **structured data**: JSON, paths, schemas, collection transforms, comparison,
  grouping, aggregation, and error-sensitive data workflows;
- **everyday reasoning**: units, time representations, counting, classification,
  decomposition, verification, and explanation strategies;
- **programming foundations**: syntax/AST concepts, types, modules, dependency
  graphs, testing, diagnostics, version control, debugging, and safe change loops;
- **tool-use patterns**: interface discovery, typed adapter construction,
  permission requests, provenance, fallback, revocation, and capability repair.

A curriculum may deliberately use a strong Teacher to accelerate acquisition,
but Teacher text is training evidence, not shipped cognition. The exported seed
must remain useful with the Teacher absent and must expose the structures that
cause its behavior. Seeds are revisable ordinary knowledge: users may inspect,
replace, retire, or relearn them.

**Deliverable**: a reproducible command creates a seed bundle from a clean
database, independently reconstructs and validates it in another clean database,
and emits a curriculum report distinguishing acquisition, retention,
generalization, structure, Teacher dependence, and unresolved assumptions.

### P0F Exit Criteria

- The pure IR can express rich deterministic transforms over text, collections,
  maps, and JSON with complete budgeted traces.
- Learned procedures can compose exact-version pure and locally authorized
  effectful dependencies without ambient authority.
- New host integrations require typed adapters/bundles, not changes to the
  trusted evaluator.
- Candidate synthesis, tests, quarantine, admission, rejection, and promotion
  evidence are durable public workflow states.
- Curated seed bundles are reproducibly learned, exported, reconstructed, and
  independently validated rather than copied from a trusted database snapshot.
- Letter counting and JSON traversal are learned once and generalized with the
  Teacher disabled.
- Spoon can construct a grounded repository model and conversational explanation,
  then acquire a safe programming workflow from local evidence.
- Legacy databases and `pure_rpn_v1` lessons remain readable and executable.
- Workspace tests, clippy, TypeScript tests/typecheck/build/depcheck, adversarial
  capability tests, and isolated public benchmarks are green.

---

## Phase 0: Seed

**Maps to**: Section 33, Stage 0
**Goal**: Primitives, execution, episode recording, evaluation. Nothing learned
can be graded until evaluation exists, nothing can be credited until episodes
are recorded.

### P0.1 - Core Data Model (`spoon-core`)

The foundational types everything else builds on.

```
Concept
  - id, name
  - mutability_class: Definitional | DefeasibleGeneral | Particular |
                      Procedural | Normative | CoreMachinery
  - confidence: { support, contradiction, scope, sources, last_tested }
  - lifecycle: Active | Validated | Provisional | Stale | UnderReview |
               Superseded | Retired | Invalid

Relationship
  - source, target, kind, strength
  - scope conditions
  - evidence (episodes that support it)

Procedure
  - id, name
  - body: a tree of PrimitiveOp | ProcedureCall | Conditional | Iterate
  - contract: { requires, promises, fails_when, costs }
  - version history
  - test cases (self-growing regression suite)

Episode (section 18)
  - situation, interpretation_candidates (including losers)
  - context_assembled (with assumptions marked)
  - knowledge_considered (surfaced + rejected)
  - reasoning_trace (steps with contracts matched)
  - prediction
  - action
  - observed_result
  - evaluation (tier, verdict)
  - cost (rung_reached, budget_spent)

Evidence
  - tier: Hard | Consensus | Deferred
  - source, timestamp
  - linked episode
```

**Deliverable**: A Rust crate with these types, serialization (serde), and
a small set of primitive operations (arithmetic, comparison, string ops).

### P0.2 - Knowledge Graph (`spoon-graph`)

Persistent storage for concepts, relationships, and procedures.

- SQLite-backed (single file, inspectable, embeddable)
- CRUD for all entity types
- Relationship traversal (typed, bounded hops)
- Basic query: find concepts by name, by relationship, by type
- Dependency tracking: "what depends on this concept/procedure?"
- Version history for procedures and contracts

**Not yet**: similarity search, learned ranking, compression. Those come in
later phases when episodes exist to train on.

**Deliverable**: A graph you can populate, query, and traverse. Inspectable
via `sqlite3` directly if needed.

### P0.3 - Execution Engine (`spoon-exec`)

Run procedures and produce results.

- Execute procedure bodies against a set of bound inputs
- Pure execution only (no side effects) - section 16's safe tier
- Primitive operations: arithmetic, comparison, string, list, lookup
- Procedure composition: call one procedure from another
- Contract checking: validate requires before execution, check promises after
- Execution trace capture: every step recorded for replay
- Timeout/budget: bounded execution, anytime interruption

The procedure representation should be:
- **Neutral**: not "TypeScript" or "Python" - an internal representation
  reducible to its parts (section 6)
- **Decomposable**: composed skills remain inspectable as constituent parts

**Deliverable**: Can define `DOUBLE(x) = x * 2`, execute it, capture the
trace, verify the contract.

### P0.4 - Episode Recording (`spoon-episode`)

The raw material of every learning mechanism downstream.

- Record full episodes per section 18's structure
- Persist to SQLite (same DB or separate - TBD)
- Query: by time, by concepts involved, by outcome, by rung
- Replay: re-run a stored trace with substitutions (foundation of
  credit assignment in P3)
- Never delete failed episodes

**Deliverable**: Episodes are recorded during execution, stored, queryable,
and the trace is replayable.

### P0.5 - Evaluation (`spoon-core` or `spoon-engine`)

The grading machinery. Without this, nothing learned can be judged.

- Verifiability tier classification (section 19)
  - Tier 1: deterministic check (test pass/fail, arithmetic, types)
  - Tier 2: independent methods agree, inverse recovers input
  - Tier 3: human judgment, deferred, weak
- Comparison: predicted vs observed result
- Surprise detection: did the result differ from prediction?
- Decomposition helper: break a weakly-verifiable goal into checkable
  subgoals (section 20's signal manufacturing)

**Deliverable**: Given a prediction and an observation, produce a tiered
evaluation with surprise signal.

### P0.6 - Server + Basic CLI

- `spoon-server`: JSON-RPC over stdio, exposes graph CRUD, procedure
  execution, episode query
- `@spoon/cli`: minimal CLI that connects to the server, lets you:
  - Define concepts and relationships manually
  - Define and execute procedures
  - Inspect the graph
  - View episodes
- `@spoon/sdk`: TypeScript client wrapping the JSON-RPC protocol

**Deliverable**: A running system you can interact with. Dumb, but
functional. "Stage 0" from section 33 is complete.

### P0 Exit Criteria

- Can define concepts, relationships, procedures with contracts
- Can execute procedures and capture traces
- Can record and query episodes
- Can evaluate results by tier
- Can replay a trace
- Kitchen test: manually define DOUBLE, execute it, record the episode,
  verify the trace, replay with a substitution

---

## Phase 1: Interpretation + Teacher

**Maps to**: Section 33, Stages 1-2
**Goal**: Understand input well enough to act, leaning heavily on a teacher.
Expect high teacher dependence - this is correct, not failure.

### P1.1 - Teacher Abstraction (`@spoon/teacher`)

```typescript
interface Teacher {
  propose(request: TeacherRequest): Promise<TeacherProposal>
  reliability(): SourceReliability  // tracked over time
}

interface TeacherRequest {
  situation: string
  context: KnowledgeContext
  specific_question?: string
  desired_output: ProposalSchema
}

interface TeacherProposal {
  content: StructuredProposal
  source: string
  status: 'unverified'  // NEVER automatically true
}
```

Provider adapters:
- **ClaudeTeacher**: spawns `claude -p`, sends structured prompts, parses
  JSON output. Uses system prompt that instructs Claude to output proposals
  in Spoon's schema, not just answers (section 30: extract the lesson, not
  the answer)
- **OpenAITeacher**: standard HTTP API calls
- **OllamaTeacher**: local models, cheap, good for high-volume self-supervision
- **HumanTeacher**: CLI prompt, waits for human input

All teacher output goes through the validation pipeline (section 30):
```
proposal -> validate -> verified | rejected | provisional
```

Teacher prompts must seek the full reusable lesson supported by the evidence:
language and terminology, definitions and meaning, user intent, inputs and
outputs, relationships, constraints, examples, and explicit uncertainty—not
only an answer. A reusable lesson may carry bounded definitional or
defeasible-general concepts alongside one to four focused procedural concepts.
Procedures may compose through acyclic, engine-resolved `lesson:<procedure-key>`
dependencies; the invocation names the selected final procedure. Every item is
still Provisional on admission, and unsupported facts, ambient assumptions, or
capabilities must be omitted rather than invented.

Source reliability is tracked per teacher over time.

**Deliverable**: Can ask any teacher a structured question, get a proposal
back, validate it, and integrate or reject it with provenance.

### P1.2 - Interpretation (section 12)

Map natural language input to internal concepts.

Initially teacher-dependent: send input to teacher, get back candidate
interpretations with weights.

```
"Could you double that?" ->
  [
    { meaning: DOUBLE, weight: 0.91 },
    { meaning: REPEAT, weight: 0.06 },
    { meaning: UNKNOWN, weight: 0.03 }
  ]
```

Key properties:
- Ambiguity preserved, not collapsed (weights sum to 1)
- Unresolved ambiguity is a legitimate output
- Losing interpretations stored in the episode (needed for credit assignment)

Later (P3): learn interpretation from accumulated episodes, reducing
teacher dependence.

**Deliverable**: Natural language input -> weighted candidate meanings
referencing graph concepts. Teacher-powered initially.

### P1.3 - Context Assembly (section 13)

Build the active working context for a task.

- Current goal and why
- Entities under discussion
- Relevant knowledge (graph neighborhood of active concepts)
- Recent actions and results
- Active assumptions (MARKED as assumptions - critical for credit assignment)
- Environmental state
- Budget remaining

Initially: simple heuristic assembly (graph neighbors of mentioned concepts,
recent episodes). Later (P3): learned context selection.

**Deliverable**: Given an interpreted input, assemble a bounded context
from the graph and episode history.

### P1.4 - Minimal Reasoning Cycle (section 11, simplified)

Wire together interpretation -> context -> reasoning -> execution ->
evaluation into a single loop.

- Interpretation: teacher-assisted (P1.2)
- Context: heuristic assembly (P1.3)
- Intuition: SKIP for now, use everything in context
- Reasoning: try to match a known procedure's contract, or ask teacher
- Execution: run procedure (P0.3)
- Evaluation: grade result by tier (P0.5)
- Episode: record everything (P0.4)

Metacognition: simple escalation ladder (section 17)
  1. RECALL - do I already know?
  2. RUN - do I have a procedure?
  3. ASK - teacher
  4. ABSTAIN - say so

**Deliverable**: Give Spoon a task, it attempts to solve it (badly), and
records the full episode. The kitchen test should partially work.

### P1 Exit Criteria

- Teacher integration working with at least one provider
- Can receive natural language, interpret it, attempt to solve it
- Full episodes recorded for every attempt
- The escalation ladder works (recall -> run -> ask -> abstain)
- Can learn from teacher proposals (with validation)
- Kitchen test: "what is double 7?" works end to end

---

## Phase 2: Credit Assignment + Adaptation

**Maps to**: Sections 21-23, the hardest part
**Goal**: When something goes wrong, identify what was responsible and make
a targeted correction. This is Claim 1 - if it doesn't work, nothing else
matters.

### P2.1 - Contract Violation Detection (section 21, mechanism 1)

Cheapest, sharpest credit assignment.

- Walk the reasoning trace
- Check each procedure's contract: was any precondition violated? Did any
  postcondition fail?
- If yes: that's the suspect, high confidence, one pass, no re-execution

**Deliverable**: Given a failed episode, detect contract violations in the
trace. Cost: O(trace length).

### P2.2 - Statistical Attribution (section 21, mechanism 3)

Cheap, blurry. Used to rank candidates for replay.

- Track per-element failure rates across episodes
- When a failure occurs, rank elements by historical failure rate
- Distinguish correlated elements (SCALE_ALL and linear scaling co-occur)
- Output: ranked suspect list, NOT conclusions

**Deliverable**: Given a failed episode, produce a ranked list of
suspects from cross-episode statistics.

### P2.3 - Counterfactual Replay (section 21, mechanism 2)

Strong, expensive. The mechanism that justifies executable knowledge.

- Take a failed trace
- For the top-K suspects (from P2.1 + P2.2):
  - Re-run the trace with exactly one element changed
  - Observe whether the outcome changes
- Deterministic replay (swap an operation) -> potentially decisive
- Simulated replay (swap a planning decision) -> strong evidence
- Track attribution confidence and provenance

Cost controls:
- Only replay top-K suspects (guided by mechanisms 1 and 3)
- Budget-bounded: stop if cost exceeds value
- Track attribution cost as a fraction of total cost (metric 8 in section 38)

**Deliverable**: Can identify the responsible element in a failed trace
with confidence levels. The three mechanisms compose: contract check first,
statistical ranking to prioritize, replay to confirm.

### P2.4 - Adaptation (section 22)

Turn attribution into targeted change.

Correction scope, narrowest first:
1. **Record only** - input was unusual, change nothing
2. **Fix assumption** - context was wrong, not the procedure
3. **Narrow scope** - add a discovered condition to the contract
4. **Replace procedure** - it fails inside its own scope
5. **Revise concept** - requires corroboration, offline

Evidence thresholds per correction width (section 22):
- Record only: 1 episode
- Narrow scope: 1-2 episodes, Tier 1 or 2 evidence
- Replace procedure: several episodes, must beat incumbent
- Revise concept: many episodes, corroborated, offline only

Attribution confidence gates the response:
- Contract violation, certain -> act now, narrowly
- Replay-confirmed -> act now
- Statistical suspicion only -> schedule a test, wait for more episodes

**Deliverable**: Failed episodes produce specific, local modifications
rather than general dissatisfaction.

### P2.5 - Knowledge Reconciliation (section 22)

When something changes, propagate consequences.

- Follow dependency structure outward from the changed element
- For each dependent: still valid? narrower scope? stale? invalid?
- Invalidation is NOT deletion - check for alternative justifications
- History records are immutable; current understanding is revisable
- Lifecycle states: active, validated, provisional, stale, under_review,
  superseded, retired, invalid

**Deliverable**: A concept revision propagates through dependent knowledge
without naive cascade deletion.

### P2.6 - Contradiction as Refinement (section 23)

When two sources disagree, default to scope refinement, not arbitration.

- Detect contradiction: two claims with conflicting implications
- Search for discriminating feature between supporting cases
- If found: split the claim into two scoped claims (both can be true)
- If not found: hold the contradiction as a first-class object
  - Reasoning that depends on it inherits uncertainty
  - It becomes a curiosity target (P4)
  - It may resolve later from unrelated observation

**Deliverable**: Contradictions produce scope refinements or held
contradictions, never silent averaging.

### P2 Exit Criteria

- Credit assignment works on injected faults (metric 7)
- Attribution cost is reasonable relative to total cost (metric 8)
- Adaptations are narrow and targeted
- Knowledge reconciliation propagates without cascading destruction
- Contradictions refine scope rather than destroy information
- Kitchen test: flat pancakes -> leavening rule scope correction

---

## Phase 3: Intuition + Self-Supervision

**Maps to**: Section 33, Stage 3 + section 24
**Goal**: First compounding step. Search gets cheaper, more problems
become reachable, more episodes accumulate.

### P3.0 - Public Benchmark Harness

Benchmark probes must exercise the same public `spoon ask` entrypoint as a
human rather than calling engine internals. The runner accepts normalized JSON
fixtures and writes JSON plus Markdown reports.

For acquisition/retention probes, the runner enforces this sequence:

1. Teacher ON canonical prompt: allow an answer or reusable lesson.
2. Teacher OFF exact repeat: measure retention only; mark it as a regression
   repeat rather than new capability evidence.
3. Teacher OFF paraphrase or novel-value variants: run only after exact
   retention passes; these are the generalization checks.
4. Fresh public CLI/server process against the fixture's database: verify
   durable persistence. Catalog execution creates a fresh temporary database
   per fixture, so unrelated fixtures cannot confound a capability-acquisition
   result. Deliberate interference belongs in a fixture that teaches its own
   competing procedures.

Each step records the public answer, disposition, episode, Teacher usage,
action, rung, trace/cost summary, and novelty identity. Held-out variants stay
in separate fixture families, and failed retention gates produce explicit
`skipped` variants rather than misleading successes.

Semantic fixture criteria are evaluated after fixture completion by a separate
**Judge** protocol. Judge reuses the provider transport/adapters (CLI or API)
but has its own prompt and strict verdict schema; it receives batched,
immutable, redacted per-step evidence and returns one independent verdict per
step. It has no engine, graph, episode, promotion, or capability-write path. A
Teacher-OFF result remains Teacher-OFF because Judge cannot influence
execution. Deterministic assertions remain authoritative for exact values,
contracts, and Teacher-call policy; Judge verdicts grade rubric criteria and
record their provider/model/provenance. Human ratings remain required for the
Bar Test.

User-facing commands:

```text
spoon ask --teacher off "..."
spoon benchmark run <fixture.json> [report.json]
spoon benchmark report <report.json>
```

The developmental catalog lives in `benchmarks/catalog.json`, with starter
experiments under `benchmarks/fixtures/`. Its source of truth is the
developmental probe suite in `ekg-benchmark-suite/`; the harness is
intentionally a runner/reporting layer and does not seed answers or silently
bypass the normal Teacher and episode paths.

Passing `benchmarks/catalog.json` to the same `benchmark run` command resolves
its suite fixture IDs, runs each fixture as a public experiment, and writes one
aggregate report with per-fixture telemetry run IDs. A catalog run preserves
fixture-local acquisition, optional `teach-*`, retention, and held-out gates
within a clean fixture-local database.

### P3.1 - Recall Index (section 14, stage 1)

Cheap, broad candidate generation without scanning everything.

- Similarity index over concepts (embedding-based, using a local model
  or teacher-generated embeddings)
- Typed traversal: follow relationships of the right kind, bounded hops
- Activation spread: start from context, propagate with decay
- Recency and frequency weighting

Output: hundreds of candidates, unranked, cheaply obtained.

### P3.2 - Learned Ranking (section 14, stage 2)

Score candidates using accumulated experience.

- Training data: episodes record what was considered, what was used,
  what succeeded, what was considered and rejected
- Model: small learned ranker (could be a local model fine-tuned on
  episodes, or a simpler ML model)
- Output: ordered shortlist small enough to reason over

### P3.3 - Representation Self-Supervision (section 24)

Train on episode structure without changing beliefs.

Safe targets (grounded in actual episodes):
- Predict missing words/phrases from context
- Predict validated interpretation from utterance + context
- Predict useful knowledge from situation + goal
- Predict next successful step from reasoning prefix
- Predict which procedure succeeds from candidates

Boundary: may freely update representations, similarity, retrieval,
interpretation probabilities, ranking policies. May NOT declare world
claims true, resolve contradictions, promote abstractions.

### P3.4 - Epistemic Self-Supervision (section 24)

Self-generated challenges that terminate externally.

- Hide a computation, predict, execute to check
- Invert a known skill, check the round trip
- Stress a contract boundary
- Predict a consequence, compute to verify

Grounding requirement: a meaningful fraction must terminate in execution,
a test, or an observation. "The graph agreed with itself" is not
acceptable.

Rationed, not unlimited. Track belief provenance by depth - beliefs
supported only by other beliefs are flagged for re-verification.

### P3 Exit Criteria

- Recall index returns relevant candidates without scanning everything
- Learned ranking improves search (fewer candidates explored for same
  results - metric 5, rung distribution)
- Self-supervision trains representations without belief drift
- Grounding ratio is tracked (metric 10)
- Problems increasingly resolve at cheaper rungs (metric 5)

---

## Phase 4: Consolidation + Skill Discovery

**Maps to**: Section 33, Stage 4 + sections 26-28
**Goal**: Enough volume for patterns to be real. Regression suite large
enough to gate promotion. First test of Claim 2.

### P4.1 - Skill Discovery (section 26)

Three routes:

1. **From repetition**: detect shared structure across episodes
   (multiple scaling tasks -> SCALE_RECIPE skill)
2. **From a single success**: explain WHY it worked, generalize along
   the explanation (not surface similarity)
3. **From failure**: generalize the failure into a critic (a precondition
   check that prevents a class of future failures)

### P4.2 - Promotion Gate (section 27)

The most important safety mechanism.

Candidate must demonstrate measurable win against incumbent:
- **Correctness**: same or better on every replayed verified episode.
  NON-NEGOTIABLE.
- **Compression**: shorter traces for same results
- **Search cost**: fewer candidates explored
- **Coverage**: solves episodes the incumbent could not
- **Transfer**: helps in a domain it wasn't derived from (strongest signal)

Pipeline: replay against history -> must win on at least one, lose on
none -> shadow deploy alongside incumbent -> promote on live win

### P4.3 - Self-Growing Regression Suite (section 27)

Every episode with a Tier 1 or 2 verified answer becomes a permanent test.

- Grows automatically with use
- Free (episodes already stored)
- Directly counters catastrophic forgetting
- Makes "did we get better?" answerable

### P4.4 - Compression and Forgetting (section 28)

- Unique episodes: retain in full
- Repeated episodes: abstract the pattern, keep first, last, and every
  failure verbatim
- Superseded episodes: demote, summary retained, detail archived
- Failures: NEVER compressed away
- Compression requires extraction first (learn from it before compressing)
- What's forgotten is recorded as forgotten (a known gap, not a silent hole)

### P4.5 - Retirement

- A skill is a candidate when a newer skill subsumes it
- Retirement is not deletion: deprecated, reconstructible, removed from
  active ranking
- Stops costing search time without losing knowledge

### P4 Exit Criteria

- Skills discovered from repetition and single successes
- Promotion gate rejects bad abstractions, promotes good ones
- Regression suite growing automatically
- Metric 1 (compounding): cost of Nth skill declining with N
- Metric 2 (transfer): learning in domain A improves unseen domain B
- Metric 11 (abstraction survival): promoted abstractions stay in use

---

## Phase 5: Curiosity + Structural Self-Modification

**Maps to**: Section 33, Stages 5-6
**Goal**: Self-directed learning. Structural change gated by a regression
suite now large enough to make degradation visible.

### P5.1 - Curiosity and Value Model (section 29)

Gap detection:
- Structural gaps (expected relation missing)
- Functional gaps (can do X but don't know why)
- Repeated impasses (stuck at same subgoal N times)
- Held contradictions (section 23)
- Failed predictions (world surprised the system)
- Distance from grounding (beliefs supported only by beliefs)

Value ranking:
- Expected blast radius (how much depends on this?)
- Relevance to goals
- Learning progress (rate of improvement, not difficulty)
- Cost to close

### P5.2 - Goal System (section 29)

- Task goals: supplied externally
- Standing goals: supplied externally, persistent
- Instrumental subgoals: derived, in service of above
- Learning goals: from gaps, ranked by value, traceable to standing goals
- Standing goals are immutable (normative mutability class)

### P5.3 - Capability Acquisition (section 32)

Give Spoon a minimal native substrate from which it can acquire richer tools:

- Native primitives are fixed, typed, and policy-enforced: scoped network
  request, scoped file read/write, observation, and sandboxed execution.
- Every primitive invocation declares effects, resource limits, permission
  requirements, redaction rules, and replayability. There is no ambient file,
  network, process, environment, or secret access.
- Interface discovery may inspect user-authorized API descriptions, schemas,
  command help, fixtures, or observed request/response examples. Discovery is
  an episode with provenance; it does not itself grant permission to call the
  discovered interface.
- Synthesis produces neutral typed procedures, input/output schemas, contracts,
  dependency pins, effect summaries, permission requirements, and tests.
- Candidate capabilities run in a sandbox against mocks or explicitly granted
  test targets. Promotion uses the Phase 4 gate and requires local contract,
  permission, regression, and effect checks.
- Capability execution resolves permissions at invocation time. Grants are
  scoped, revocable local objects; procedures contain permission requirements,
  never credentials.
- Failures preserve traces and feed normal credit assignment, contract
  refinement, repair, and retirement.

### P5.4 - Reconstructible Capability Bundles (sections 32 and 34)

Define a versioned, canonical bundle format containing:

- Manifest, stable capability identity, procedures in neutral IR, dependency
  DAG and exact versions/content hashes
- Contracts, typed schemas, permission/effect declarations, resource bounds,
  tests and fixtures, reconstruction recipe, compatibility constraints
- Provenance for authorship/discovery, source-interface fingerprints, build
  steps, validation episodes, and exported evidence references
- No secret values, bearer tokens, cookies, raw environment variables,
  machine-local paths, ambient grants, or local trust receipts

Export is deterministic and content-addressed. Import verifies structure,
hashes, dependency closure, schema compatibility, prohibited effects, and
resource bounds before storing the entire bundle in quarantine. Imported
concepts and procedures are always Provisional, imported evidence is historical
provenance rather than local authorization, and imported permission
requirements remain unsatisfied until the local operator grants them. Local
tests and locally grounded observations must revalidate the capability before
the ordinary promotion gate may activate it. Failed imports and failed
revalidations remain inspectable and cannot partially mutate the active graph.

### P5.5 - Structural Self-Modification (section 33, stage 6)

Attempted last, deliberately. The biggest gains and only truly
catastrophic failures live here.

- Reorganize concepts and search policies
- Gated by regression suite now large enough to make degradation visible
- Two rates of change enforced (section 25):
  - Fast (during use): local, additive, safe mid-conversation
  - Slow (offline): global, restructuring, unsafe mid-thought

### P5 Exit Criteria

- Spoon directs its own learning within goal boundaries
- Spoon can discover an authorized interface and synthesize a typed, contracted,
  sandbox-tested capability from the native primitives
- Exported bundles reconstruct the same dependency DAG and tests on a clean
  instance; deterministic re-export has the same content identity
- Imported capabilities cannot execute before local permission resolution and
  revalidation, and cannot inherit trust or secrets from the exporter
- Malformed, incomplete, over-permissioned, secret-bearing, or dependency-
  conflicting bundles fail atomically and remain outside the active graph
- Structural changes pass regression suite
- Goals remain immutable
- Metric 3 (weaning): teacher calls declining per domain
- Metric 9 (teacher ablation): competence survives disconnection

---

## Phase 6: Inspector + Metrics Dashboard

**Maps to**: Section 38
**Goal**: Make the flywheel measurable and the system inspectable.

### P6.1 - Web Inspector (`@spoon/inspector`)

- Knowledge graph visualization (concepts, relationships, procedures)
- Episode browser (search, filter, replay)
- Procedure inspector (contract, test cases, version history)
- Contradiction viewer
- Dependency graph (what depends on what)
- Human-readable episode narrative (“What happened?” view):
  - the original request and the escalation path taken;
  - whether a teacher was used, which provider/model/source answered, and the
    teacher's structured proposal summarized in plain language;
  - validation status, rejected/provisional/verified checks, and the exact
    reason a proposal was accepted, retried, or discarded;
  - what Spoon learned (new concept/procedure/contract/test), what it deliberately
    did not learn, and the provenance episode that supports each change;
  - execution steps, contract checks, observed result, evaluation tier,
    confidence/surprise, cost, and the reason for any abstention;
  - capability permissions/effects and local revalidation status when a
    capability participated.
- Narrative rendering is a read-only projection over immutable episode,
  teacher-interaction, trust, and provenance records. It must redact secrets,
  bearer tokens, cookies, raw environment values, and sensitive payloads, and
  must retain a raw-JSON drill-down for forensic inspection rather than
  replacing the underlying evidence.
- CLI parity: `ask --explain` (or an equivalent human-readable episode command)
  should print the same bounded narrative without requiring the web inspector.

### P6.2 - Section 38 Metrics Dashboard

The twelve metrics, tracked continuously:

1. **Compounding**: cost of Nth skill vs N (THE most important number)
2. **Transfer**: learning in domain A improves unseen domain B
3. **Per-domain weaning**: teacher calls on Nth novel task vs 1st
4. **Trace compression**: steps needed for repeated task family
5. **Rung distribution**: which escalation rung resolves problems
6. **No regression**: verified episodes still pass after new structure
7. **Attribution accuracy**: on injected faults, does credit find the culprit?
8. **Attribution cost**: as fraction of total cost, as traces lengthen
9. **Teacher ablation**: disconnect teacher, re-run task history
10. **Grounding drift**: fraction of beliefs traced to external evidence
11. **Abstraction survival**: promoted abstractions still in use later
12. **Calibration**: when Spoon says .9, is it right ~90%?

Anti-gaming rules (section 38):
- Held-out task families for transfer measurement
- Novel tasks only (caching is not capability)
- Report abstentions separately
- Publish the failures

### P6 Exit Criteria

- All 12 metrics tracked and visualized
- Anti-gaming rules enforced
- The flywheel is either turning or visibly not turning
- Can make the honest call: "the thesis is alive" or "the thesis is dead"

---

## Phase 7: Hierarchical Configuration + Episodic Sessions

**Status**: In progress — the first public vertical slice is implemented
**Maps to**: Sections 14 and 18 plus the product/configuration boundary needed
to expose them safely
**Goal**: Give users predictable project-local behavior and human-like global
episodic continuity while preserving explicit, testable isolation when a task
or benchmark requires it.

This phase is a follow-on integration layer over the completed engine. It does
not replace graph knowledge, episode provenance, capability grants, or the
Phase 3 recall/ranking machinery. It supplies a single resolved runtime policy
to all of them.

### P7.1 - Versioned Hierarchical Configuration

**Implementation status**: Implemented in the CLI: strict v1 resolution,
source/shadow diagnostics, schema publication, environment projection, safe
path validation, atomic layer writes, and redacted receipts are live.

Add one typed configuration model shared by CLI startup, public server launch,
benchmarks, and the SDK's local-process helper. Configuration files are strict
JSON with a checked `version` field and a published JSON Schema. Unknown keys,
invalid durations, invalid paths, and incompatible versions fail with a useful
source location rather than being silently ignored.

Resolution order, from lowest to highest precedence:

1. Built-in safe defaults.
2. User defaults in `~/.spoon/config.json`.
3. `.spoon/config.json` files in ancestor directories, applied shallowest to
   deepest and ending at the process working directory. The home config is
   de-duplicated if it is also encountered as an ancestor.
4. The nearest `.spoon/config.local.json`, intended for uncommitted
   machine-specific values and ignored by Git.
5. `SPOON_*` environment variables.
6. Explicit CLI flags or equivalent per-call SDK options.

Merge and path rules:

- Objects deep-merge, scalars override, and arrays replace rather than append.
- `null` clears a parent value only where the schema explicitly permits it.
- Relative paths resolve against the directory containing the file that
  declared them, not against whichever directory later launches Spoon.
- Symlinks are canonicalized for source identity and cycle/duplicate detection.
- A resolved config includes source metadata internally so every effective
  value can explain where it came from.
- Runtime limits use a safety lattice: a child config may reduce a hard cap but
  may not exceed a stricter parent/admin cap.

Configuration trust classes are deliberately separate:

- Portable repository config may choose recall behavior, ordinary budgets,
  provider/model names, output style, benchmark defaults, and requested
  capability requirements. It may also further restrict effects.
- User-home config may contain machine paths and mappings to locally installed
  teacher adapters, subject to local policy. Repository-adjacent local config
  remains subject to the project path and authority boundaries.
- Secrets remain in environment variables or a future secret store. They are
  never accepted from committed config and never appear in `config show`.
- Capability grants remain revocable local/admin records. No repository-
  controlled config file can self-grant file, network, observation, sandbox,
  process, environment, or secret access. A user-home full-access policy is an
  explicit local-operator decision, never inherited from a project. Authority
  is checked again at invocation time.

Initial public shape:

```json
{
  "$schema": "https://spoon.dev/schemas/config-v1.json",
  "version": 1,
  "database": { "path": ".spoon/spoon.sqlite" },
  "teacher": { "provider": "codex", "model": null },
  "capabilities": { "permissionMode": "ask" },
  "recall": {
    "mode": "global",
    "lookback": "90d",
    "maxEpisodes": 64
  },
  "output": { "mode": "explain" }
}
```

Public diagnostics:

```text
spoon config path
spoon config show
spoon config show --sources
spoon config validate
```

`config show` renders a redacted effective configuration. `--sources` annotates
each value with its winning file/environment/flag source and reports shadowed
values. `validate` checks every discovered layer and the merged result without
starting the engine or contacting a teacher.

**Deliverable**: Running Spoon from any directory produces one deterministic,
explainable, schema-valid configuration; relocating a project does not break
paths declared relative to that project; repository config cannot acquire
authority or expose a secret.

Portable project paths are confined to the project tree by default. A path
outside it requires an explicit user-home, environment, or CLI override; merely
checking out and entering a repository must not make Spoon open or create an
arbitrary machine path.

### P7.2 - Native Configuration and Permission Administration

**Implementation status**: The deterministic local administration slice is
live for teacher enablement, permission mode, recall mode, and database path;
it writes user-layer changes and redacted receipts. Generic typed permission
grant UX and interactive confirmation remain follow-on hardening.

Configuration and permission management are built-in Spoon administration
operations, not learned procedures and not general file-access primitives.
They are fixed, typed, non-exportable, non-retirable, and unavailable to
imported capabilities. This lets a user manage Spoon naturally through
`spoon ask` or `spoon chat` without giving the engine arbitrary write access to
its own policy files.

The native administration surface exposes narrow operations:

- Inspect the supported config schema and effective redacted configuration.
- Explain a setting, its allowed values, its winning source, and what a change
  would affect.
- Propose a typed JSON Patch against a named config layer, validate the
  resulting layer and effective configuration, and show the redacted diff.
- Apply an authorized patch atomically with locking, a recovery copy, config
  version checks, and an immutable administration receipt.
- Inspect capability requirements, local grants, denials, scope, expiry, and
  provenance; request, narrowly grant, renew, or revoke a local permission.
- Report whether a setting applies immediately, on the next cycle, or only
  after the server/database is restarted.

The effective configuration includes capability *policy*, but distinguishes
three sources of authority:

1. Project config may declare required permissions and may deny or narrow
   effects. These are requests/constraints, never grants.
2. User-home config may define approval defaults, hard ceilings, and mandatory
   denials. It still does not contain bearer tokens or transferable grants.
3. Actual grants are scoped, revocable objects in Spoon's local authority
   store, bound to a capability identity/content hash, exact effects/resources,
   optional expiry, granting actor, and audit receipt. They appear in
   `config show --sources` as a separate local-authority source but are never
   serialized into portable project config or capability bundles.

Natural-language management uses the ordinary interpretation path to produce a
typed administration intent, then crosses a separate authorization gate. A
teacher may help map “turn the teacher off,” “use this database next time,” or
“allow this weather capability to call api.example.com” into a proposed patch,
but the teacher's text is untrusted input and can neither execute the patch nor
authorize it. Common administration intents should also have a deterministic
local interpretation path so turning off or changing the teacher does not
itself require a working teacher.

Effect classes determine interaction behavior:

- Authority-reducing changes, such as disabling the teacher, revoking a grant,
  lowering a budget, or changing recall to `none`, may be applied from the
  user's explicit chat request and return a receipt.
- Reversible behavior changes, such as output mode, lookback, or global/session
  recall defaults, apply to the appropriate layer and report their effective
  time.
- Authority-expanding or context-switching changes, including new capability
  grants, broader network/file scopes, arbitrary machine paths, teacher command
  mappings, or a database-pointer change, require an explicit redacted diff and
  local operator confirmation. Non-interactive use requires a separately
  authenticated admin mechanism; a teacher, imported bundle, benchmark, or
  ordinary capability can never confirm on the user's behalf.

Capability permission mode is a first-class user preference with three levels:

- `ask` (default): declared capability requirements resolve against explicit
  local grants, and missing authority produces a bounded approval request.
- `workspace`: automatically authorize declared file read/write and sandboxed
  execution effects inside the resolved project root for the active workspace.
  External paths, network hosts, secret-store references, and broader effects
  still require grants.
- `full-access`: automatically satisfy declared native capability permission
  requirements within the operating-system authority of the Spoon process,
  without per-capability approval prompts. It may be enabled for one chat/
  process or persisted in user-home config.

`full-access` is intentionally comparable to other coding agents' bypass/full
access modes: the user opts in once instead of approving every file, host, or
sandbox request. It bypasses Spoon's routine capability *grant prompts*, but it
does not bypass non-permission invariants:

- A capability must still declare its effects and typed resource requirements;
  undeclared effects are rejected rather than silently allowed.
- Imported capabilities remain quarantined until structural validation and
  local revalidation succeed; full access is not automatic trust or promotion.
- Contract checks, budgets, timeouts, deterministic traces, provenance,
  redacted audit records, and atomic mutation rules remain active.
- Full access cannot raise operating-system privileges, disable the episode
  trail, mutate the administration control plane, confirm its own config/grant
  change, or override a mandatory user/admin denial.
- Dedicated secret-store injection remains separately declared. Full file
  access can nevertheless expose credentials present in readable files, so the
  UI and documentation must state that risk plainly rather than implying that
  redaction is a security boundary.

Only an explicit local user action may enable `workspace` or `full-access`.
Project and imported config may force a *stricter* mode but may never elevate
one. Chat accepts natural requests such as “give capabilities full access for
this chat” or “always use workspace permissions”; persistent full access shows
a single redacted confirmation before activation. The CLI/server/inspector
display a persistent `FULL ACCESS` indicator while it is effective. Revocation
or an emergency `ask` override takes effect before the next primitive
invocation, including during a long-running chat.

Equivalent recovery and automation controls use the same authorization path:

```text
spoon chat --permission-mode workspace
spoon chat --permission-mode full-access
spoon ask --permission-mode full-access "..."
spoon config set capabilities.permissionMode full-access --layer user
spoon permission mode ask|workspace|full-access
```

Non-interactive persistent activation requires an explicit acknowledgement
flag or authenticated admin policy. The acknowledgement suppresses repeated
prompts; it is not repeated for each capability invocation.

Database changes are especially conservative. The administration operation
validates and writes the new pointer but does not hot-swap the database beneath
an active cycle. It reports that the change takes effect on restart, verifies
whether the target is new or existing, and never deletes, moves, or mutates the
previous database as a side effect.

Explicit commands remain available for scripting and recovery, backed by the
same native operations used by chat:

```text
spoon config explain <key>
spoon config set <key> <json-value> [--layer user|project|local]
spoon config unset <key> [--layer user|project|local]
spoon config apply <patch.json> [--dry-run]
spoon permission list
spoon permission grant <capability> <scoped-permission>
spoon permission revoke <grant-id>
```

Every mutation records the requesting user text, normalized intent, target
layer, redacted before/after hashes, exact non-secret patch, authorization
source, result, and effective time. Failed validation and denied authorization
also produce receipts. Secret values are accepted only through a dedicated
secret-reference flow and are never echoed, passed through a teacher, written
to config, or retained in episode text.

**Deliverable**: A user can ask Spoon in ordinary language to explain or safely
change any supported setting and can manage narrowly scoped local capability
permissions. The result is schema-valid, atomic, auditable, source-aware, and
cannot be triggered or authorized by untrusted teacher/capability content.

### P7.3 - Session and Recall Data Model

**Implementation status**: Implemented end to end through SQLite, the engine,
JSON-RPC, SDK, CLI session commands, and global/session/none recall filtering.

Sessions are first-class continuity records, not the default unit of memory.
Add a durable `Session` record with an opaque ID, optional unique human name,
timestamps, lifecycle state, and `Global` or `Isolated` visibility. Add optional
episode metadata for `session_id`, monotonically assigned `turn_index`, and
durable memory visibility. Do not infer isolation from a session name.

Keep storage and retrieval policy distinct:

- `session.visibility = global`: its episodes remain eligible for ordinary
  global recall. When that session is active, same-session episodes receive a
  ranking boost but global evidence remains available.
- `session.visibility = isolated`: its episodes may be recalled only by later
  turns in that same session. They are excluded from global recall, other
  sessions, learning corpora, consolidation, automatic regression promotion,
  and capability evidence unless a future explicit export/promotion operation
  is authorized.
- `recall.mode = global` (default): search all non-isolated episodes, bounded
  by time, score, and item budget, with an active-session continuity boost.
- `recall.mode = session`: recall only the active session. This requires a
  session ID but does not retroactively change the visibility of older rows.
- `recall.mode = none`: assemble no episodic context for this cycle. The new
  episode is still recorded with its configured visibility and provenance.

An isolated session may use a teacher for its current answer, but may not
silently mutate the global graph, ranker, procedures, capability store, or
regression suite. Any reusable lesson it produces remains a provisional
session-local artifact until a future explicit, locally revalidated promotion
operation crosses that boundary. The first implementation may conservatively
disable isolated-session learning if a complete session-local overlay is not
yet available.

Global recall means “eligible retrieval pool,” not “copy all history into the
prompt.” Candidate generation remains bounded and ranked using entity
relevance, learned recall score, same-session continuity, recency, and frequency.
`lookback`, `maxEpisodes`, and the cycle's existing context budget provide hard
limits. The effective item count is the minimum of all applicable caps.

SQLite migration must be additive and backward compatible:

- Existing episodes deserialize with `session_id = null` and global visibility.
- Add indexed query columns for session, visibility, turn, and creation time;
  preserve the immutable historical episode JSON rather than rewriting it.
- New JSON fields use serde defaults so old databases and exported episodes
  remain readable.
- Backfill indexes transactionally and idempotently; interruption must leave a
  database that can be reopened and resumed.
- Episode queries gain explicit visibility/session filters. Internal callers
  must choose a recall policy; they may not accidentally use an unfiltered
  “recent rows” helper for reasoning.

**Deliverable**: Existing databases continue to provide global recall without
migration surprises; session continuity improves ranking; isolated episodes
cannot influence any global reasoning, learning, metric, or promotion path.

### P7.4 - Public Runtime and Human CLI

**Implementation status**: Implemented for `ask`, `chat`, session lifecycle,
recall flags, permission-mode flags, and human-readable configuration/admin
surfaces.

Carry the resolved policy through the same public path a user exercises:
CLI -> SDK -> JSON-RPC server -> `CycleInput` -> context assembly -> episode
recording. The server records the effective non-secret recall policy, config
fingerprint, and optional caller-supplied working-directory path in episode
provenance so an explanation can say why a memory was or was not eligible and
which local repository context was active. The path is metadata only: it never
grants filesystem authority, transfers with capability bundles, or substitutes
for an explicit scoped host permission.

Public commands and flags:

```text
spoon chat
spoon chat --session <id-or-name>
spoon session start [--name <name>] [--isolated]
spoon session list
spoon session show <id-or-name>
spoon session end <id-or-name>
spoon ask --session <id-or-name> "..."
spoon ask --session <id-or-name> --recall session "..."
spoon ask --recall none "..."
```

`spoon chat` is the simple human interface: it starts or resumes a session,
prints concise answers by default, and offers the existing explain view on
demand. A normal chat session remains globally recallable. `--isolated` is
visibly marked in the prompt/header and cannot be toggled off for that session
after episodes exist; avoiding a leaky boundary is more important than
convenience.

The SDK exposes the same typed `Session`, `SessionVisibility`, `RecallMode`, and
`RecallPolicy` values. RPC methods cover session lifecycle and filtered episode
views. Inspector and CLI explanations display the active session, recall mode,
lookback/cap, candidate counts by source, exclusions caused by isolation, and
the selected memories without exposing redacted content. Isolated sessions are
absent from default/global listings and metrics, but remain inspectable through
an explicit request for that session by the local operator.

**Deliverable**: A user can chat naturally across process restarts, inspect
what Spoon remembered and why, deliberately run without episodic recall, or
create a provably isolated conversation without changing databases manually.

### P7.5 - Conversational Public Benchmarks

**Implementation status**: Ordered conversational fixtures are validated and
executed through public `session start` and `ask` subprocesses, with separate
Teacher-ON acquisition and Teacher-OFF retention sessions and per-turn reports.

Extend the benchmark JSON Schema with ordered conversations while retaining
the existing single-prompt format. A case may contain setup variables and a
sequence of turns, each with a user prompt, optional expected answer/regex,
disposition requirements, teacher-use requirements, and memory assertions.
Later prompts may interpolate captured public outputs but may not query engine
internals to construct an answer.

Conversational acquisition/retention follows the same public-entrypoint rule:

1. Teacher ON conversation in a fresh named session.
2. Teacher OFF exact replay in a new process against the same database, using
   either the same global memory pool or an explicitly declared session mode.
3. Teacher OFF paraphrased conversation after exact replay passes.
4. Isolation probes run a control conversation outside the isolated session
   and must demonstrate non-recall, not merely a different answer.

Reports group results by conversation, pass/fail each turn, identify the first
memory-dependent failure, and show teacher calls, learned/reused artifacts,
episode IDs, session IDs, recall policy, and skipped downstream gates. Fixture
validation rejects ambiguous cases such as `recall: session` without a session
or an isolation assertion that reuses the isolated session.

**Deliverable**: Multi-turn learning, durable conversational continuity,
teacher weaning, paraphrase transfer, and isolation non-leakage are all tested
through `spoon ask`/`spoon chat` rather than direct engine answer APIs.

### P7.6 - Safety, Migration, and Usability Gate

**Implementation status**: Core migration, isolation, permission-mode, and
CLI regression coverage is green; the broader adversarial matrix remains the
remaining Phase 7 hardening gate.

Required test matrix:

- Precedence tests for defaults, home, multiple parents, local override, env,
  and flags; source annotations must identify the winner.
- Relocation, symlink, malformed JSON, unknown key, unsupported version,
  invalid duration, and partial-file tests.
- Security tests proving repository config cannot grant capabilities, increase
  hard limits, escape the project with a machine path, select an arbitrary
  executable, inject a secret, or reveal a redacted value.
- Administration tests proving schema-only typed patching, atomic recovery,
  source-correct edits, redacted receipts, and correct immediate/restart
  behavior. Teacher responses, imported capabilities, benchmark prompts, and
  recalled episode text must all fail to authorize a mutation or grant.
- Permission-mode tests covering `ask`, workspace path containment, full-access
  prompt bypass, one-chat versus persistent scope, visible mode indicators,
  immediate emergency downgrade, mandatory denials, and rejection of project-
  supplied elevation. Full access must not bypass undeclared-effect, quarantine,
  validation, budget, provenance, control-plane, or operating-system limits.
- Natural-language tests for deterministic local intents and teacher-assisted
  interpretation, covering config explanation, teacher disablement, recall
  changes, database-pointer proposals, narrow permission grants, and revocation.
- Old-database migration, interrupted migration/reopen, mixed old/new episode,
  and deterministic turn-order tests.
- Global-default, same-session boost, session-only, recall-none, lookback, and
  budget-bound retrieval tests.
- Adversarial cross-session tests across context assembly, learned ranking,
  self-supervision, consolidation, regression promotion, metrics, inspector,
  export, and capability provenance. Isolated content must not affect any
  global output, count, score, or learned artifact.
- End-to-end public CLI tests spanning fresh processes and conversational
  benchmark acquisition/exact-retention/paraphrase gates.

Human documentation is part of the gate: root, CLI, SDK, server, episode,
reason, benchmark, and inspector READMEs explain configuration precedence,
global memory, isolation, recall-off semantics, privacy limitations, migration,
and copy-paste examples. Documentation must explicitly say that isolation is a
retrieval/learning boundary inside a local Spoon instance, not encryption or an
operating-system security boundary.

### P7 Exit Criteria

- The effective configuration is deterministic, redacted, explainable, and
  identical across CLI/server/benchmark local launches.
- Project configuration cannot create authority, carry secrets, or relax local
  safety policy.
- Through `ask`/`chat`, Spoon can explain and update every supported setting
  through schema-bound native operations, with an auditable receipt and no
  general-purpose access to its policy files.
- Permission requests/constraints are visible in effective config, while
  actual grants remain local, scoped, revocable, identity-bound, and impossible
  for a teacher or capability to authorize.
- Workspace and full-access modes remove repetitive capability prompts when the
  local user opts in, remain visibly active, and can be downgraded immediately;
  no repository, teacher, imported bundle, or learned procedure can enable or
  preserve the elevated mode.
- Normal sessions retain global episodic continuity across restarts.
- Recall remains bounded; “global” never means unbounded prompt injection.
- Recall can be disabled without suppressing episode recording.
- Isolated session data produces zero influence outside its session across
  reasoning, learning, promotion, global metrics/default inspection, and
  export paths; explicit local inspection of that session remains possible.
- Multi-turn teacher ON -> teacher OFF exact -> teacher OFF paraphrase suites
  run through public user entrypoints and report per-turn retention clearly.
- Existing databases migrate without rewriting or losing historical episodes.

---

## Open Problems (from Appendix D)

Ranked by likelihood of sinking the project:

1. Credit assignment cost at scale (P2.3)
2. Non-replayable steps (P2.3)
3. Joint responsibility / interaction effects (P2.3)
4. Gate calibration - too strict or too loose (P4.2)
5. Correction scope - how broadly to fix (P2.4)
6. Open-ended tasks resisting decomposition (P1.4)
7. Recall without scanning everything (P3.1)
8. Grounding ratio - how much self-supervision per grounded check (P3.4)
9. Contract acquisition for new procedures (P2.4)
10. Discriminating feature discovery for scope refinement (P2.6)
11. Safe interface discovery under adversarial schemas and responses (P5.3)
12. Portable reconstruction across runtime and environment drift (P5.4)
13. Recall contamination across long-lived global histories (P7.3)
14. Session isolation across indirect learning and metric side channels (P7.6)
15. Hierarchical config provenance and authority composition (P7.1-P7.2)

These are where the experiment produces information regardless of outcome.

---

## Suggested Build Order

```
P0.1 -> P0.2 -> P0.3 -> P0.4 -> P0.5 -> P0.6    (seed, ~4-6 weeks)
                                           |
P1.1 -> P1.2 -> P1.3 -> P1.4                      (interpretation, ~3-4 weeks)
                          |
P2.1 -> P2.2 -> P2.3 -> P2.4 -> P2.5 -> P2.6     (credit, ~4-6 weeks)
                                          |
P3.1 -> P3.2 -> P3.3 -> P3.4                      (intuition, ~4-6 weeks)
                          |
P4.1 -> P4.2 -> P4.3 -> P4.4 -> P4.5              (consolidation, ~3-4 weeks)
                                  |
P5.1 -> P5.2 -> P5.3 -> P5.4 -> P5.5               (curiosity + capability,
                                  |                  ~5-8 weeks)
P6.1 -> P6.2                                       (inspector, ~2-3 weeks)
                          |
P7.1 -> P7.2 -> P7.3 -> P7.4 -> P7.5 -> P7.6       (configuration + sessions,
                                                     ~3-5 weeks)
```

P6 (inspector) can start alongside P1 and grow incrementally - you'll want
to see the graph and episodes early. The timeline above is sequential
solo-developer pace.

Total rough estimate: 6-9 months to a working system that can measure
whether the flywheel turns. This is aggressive but not insane.

Phase 7 is a follow-on estimate of roughly 3-5 weeks at solo-developer pace.
Its safe order is resolver/schema first, native administration and authorization
second, session storage third, public wiring fourth, conversational benchmarks
fifth, and the cross-cutting leakage audit last. Once the shared config/session
wire types are fixed, P7.2 administration fixtures and P7.3's Rust migration
can proceed in parallel, but P7.4 must integrate both before P7.5 begins.

## What Success Looks Like

Metric 1 (compounding) slopes downward. Each new skill makes the next
cheaper to acquire. If it's flat after Phase 4, the thesis is dead and
the honest thing to do is say so publicly.

The second-best outcome is that credit assignment works but costs too
much at scale - that's a tractable engineering problem, not a
fundamental one.

The worst outcome is not being wrong. It's being unable to tell.
