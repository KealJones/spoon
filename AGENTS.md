# Spoon v2: Agent Instructions

## Architecture

Read in this order before doing anything:
1. `PIVOT_PLAN.md` - the re-architecture plan, stages, crate structure
2. `docs/CONCEPT-IR-DESIGN.md` - core design: Concepts, evaluation, inference, ears
3. `docs/EVALUATION.md` - the evaluator contract (Stage 2 spec)
4. `STATUS.md` - what actually works (newest entry first)

The old v1 code in `crates/` is **reference only**. We are rebuilding from
scratch with a unified Concept model. See PIVOT_PLAN.md for what we're keeping
(ideas) vs what's being replaced (code).

## Hard Rules

1. **No LLM in the interior.** Three seats only: `Ears`, `Mouth`, `Teacher`.
   Evaluation, inference, realization selection, learning - all pure code.
   Track `interior_llm_calls` and assert it stays 0 in tests.
2. **Everything is a Concept.** No separate Concept/Action/Fact/Clause types.
   Atomic, compound, hole - that's the representation. If you're creating a new
   struct for something that should be a concept, stop and reconsider. There are
   no privileged primitives: numbers and strings are atomic concepts with ground
   identity, not a separate value type.
3. **Wire or delete.** A module counts as done only when a REPL turn or test
   exercises it. Update `STATUS.md` when that happens, not before.
4. **No god files.** Keep modules under ~800 lines.
5. **Teacher never writes executable bodies by default.** It writes specs,
   examples, synonyms, concept relationships. The synthesizer writes programs.
6. **Realizations compete.** No concept gets exactly one implementation.
   Selection is contextual and evidence-weighted.
7. **Everything survives restart.** SQLite persistence. Test with
   open/close/open cycle. Export/import for git-friendly seeds.
8. **No em-dashes** in any text (code comments, docs, prompts, replies).
9. **Third-party crates for plumbing only** (parsing, storage, HTTP, search,
   embeddings). Never for the choosing.

## Crate Structure (target)

```
crates/
  spoon-concept/    Concept, ConceptId, Ground, HoleId, structural ops
  spoon-store/      Store trait, SQLite impl, indexes, queries, interning
  spoon-eval/       Evaluator, realization dispatch, budget/limits
  spoon-infer/      Pattern matching, unification, rule application
  spoon-ears/       LLM structured output, prompt assembly, vocab ranking
  spoon-mouth/      Response rendering, templates, LLM surface realization
  spoon-learn/      Episodes, evidence, credit, consolidation, synthesis
  spoon-teach/      Teacher seat, gap filling, synonym/concept minting
  spoon/            Binary: repl, stdio, serve, bench, export, import, doctor
```

Dependency: concept <- store <- eval <- infer <- ears/mouth/learn/teach <- spoon

## Key Types

Everything in Spoon is a Concept. Three shapes.

```rust
enum Concept {
    Atomic(ConceptId),                                  // Greg, Add, 42
    Compound { head: Box<Concept>, args: Vec<Concept> }, // FriendWith<Greg, Keal>
    Hole(HoleId),                                        // placeholder in patterns
}

enum ConceptId {
    Named(SymbolId),   // greg, add, sort, friend-with
    Ground(Ground),    // 42, "hello", true, {json}
}

enum Ground {
    Bool(bool), Int(i64), Float(f64), Text(String),
    Bytes(Vec<u8>), DateTime(chrono::DateTime<Utc>),
    Json(serde_json::Value),
}

enum Realization {
    Native(fn(&[Concept], &Store) -> Result<Concept>),
    Composed(Concept),
    Rule { pattern: Concept, condition: Concept, produce: Concept },
    Neural { prompt: Concept, parse: Concept },
    External { effect: Effect, spec: Concept },
}
```

**Named vs ground identity.** A named concept's meaning lives in the store
(surface forms, realizations, relationships). A ground concept's identity IS
its value, so it is self-describing and needs no store row until something is
asserted about it. `Add<42, 1>` touches the store zero times.
`Synonym<"forty-two", 42>` materializes a row for 42.

**Atomic and compound are equal citizens.** Both can be stored, carry
realizations, participate in relationships, and accumulate activation stats.
`Employment<Greg, Workiva>` (compound) gets `Role<..., SoftwareEngineer>`
attached the same way `Greg` (atomic) does.

**Holes are the only non-concept.** They are gaps inside rule patterns and
partially resolved expressions.

**Symbol naming.** `SymbolId::of` lowercases and trims but does not touch
separators, because collapsing them would merge `co-op` with `coop`. So
`FriendWith` and `friend-with` are two different concepts. Use **kebab-case**
for every symbol name passed to `Concept::named` / `Concept::call` and stored in
seeds. PascalCase in prose (`FriendWith<Greg, Keal>`) is a readability
convention for documents, not the canonical spelling.

See `docs/CONCEPT-IR-DESIGN.md` for full rationale.

## Commands

```
cargo check                       fast compile
cargo test --workspace            all tests
cargo run -p spoon -- repl        talk to it
cargo run -p spoon -- serve       OpenAI API on :8787
cargo run -p spoon -- bench       regression suite
```

## Subagent Protocol

Subagents get: exact files they own, the types they code against (read-only),
tests they must make pass, and a ban on touching other modules. They are told
they are subagents and must not re-delegate. Prefer Sonnet. Each prompt is
self-contained.

## Pick-up Procedure

1. `cargo test --workspace` and note failures.
2. Read the top entry of `STATUS.md`; its NEXT list is the queue.
3. Check which stage of PIVOT_PLAN.md we're on.
4. Continue. Append a STATUS entry when something is wired.

## Reference Material

- `docs/CONCEPT-IR-DESIGN.md` - Concept/IR/ears design discussion
- `docs/EVALUATION.md` - evaluator contract: budgets, selection, effects, tracing
- `PIVOT_PLAN.md` - re-architecture stages and success criteria
- Old v1 codebase in `crates/` (reference for porting primitives/benchmarks)
- Spoon whitepaper: `~/Downloads/writing-block (1).md`
- Prior iterations: `~/Git/Personal/ekg/`, `~/Git/Personal/ekg-ai/`
