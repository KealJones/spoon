# Spoon

A persistent, self-extending semantic cognitive system.

Spoon is an experiment in building an intelligent system whose accumulated
knowledge, reasoning methods, and executable abilities exist outside the fixed
parameters and finite context window of a large language model. LLMs serve as
Spoon's ears (language in), mouth (language out), and teacher (filling knowledge
gaps) - but the persistent cognitive substrate is Spoon's own.

## Architecture

Everything in Spoon is a **Concept**. Concepts come in three shapes:

```rust
enum Concept {
    Atomic(ConceptId),                                   // Greg, Add, 42
    Compound { head: Box<Concept>, args: Vec<Concept> }, // FriendWith<Greg, Keal>
    Hole(HoleId),                                        // placeholder in patterns
}

enum ConceptId {
    Named(SymbolId),   // greg, add, sort, friend-with
    Ground(Ground),    // 42, "hello", true, {json}
}
```

Entities, relationships, expressions, programs, and inference rules are all
concepts. There are no privileged primitives: `42` is an atomic concept whose
identity is its value, and it costs no database row until something is asserted
about it.

There is no separate IR. Evaluation is term rewriting. A concept like `Sort` has
one or more **realizations** (native Rust, composed concepts, rewrite rules, LLM
calls, external processes). Execution means: find a realization for the head
concept, apply it, recurse. Realizations compete, and experience decides which
one wins in a given context.

Concepts infer other relationships through meta-concepts like `Symmetric`,
`InverseOf`, and `TransitiveClosure`. These are not magic: they are concepts
whose realizations happen to be rewrite rules.

The LLM ears decompose messy human language into flat concept sequences. The
vocabulary self-ranks by usage (ACT-R activation), so the most useful concepts
sit at the top of the LLM prompt. Unknown words get structurally placed and
resolved through synonym lookup or Teacher assistance.

## Status

**v2 architecture pivot in progress.** See `PIVOT_PLAN.md` for the staged
development plan and `docs/CONCEPT-IR-DESIGN.md` for the core design rationale.

v1 code in `crates/` (345 tests, typed CAN, Earley parser, synthesis, episodes)
is retained as reference. It proved out many pieces but used a rigid
Concept/Action/Fact/Clause representation that the unified v2 model replaces.

## Design Documents

| Document | Contents |
|---|---|
| `PIVOT_PLAN.md` | Re-architecture stages, crate structure, success criteria, timeline |
| `docs/CONCEPT-IR-DESIGN.md` | Core design: concepts, evaluation, inference, ears, vocabulary ranking |
| `docs/EVALUATION.md` | Evaluator contract: budgets, realization selection, effect authority, tracing |
| `docs/PRIOR_ART_MEMO.md` | Relevant algorithms and system precedents |
| `AGENTS.md` | Agent instructions, hard rules, pick-up procedure |

## Prior Iterations

- `ekg` / `ekg-ai`: earlier implementations exploring the same ideas
- The Spoon whitepaper describes the full vision this system is working toward

## Quick Start

```bash
cargo check                       # fast compile
cargo test --workspace            # all tests
cargo run -p spoon -- repl        # talk to it
cargo run -p spoon -- serve       # OpenAI API on :8787
```

Rust, one binary, SQLite persistence. Optional: Ollama for the three LLM seats.
