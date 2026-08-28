# Spoon Language Implementation

Status: M0 core contracts, the real Ollama-backed CLI vertical slice, and the
first hybrid typed routing catalog are implemented. Durable semantic Intent
Catalog storage, directly selectable imported capability procedures,
supplemental context, and the user clarification continuation remain M1/M2
work.

## Objective

Make Spoon's typed language structures part of the real cognitive cycle. A
small, replaceable language interpreter should map user text to bounded intent
hypotheses, improve retrieval, and preserve ambiguity. The Engine remains the
authority for concept identity, procedure selection, execution, evidence, and
response claims.

The first model backend uses Ollama only to validate that a learned interpreter
improves usability. Ollama is not a permanent runtime requirement. After the
model passes held-out evaluation, Spoon will load a verified local model
artifact through a repository-owned Rust interface.

## Current Reality

The repository already has:

- deterministic UTF-8 tokenization with byte-accurate spans;
- `IntentFrame`, `IntentSlot`, `IntentScope`, ambiguity strings, and confidence;
- deterministic procedures, contracts, budgets, traces, and replay;
- evidence-linked `ResponsePlan` values and a bounded renderer;
- lexical/co-occurrence retrieval and an outcome-trained local ranker;
- Teacher transports with structured-output support, including Ollama;
- a declared language-kernel curriculum.
- normal-cycle `IntentFrameSet` grounding and execute/clarify/abstain handling;
- request-local procedure aliases resolved to exact captured IR revisions;
- hybrid lexical/local-semantic procedure ranking before bounded truncation;
- a model-facing typed catalog of selectable procedures, their used pure
  primitives, and non-selectable native capability boundaries;
- candidate-specific structured-output schemas and literal-range ordering.

The repository does not yet have:

- a durable catalog mapping semantic intent keys and slot schemas to existing
  concepts and exact procedure versions;
- a semantic interpreter, entity/reference resolver, or clarification policy;
- an Engine-created response plan in the normal cycle;
- an executable language curriculum/data forge;
- a trained language-interpreter artifact or embedded model runner.

Procedure inputs now carry a persisted `ParamType` boundary (`number`,
`text`, `list`, `map`, etc.). Runtime binding enforces that boundary for both
direct calls and interpreter-routed calls. Existing procedures are upgraded on
database open: untyped slots are inferred from their descriptions when
possible and become explicit `any` when the legacy evidence is ambiguous. The
procedure ID and behavior are retained; the upgrade is recorded as a new
procedure revision so old capabilities do not disappear.

The older `local_interpretations` path is retrieval, not parsing. The optional
Language Interpreter now receives Engine-filtered candidates first. Small
procedure sets are preserved; oversized sets are ranked through the existing
lexical/local-semantic intuition index before truncation. The returned alias
and grounded arguments are still validated against the exact stored procedure
revision before execution.

## Naming and Boundaries

Use these names consistently:

- **Language Interpreter**: inbound fuzzy component that proposes meaning.
- **Interpretation Proposal**: untrusted model output at the provider boundary.
- **Intent Frame Set**: validated competing `IntentFrame` values plus the
  execute/clarify/abstain disposition.
- **Intent Catalog**: inspectable Engine data mapping semantic intent keys and
  slot schemas to local concepts and exact procedures.
- **Language Realizer**: future outbound component that may improve wording
  without authoring claims.
- **Response Plan**: Engine-owned, evidence-linked content to render.

Do not call the interpreter the intuition model. Intuition covers retrieval,
ranking, and usefulness learned from episodes; language interpretation is one
consumer and producer of intuition signals.

## Target Flow

```text
user utterance
  -> deterministic tokenization
  -> Engine-built bounded Language Context Packet
       -> same-session recent turns
       -> request-local catalog/search candidates
       -> safe assumptions and environment projection
  -> Language Interpreter
       -> competing intent frames
       -> grounded slots and references
       -> ambiguity / clarification needs
       -> bounded retrieval hints
       -> optional bounded supplemental-context request
  -> structural, span, and catalog validation
  -> Intent Catalog resolution
       -> exact concept
       -> exact procedure version
       -> named inputs
  -> contract-checked deterministic execution
  -> Engine-created evidence-linked Response Plan
  -> deterministic renderer
```

Unknown or invalid interpretations fall through to bounded retrieval, Teacher
escalation, clarification, or abstention according to policy. Model output
never creates knowledge, grants authority, executes a capability, or authors a
result claim.

### Context and clarification handshake

The interpreter should not work from the current sentence alone. Spoon already
owns the knowledge graph, session history, privacy rules, recall policy, and
retrieval budgets, so the Engine must decide what context is eligible and
project it into a small read-only `LanguageContextPacket` before inference.

The packet contains only bounded, redacted, request-local material:

- the current tokenized utterance;
- a small same-session dialogue window, using summaries rather than raw traces;
- relevant Intent Catalog entries and retrieved concepts/procedures represented
  by request-local aliases rather than durable database IDs;
- known terminology/aliases and their scope;
- safe assumptions and selected environment facts, never secrets or authority;
- explicit truncation flags, budgets, and provenance for every context group.

The first interpreter result may be a final `InterpretationProposal` or one
typed supplemental-context request. The request is an allowlisted operation
such as details for already-surfaced candidate aliases, a bounded earlier-turn
window, or terminology for a grounded source phrase. It cannot contain SQL,
arbitrary graph queries, paths, durable IDs, or capability requests. The Engine
may answer once within budget, then requires execute/clarify/abstain. This gives
the model useful back-and-forth without turning it into an unbounded database
agent.

User clarification is a separate loop. If material ambiguity remains, the
Engine emits a bounded question derived from the competing frames. The user's
reply becomes a new session turn linked to the pending interpretation. Spoon
then re-runs interpretation with both turns; the model never silently chooses
an ambiguity merely because one candidate has a slightly higher score.

## Language IR

### Trusted core values

`IntentFrame` remains the unit for one proposed meaning. Add an
`IntentFrameSet` that preserves multiple hypotheses and records a disposition:

```text
IntentFrameSet
  candidates: [IntentFrame]
  selected: optional candidate index
  disposition: Execute | Clarify | Abstain
```

Core validation must enforce:

- bounded candidate and slot counts;
- finite confidence values from zero through one;
- source spans that are valid UTF-8 boundaries in the exact current document;
- selected indices that are in range;
- `Execute` has one selected candidate and no unresolved ambiguity;
- `Clarify` does not select a candidate and exposes an ambiguity;
- `Abstain` does not select a candidate;
- all names and text remain within language limits.

The model-facing proposal should refer to deterministic token indices. The
trusted boundary converts them to byte spans and validates that slot literals
match the referenced source. The model should not be asked to count UTF-8 byte
positions.

### Intent catalog

The catalog is versioned data rather than model weights or Rust match arms:

```text
semantic intent key
  -> allowed scope
  -> required and optional slot schemas
  -> material ambiguity rules
  -> local concept identity
  -> exact procedure identity/version
  -> parameter binding
```

Training artifacts use stable semantic keys such as
`text.count_occurrences`, never database UUIDs. Each Spoon instance resolves a
semantic key through its locally validated catalog and knowledge graph.

## Search Integration

The first integrated router is now live. Procedure recall documents include
concept language and real IR bodies, so synonyms present in descriptions and
locally learned co-occurrence terms can retrieve the right procedure. Exact-IR
duplicates are collapsed, inactive graph objects are filtered, and ranking is
candidate generation only. The model sees typed `procedures`, `primitives`,
and `capabilities`; only captured procedure aliases are directly selectable.
Native capability entries remain descriptive until an exact locally validated
capability procedure, permission policy, and adapter can be captured in the
same request-local binding scheme.

The interpreter improves search in two ways:

1. A known intent bypasses fuzzy concept search by resolving through the
   Intent Catalog.
2. An unknown or partial intent can supply bounded retrieval hints such as a
   semantic key, canonical terms, or request-local candidate rankings.

Hints affect candidate generation/ranking only. They do not become aliases,
concepts, or evidence automatically.

Stable names and aliases should also become inspectable language associations
in the graph, with provenance, language/domain scope, lifecycle, and evidence.
The database stores durable terminology and corrections; model weights provide
generalization across phrasing. Spoon should not materialize every recognized
paraphrase as a graph fact.

For candidate reranking, present a bounded list using request-local aliases
such as `candidate_0`. The interpreter may rank those aliases but never emits
or invents durable graph IDs.

### Declarative knowledge is a separate missing path

Language interpretation cannot by itself make a Teacher-supplied world fact
trusted or durable. The current Engine admits executable reusable lessons, but
an `external_observation` such as “Pierre is the capital of South Dakota” is a
one-off provisional answer and is intentionally not learned.

Spoon needs a separate declarative fact-admission path: a typed predicate/value
claim with subject, scope/effective time, source provenance, uncertainty, and
verification state. It may be stored provisionally, independently corroborated
or authenticated, and only then promoted for semantic retrieval. These facts
must not be disguised as constant procedures or treated as verified merely
because a Teacher stated them.

### Codex Teacher authoring boundary

The canonical `pure_expr_v2` lesson schema is recursive, but Codex CLI's
structured-output endpoint cannot accept that recursive grammar. The Codex
Teacher therefore uses a separate strict, non-recursive
`spoon_flat_expr_v1` wire format for Spoon lesson proposals.

Each expression is a flat node graph with stable local node IDs. For example,
an indexed field lookup has distinct `parameter`, `literal`, `index`, and
`field` nodes. The output schema requires `index.collection` and
`index.index`, and `field.object` and `field.field`; unsupported keys such as
`target` are rejected at the provider boundary. The adapter rejects duplicate,
unknown, and cyclic node references, expands valid graphs into canonical
`pure_expr_v2`, and then passes them through the ordinary proposal validator
and Engine compiler. This is an authoring transport only—the Engine still owns
knowledge admission and execution.

The initial flat wire deliberately requires empty contract arrays to keep the
provider schema compact and accepted by Codex. Spoon independently executes
the supplied invocation and compares the observed result with the claimed
answer before learning. Flat contract graphs are a follow-up extension, not a
reason to weaken the existing runtime contract model.

This is preferable to an opaque escaped-JSON envelope: provider-side schema
enforcement now catches incorrect AST keys before a model response is returned.
Non-Spoon recursive schemas retain the generic JSON-envelope fallback.

## Response Generation

The initial language model is inbound-only:

```text
natural language -> IntentFrameSet       Language Interpreter
execution/evidence -> ResponsePlan       Engine
ResponsePlan -> natural language         deterministic renderer
```

The Engine must construct response claims from execution outcomes, observations,
and verified evidence references. The existing standalone `language.render`
endpoint remains an untrusted caller-supplied rendering endpoint.

A future Language Realizer may choose templates, ordering, tone, and formatting.
Open-ended prose generation is not admitted until a grounding benchmark proves
claim and uncertainty preservation. The realizer never receives authority to
add facts or effects.

## Runtime Strategy

### Validation backend: Ollama

Use an `OllamaLanguageInterpreter` first because it supplies a cheap local
structured-output experiment. It implements the same logical interface as the
future embedded runner. Missing Ollama is a recoverable backend-unavailable
condition; Spoon continues through configured retrieval, Teacher, or abstention
paths.

Example development configuration:

```json
{
  "language": {
    "interpreter": {
      "backend": "ollama",
      "model": "spoon-language-interpreter:dev"
    }
  }
}
```

### FUTURE: embedded Rust runner

After a trained artifact passes the promotion gate, add a
`spoon-language-model` crate with a repository-owned interface:

```text
LanguageInterpreter
  interpret(request) -> InterpretationProposal
  artifact() -> verified ModelArtifact metadata
```

The crate owns:

- artifact and tokenizer loading;
- model/tokenizer/schema fingerprints;
- bounded context and output limits;
- deterministic sampling configuration;
- grammar-constrained structured output;
- cancellation, time, memory, and concurrency limits;
- redacted inference telemetry;
- conversion into the same untrusted proposal validated by the Engine.

Backend bakeoff:

- `llama-cpp-2`: pragmatic GGUF/acceleration option behind a narrow wrapper;
- Candle: more Rust-native, with quantized Qwen2/GGUF support but more decoding
  and schema machinery for Spoon to own;
- mistral.rs: batteries-included Rust inference with a larger dependency and
  feature surface.

The likely first implementation is `llama-cpp-2`, subject to a focused spike
covering Qwen artifact compatibility, Metal and CPU packaging, JSON grammar
enforcement, cancellation, startup latency, and release-binary size.

The model is a separately versioned installable artifact:

```text
spoon model install language-interpreter-v1
spoon model verify language-interpreter-v1
```

Its manifest contains the base-model revision and license, tokenizer and chat
template hashes, dataset hash, adapter/fused-model hash, quantization, language
schema version, evaluation-report hash, and compatibility requirements. Once
installed, the embedded backend operates offline without an Ollama daemon.

## Training Pipeline

Training is an offline, reproducible workflow. Ordinary Spoon startup and use
never require a Python environment and never update weights online.

```text
semantic scenarios
  -> deterministic values and canonical IR
  -> batched Teacher surface generation
  -> validation, alignment, and deduplication
  -> family-aware train/validation/test split
  -> prompt-only baselines
  -> LoRA training
  -> held-out and adversarial evaluation
  -> merge and GGUF conversion
  -> post-quantization evaluation
  -> signed/fingerprinted artifact promotion
```

### Dataset source of truth

Each record retains:

- schema and curriculum version;
- utterance and deterministic token stream;
- expected disposition and competing semantic frames;
- expected slots, ambiguity, and clarification target;
- semantic retrieval target and canonical terms;
- generator/scenario/batch provenance;
- deterministic validation result;
- split family identifiers.

Teacher calls generate 25-50 surface variants for one already-labeled semantic
scenario. The scenario owns the IR label. The Teacher supplies phrasing, not
semantic authority. Reject examples that cannot be aligned to their required
literal slots or violate the scenario.

Add deterministic casing, punctuation, quote, Unicode, noise, and distractor
mutations without additional Teacher calls.

### Split policy

Withhold complete paraphrase templates, semantic families, Unicode structures,
slot-value families, ambiguity types, and Teacher batches. Never use a random
row split that leaks close variants across train and test.

### Training targets

Use related bounded tasks:

- utterance -> intent frame candidates and disposition;
- utterance -> semantic key and canonical retrieval terms;
- utterance plus request-local candidate summaries -> candidate ranking.

Begin with prompt-only Qwen 0.5B and 1.5B baselines. Fine-tune LoRA candidates
only after the baseline report is frozen. Use Hugging Face Transformers/PEFT as
the canonical portable artifact path. MLX-LM may be used for local experiments
when its resulting artifact can be proven equivalent to the release path.

Release conversion:

```text
pinned base + LoRA adapter
  -> merged Hugging Face model
  -> GGUF conversion
  -> selected quantization
  -> full structural/safety evaluation again
```

### Explicit user teaching

User-authored procedures have a separate, opt-in boundary from ordinary
questions. The first public slice is:

```text
spoon teach "given two numbers, add them"
  -> configured Teacher drafts a reusable pure_expr_v2 lesson
  -> Engine compiles the real lesson IR
  -> typed parameters, contracts, dependency and safety boundaries validate
  -> Engine admits a versioned procedure and runs the grounded example
```

`ask` is not silently converted into authoring when a Teacher returns a lesson.
This keeps durable mutation intentional and makes failures visible. The CLI
reports the installed procedure IR and episode audit trail with `--explain`.
Chat exposes the same first slice as `:teach <request>`. A later conversational
wizard can collect the procedure name, inputs/types, examples, expected
behavior, and corrections, then show a final draft before admission. Chat must
call this same Engine path rather than writing procedures directly.

The teaching workflow is still bounded by the existing authority split:

- the user supplies intent, examples, corrections, and approval;
- the Teacher proposes structure and bounded expressions;
- the Engine owns IDs, lifecycle, versions, compilation, validation, and
  persistence;
- capability references are ordinary procedure-body IR nodes. They must point
  at an imported, locally validated capability procedure; the host adapter and
  permission policy are still required at execution time.

Future refinements should add a preview/confirm transaction for chat and a
portable lesson export/import format. Neither should bypass the Engine
compiler or typed procedure boundary.

Effectful teaching now uses the same lesson compiler and `Procedure` type as
pure teaching. `pure_expr_v2` retains its wire name for compatibility, but a
body may contain an explicit `capability_call` referencing the advertised
`contentId` and `procedureId`. Admission checks that the bundle is imported,
locally validated, and contains that exact procedure; it does not grant
permissions. At execution, Spoon re-resolves the capability, applies the
current `ask`, `workspace`, `full-access`, or `god-mode` policy, checks the host adapter,
and records the capability receipt. Contract checks remain pure so validation
cannot trigger an effect. A request such as “spell-check through a Google
search” therefore needs a real imported `web.fetch` bundle, local validation,
host adapter, and runtime permission—but no artificial pure/effectful
procedure split.

### Metrics and promotion gate

Compare:

- current lexical/co-occurrence search;
- prompt-only 0.5B and 1.5B models;
- LoRA 0.5B and 1.5B models;
- exported/quantized release candidates.

Measure exact intent-frame match, intent accuracy, required-slot exactness,
token/span grounding, clarification precision/recall, unsafe execution on
ambiguous cases, retrieval recall@k and reciprocal rank, malformed-output rate,
latency, memory, artifact size, and Teacher escalation rate.

Promotion requires:

- improvement over current search and the prompt-only baseline on frozen
  held-out families;
- zero execution across the material-ambiguity adversarial gate;
- all model output still passing independent Engine validation;
- no regression after merging and quantization;
- equivalent acceptance behavior through Ollama and the candidate embedded
  runner;
- a complete artifact manifest and reproducible evaluation report.

Verified, globally eligible, explicitly permitted episodes may later become
offline training candidates. Isolated/private episodes, unverified outcomes,
Teacher prose, secrets, machine paths, and permission state are excluded.

## Implementation Milestones

### M0 - Core language contract

- [x] Add document-aware `IntentFrame` validation.
- [x] Add bounded `IntentFrameSet` and disposition rules.
- [x] Add serialization, invalid-span, selection, ambiguity, and limit tests.
- [x] Keep the changes isolated from the currently in-flight Engine work.

Exit: the trusted core can represent and validate competing grounded meanings.

### M1 - Intent catalog and Engine boundary

- Add versioned intent/slot/catalog records and storage.
- Add catalog resolution without model involvement.
- Accept an untrusted interpretation proposal and convert token references to
  validated core spans.
- Add the bounded `LanguageContextPacket` projection and a single allowlisted
  supplemental-context round.
- Persist the frame set and validation decision in episodes.

Exit: a hand-authored proposal drives an exact known pure procedure or a safe
clarification/abstention path.

### M2 - Cycle and public protocol

- [x] Add `NeedIntent` and `cycle.resumeIntent` without disrupting
      `NeedTeacher`, using the real Ollama backend.
- [x] Add SDK wire types/client methods and CLI orchestration.
- [x] Add deterministic disabled/backend-unavailable fallback behavior without
      consuming the Teacher budget.
- [x] Expose interpreter source, decision, and context counts in `--explain`.
- Add Inspector visibility for proposal, validation, selection, and fallback.
- Add a normal-user clarification continuation that reuses the current session
  rather than treating the reply as an unrelated request.

Exit: the public CLI can complete an interpreter-assisted cycle using the real
configured Ollama backend and Teacher OFF. There is no shipped fake interpreter
and no synthetic manual-test gate.

This is the first **manual test gate**. It must exercise an actual local model
and include a copy-paste CLI recipe
that exercises success, paraphrase-assisted search, ambiguity/clarification,
abstention, session context, and backend-unavailable fallback. Passing unit
tests alone does not satisfy this exit.

### M3 - Ollama evaluation and hardening

- Harden the interpreter-specific Ollama prompt and strict output schema added
  for the M2 manual path.
- Add bounded configuration under the language namespace.
- Add mocked transport tests and one opt-in local smoke test.
- Benchmark prompt-only 0.5B and 1.5B against current retrieval.

Exit: evidence establishes whether the interpreter improves real usability.
This is the first manual model-backed usability gate: the same CLI recipe must
run against Ollama and emit an explain trace showing exactly which bounded
context was supplied and whether it affected the decision.

### Current manual validation

The real `qwen2.5:1.5b` Ollama model successfully interpreted “please make 17
twice as large,” selected a request-local known procedure, grounded `17`, and
completed a verified Teacher-OFF execution returning `34`. The interpreter is
opt-in; without `SPOON_INTERPRETER=ollama`, normal Spoon behavior remains
portable and does not require an Ollama daemon.

The same pass also fixed deterministic operator-delimited literal binding:
`6*2` no longer requires spaces for Spoon to recover the two numeric inputs.

### M4 - Dataset forge

- Turn the language-kernel curriculum into executable semantic scenarios.
- Add batched surface generation, deterministic alignment, rejection reporting,
  deduplication, family splits, privacy filters, and dataset manifests.
- Freeze a held-out/adversarial benchmark before training.

Exit: one command produces a reproducible, inspectable training dataset and
report without one Teacher call per example.

### M5 - LoRA training and artifact pipeline

- Add pinned training configuration for 0.5B and 1.5B.
- Train, evaluate, merge, convert, quantize, and reevaluate.
- Produce model cards, manifests, and benchmark reports.

Exit: the smallest qualifying GGUF interpreter is a versioned candidate
artifact.

### M6 - Embedded Rust runtime

- Benchmark the Rust backend candidates.
- Implement the repository-owned interpreter trait and selected backend.
- Add model installation, verification, caching, offline loading, budgets,
  cancellation, and packaging.
- Run behavior parity and platform smoke tests.

Exit: Spoon performs local interpretation without Ollama or a Python runtime.

### M7 - Grounded response planning

- Construct response plans inside the Engine from execution/evidence.
- Add deterministic templates for core dialogue acts and intent results.
- Resolve evidence internally and distinguish it from the public caller-supplied
  renderer audit.

Exit: interpretation, execution, response planning, and rendering compose with
Teacher OFF.

### FUTURE - Language Realizer

- Evaluate template selection and bounded clause ordering first.
- Admit generative realization only with immutable-plan grounding checks and a
  dedicated claim/uncertainty preservation benchmark.
- Keep the interpreter and realizer as independently replaceable artifacts.

## File Map

Expected additions:

```text
crates/spoon-language-model/       embedded interpreter abstraction/backend
packages/intent/                   Ollama/dev interpreter transport
packages/forge/                    batched dataset generation and validation
seeds/intent-specs/                versioned semantic intent catalogs
training/language-interpreter/     pinned LoRA/evaluation/export workflow
benchmarks/intent-parser/          frozen held-out and adversarial fixtures
```

Expected modifications:

- `crates/spoon-core/src/language.rs` and focused tests;
- `crates/spoon-core/src/episode.rs`;
- `crates/spoon-engine/src/cycle.rs` and Engine storage/API modules;
- `crates/spoon-server/src/lib.rs`;
- SDK types/client, CLI cycle/config, and Inspector projections;
- seed schemas/curricula, benchmark runner/reporting, `STATUS.md`, and the
  implementation inventory as evidence becomes real.

## Delegation Map

Once M0 fixes the contracts, bounded work can proceed in parallel with explicit
file ownership:

- core/Engine integration and final architectural review: root agent;
- SDK/CLI protocol and tests: delegated agent;
- Ollama adapter package and mocked tests: delegated agent;
- curriculum/data-forge schemas and fixtures: delegated agent;
- training harness/configuration: delegated agent;
- Rust runtime bakeoff: separate research/prototype agent after artifact shape
  is stable.

Delegated branches must not claim subsystem completion. The root integration
pass owns cross-package validation, `STATUS.md`, and implementation-reality
claims.

## Repository Gates

Run focused tests during each TDD cycle, then before an integrated milestone:

```text
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
pnpm test
pnpm typecheck
pnpm build
pnpm depcheck
git diff --check
```

No model benchmark, mock transport, passing schema test, or declared type alone
upgrades Spoon's implementation status. A milestone is real only when its
public path, local validation boundary, Teacher-OFF behavior, and adversarial
evidence pass.
