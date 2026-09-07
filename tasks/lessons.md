# Lessons (corrections from Keal, newest first)

## 2026-09-02 Subagent model cost
- Every subagent spawned with the default `inherit` ran on the session model
  (Fable), the most expensive one, across 17 rounds. Keal caught it.
- Rule: always pass `model` explicitly. Sonnet for mechanical rounds (rules,
  wiring, tests, HTML, docs, benches). Opus-class only for a design-heavy
  round, and say so before spawning.
- Rule: never run more than two subagents at once; each one compiles the whole
  workspace and runs Ollama.
- Rule: skip live-Ollama benchmarks in a round unless the change is in the ears
  or mouth. Offline tests are enough to verify wiring.

## 2026-09-03 Architecture pivot - don't preserve v1 abstractions
- v1 had separate Concept, Action, Fact, Clause types. v2 unifies everything as
  one `Concept` type in three shapes (atomic / compound / hole). Don't carry
  forward v1 type hierarchies.
- When porting v1 functionality (primitives, synthesis, etc.), port the behavior,
  not the types. Everything becomes concepts.
- The SCE controlled language and Earley parser are gone. Ears now produce
  concept sequences directly via LLM structured output.

## 2026-09-03 Don't reintroduce privileged primitives
- I proposed a 4-variant representation with a separate `Val` variant for
  literals. Keal pushed back: that IS a privileged primitive, exactly what the
  whitepaper argues against, and it creates a two-tier system where literals
  can't participate in relationships.
- Resolution: ground values are atomic concepts whose identity IS their value.
  No separate value type. `42` is `Atomic(Ground(Int(42)))`.
- The performance worry (a database row per number) was solved by lazy
  materialization, not by adding a special case. Ground concepts are
  self-describing, so they need no row until something is asserted about them.
- Lesson: when a design decision feels like it violates a stated principle "for
  performance", look for a mechanism that preserves the principle first. The
  optimization usually exists.
