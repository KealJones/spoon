# Spoon: Pivot Plan

## Context

The previous Spoon implementation (v1) proved out many individual pieces:
typed IR, kernel primitives, SQLite persistence, ears/mouth/teacher LLM seats,
enumerative synthesis, episodic memory, planner, discourse. It works - 345 tests
pass, benches run, learned capabilities survive restart.

But v1's representation is rigid. Concepts, Actions, Facts, and Clauses are
separate database citizens with different schemas. Relationships are fact
predicates, not first-class concepts. The CAN is a typed function graph, not
a universal semantic substrate.

The Spoon proposal (see `docs/CONCEPT-IR-DESIGN.md` and the full whitepaper)
describes a unified architecture where **everything is a Concept** - entities,
relationships, expressions, executable programs, inference rules. Concepts come
in three shapes: atomic (indivisible, has identity), compound (one concept
applied to others), and hole (a gap in a pattern). There is no separate IR.
Evaluation is term rewriting.

This plan describes how to get there.

---

## What We're Keeping (ideas, not code)

- **Three LLM seats only**: ears, mouth, teacher. Interior is pure code.
- **Persistence in SQLite**: single-file brain, survives restart.
- **Episodic memory**: every turn produces a structured episode.
- **Synthesis from examples**: learn programs from I/O specs.
- **Teacher proposes structure, not executable bodies**.
- **ACT-R activation**: recency + frequency + success for selection/ranking.
- **Effect levels and permissions**: Pure/Read/Write/Network/Shell.
- **Export/import**: git-friendly seed format.
- **Benchmarking harness**: regression suites.

## What's Changing

| v1 | v2 |
|---|---|
| Separate Concept, Action, Fact, Clause types | Everything is `Concept` (atomic / compound / hole) |
| Actions are typed functions in a CAN | Concepts with `Realization` attachments |
| Facts are predicate(args) in a table | Compound concepts in a flat store with multi-index |
| Clauses (DRS-style) from parser | Flat concept sequences from LLM ears |
| SCE controlled English as intermediate | Concepts ARE the intermediate representation |
| Rigid `ConceptKind` enum | Concepts participate in other concepts |
| Fixed relationship predicates | Relationships are concepts with inference rules |
| Kernel is a fixed set of primitives | Primitives are concepts with Native realizations |
| `Value`/`Type` as a separate value system | Ground values are atomic concepts with ground identity |
| Single realization per action | Multiple competing realizations per concept |
| Earley parser for SCE | LLM structured output producing concepts directly |

## What We're Deleting

- `crates/spoon-lang/src/sce/` - the SCE grammar, Earley parser, realizer
- `crates/spoon-core/src/types/` - old Concept/Action/Fact/Clause types
- `crates/spoon-core/src/can.rs` - old CAN index
- `crates/spoon-mind/src/dispatch/` - old clause-act routing
- `crates/spoon-lang/src/ears/` - old native recognizer + phrasing matcher
- Most of `crates/spoon-mind/src/plan/` - old Viv-style typed planner

These aren't bad code. They're the wrong abstraction for the unified model.

---

## Development Stages

### Stage 1: Concept Substrate (foundation)

Build the core representation and store.

**Deliverables:**
- `Concept` enum: `Atomic(ConceptId)`, `Compound { head, args }`, `Hole(HoleId)`
- `ConceptId` enum: `Named(SymbolId)`, `Ground(Ground)`
- `Ground` enum: Bool, Int, Float, Text, Bytes, DateTime, Json
- Symbol interning for named identities
- Structural equality and hashing over concepts
- `Store` trait with SQLite implementation:
  - Insert/query concepts
  - Index by head, by participant, by structure hash
  - Bi-temporal validity (asserted_at, invalidated_at)
  - Provenance per assertion
  - Lazy materialization: ground concepts get a row only when something is
    asserted about them
- `Realization` enum: Native, Composed, Rule, Neural, External
- Concept metadata: surface forms (nouns/verbs/synonyms), activation stats
- `spoon export/import` for the new concept format

**Test:** Store compounds, query by head, query by participant, round-trip
through SQLite, survive restart. Verify `Add<42, 1>` requires zero store rows
while `Synonym<"forty-two", 42>` materializes one for 42.

### Stage 2: Evaluator (term rewriting)

The single execution model. Full contract in `docs/EVALUATION.md`.

**Deliverables:**
- `evaluate(concept, store, context) -> Result<Concept>` - recursive evaluator
- Native realizations for bootstrap primitives (~50-100):
  - Arithmetic: Add, Sub, Mul, Div, Mod
  - Comparison: Eq, Lt, Gt, Lte, Gte
  - Collections: List, Map, Filter, Reduce, Sort, First, Last, Count, Sum
  - Text: Concat, Split, Contains, Replace, Upper, Lower
  - Logic: And, Or, Not, If
  - Store: Assert, Retract, Query, Exists
  - IO: Print, Fetch, ParseJson, ReadFile
- Realization selection: when multiple realizations exist, pick by
  (context_match * success_rate * activation) score
- Depth/budget limits to prevent infinite rewriting
- Effect level checking before executing side-effectful realizations

**Test:** Evaluate `Sum<List<1, 2, 3>>` -> `Atomic(Ground(Int(6)))`. Evaluate
`First<Sort<List<3, 1, 2>>>` -> `Atomic(Ground(Int(1)))`. Evaluate with
competing realizations, verify selection.

### Stage 3: Inference Rules (concepts that infer)

Meta-concepts with rule realizations.

**Deliverables:**
- Pattern matching / unification over concepts (holes bind to concepts)
- `Rule` realization type: pattern + condition + produce
- Bootstrap meta-concepts:
  - `Symmetric<R>`: R<X,Y> -> R<Y,X>
  - `InverseOf<R1, R2>`: R1<X,Y> -> R2<Y,X>
  - `TransitiveClosure<R>`: R<A,B> + R<B,C> -> R<A,C>
  - `DefaultExpectation<Type, Prop, Val>`: defeasible defaults
  - `Synonym<Surface, Concept>`: surface form mapping
- Query integration: when direct lookup fails, check applicable rules
- Depth limits on inference chains

**Test:** Assert `Symmetric<FriendWith>` and `FriendWith<Greg, Keal>`.
Query `FriendWith<Keal, Greg>` -> inferred true. Assert direct evidence
overriding a default.

### Stage 4: LLM Ears (messy language -> concepts)

The language interface.

**Deliverables:**
- Base concept vocabulary (~200-400 structural/operational/reference concepts)
- Dynamic prompt assembly from activation-ranked vocabulary
- LLM structured output producing flat `Utterance<...steps...>`
- Utterance concepts: Task, Bind, Let, Correction, Qualify, Source, Present,
  Fuzzy, Unknown, etc.
- Unknown<"word"> handling: synonym lookup -> Teacher -> new concept
- Correction detection and retraction
- Pair storage: (messy input, concept sequence) for future training

**Test:** Feed messy English through ears, get concept sequences that resolve
to correct operations. "grab the scores and add em up" -> produces a
resolvable concept sequence.

### Stage 5: Episodes + Learning

Cognitive trace and evidence accumulation.

**Deliverables:**
- Episode: stores raw input, concept interpretation, evaluation trace,
  result, user feedback, metrics
- Credit assignment: when something fails, estimate which stage was responsible
- Realization evidence update: success/failure updates activation stats
- User correction handling: "no, that's wrong" -> retract + re-store +
  update evidence
- Positive reinforcement: successful paths get activation boosts
- Consolidation: repeated patterns -> candidate abstractions

**Test:** Run a turn, store episode. Correct it, verify evidence updates.
Verify realization selection shifts after failures.

### Stage 6: Teacher + Self-Extension

Fill capability gaps.

**Deliverables:**
- Gap recognition: evaluator hits concept with no realization, flags it
- Teacher prompt: "here's the concept, context, what was tried, how it failed"
- Teacher produces candidate realization (Composed concept or spec)
- Validation: sandbox execution, test against examples
- Persist new realization, resume original task
- Synonym/paraphrase learning from Teacher

**Test:** Encounter a concept with no realization. Teacher proposes one.
Validate it. Persist. Use it on the next turn without Teacher.

### Stage 7: Mouth + Surfaces

Output, interfaces, and tooling. Port from v1 - these are transport/UI, not
architecture. The atom model changes what's inside the payloads, not the
surfaces themselves.

**Deliverables:**
- Structured response from evaluator (result concepts + context)
- Template rendering for common patterns (data reports, yes/no, lists)
- LLM mouth for natural language rendering (with faithfulness check)
- **REPL** (port from v1 `crates/spoon/src/repl.rs`)
- **stdio** JSON lines (port from v1 `crates/spoon/src/stdio.rs`)
- **OpenAI-compatible API** (port from v1 `crates/spoon/src/server/`):
  - `/v1/chat/completions` with SSE streaming
  - `/v1/models`
  - `/health`
  - Session management via `X-Spoon-Session` header
  - `spoon` metrics block in non-streaming responses
- **Inspector web UI** (port from v1):
  - Tabs: Concepts, Store, Episodes, Realizations, Activation, Vocabulary, Health
  - `/debug/metrics`, `/debug/snapshot` JSON endpoints
  - `/inspector` serves static HTML
  - `?q=` filter on debug endpoints
- **Web chat** interface alongside inspector
- **Doctor** (`spoon doctor`): mine episodes for failure patterns
- **Bench** (`spoon bench`): regression suites with weaning metrics
- **Teach** (`spoon teach`): curriculum-driven training, resumable
- **Export/import** (`spoon export`, `spoon import`): git-friendly atom seeds

**Test:** End-to-end conversation through REPL. Verify template vs LLM
path selection. API serves correct SSE. Inspector renders the concept store.
Doctor identifies failure clusters from episodes.

---

## Migration Strategy

**Fresh start, not incremental refactor.** The unified concept model is
different enough that retrofitting it onto the old types would create a hybrid
worse than either.

1. New crate structure (see below), build Stage 1-2 first
2. Port bootstrap primitives from old kernel (the Rust functions, not the types)
3. Port SQLite schema (new tables, fresh migration)
4. Port benchmarks (same test cases, new format)
5. Keep old codebase as reference (separate dir or git branch)

## Proposed Crate Structure

```
crates/
  spoon-concept/    Concept, ConceptId, Ground, HoleId, structural ops
  spoon-store/      Store trait, SQLite impl, indexes, queries, interning
  spoon-eval/       Evaluator, realization dispatch, budget/limits
  spoon-infer/      Pattern matching, unification, rule application
  spoon-ears/       LLM structured output, prompt assembly, vocabulary ranking
  spoon-mouth/      Response rendering, templates, LLM surface realization
  spoon-learn/      Episodes, evidence, credit, consolidation, synthesis
  spoon-teach/      Teacher seat, gap filling, synonym/concept minting
  spoon/            Binary: repl, stdio, serve, teach, bench, export, import, doctor
```

Dependency chain: concept <- store <- eval <- infer <- ears/mouth/learn/teach <- spoon

## Success Criteria

Same as the proposal's evaluation metrics, but concretely:

- [ ] Store and retrieve concepts, survive restart
- [ ] Evaluate nested compound concepts to ground values
- [ ] Ground concepts cost zero store rows until asserted about
- [ ] Infer relationships through meta-concept rules
- [ ] Parse messy English into concept sequences via LLM
- [ ] Learn from corrections (evidence shifts, realization selection changes)
- [ ] Fill capability gaps via Teacher, persist, reuse without Teacher
- [ ] Benchmark suite passes (port v1 demo + babi cases)
- [ ] `interior_llm_calls` stays 0
- [ ] Learned concepts survive restart and export/import

## Timeline Estimate

| Stage | Effort | Cumulative |
|---|---|---|
| 1: Concept Substrate | 2-3 weeks | 2-3 weeks |
| 2: Evaluator | 2-3 weeks | 4-6 weeks |
| 3: Inference Rules | 1-2 weeks | 5-8 weeks |
| 4: LLM Ears | 2-3 weeks | 7-11 weeks |
| 5: Episodes + Learning | 2-3 weeks | 9-14 weeks |
| 6: Teacher | 1-2 weeks | 10-16 weeks |
| 7: Mouth + Surfaces | 1-2 weeks | 11-18 weeks |

~3-4 months for a working system. Stages 1-3 are the foundation and should
be solid before moving on. Stage 4 is where the system becomes conversational.
