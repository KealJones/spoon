# Spoon: status log

Pick-up-later file. Newest entry first. Each entry: DONE (wired and exercised
through the binary or a test), IN PROGRESS, NEXT, gotchas. "Done" means it runs,
not that a type exists. Decisions live in PLAN.md; rules in AGENTS.md.

---

## 2026-09-01 21:20  M0 skeleton compiles, batch 1 subagents running

DONE
- Recon: autopsies of `ekg` and `ekg-ai`, Viv patent digest, ACE spike read.
  Conclusions folded into PLAN.md "Lessons".
- PLAN.md, AGENTS.md, docs/SCE.md (controlled language spec: the contract
  between ears, parser, normalizer prompt, realizer).
- Cargo workspace: spoon-core / spoon-lang / spoon-mind / spoon (bin).
- spoon-core frozen contracts: `types/{value,can,ir,clause,intent,response,episode}.rs`,
  `can.rs` (in-memory CAN index with is-a, producers_of, ACT-R activation),
  `llm.rs` (one OpenAI-compatible client with Seat counters), `kernel/mod.rs`
  (Kernel, Ctx, Budget, Sandbox, PermissionMode, Host trait), `store/mod.rs`
  (Store API with unimplemented stubs). `cargo check` green.

IN PROGRESS (subagents, Sonnet)
- kernel: `kernel/eval.rs` + `kernel/prims/*` (Stage 0 primitive inventory)
- store: SQLite impl of `store/mod.rs`
- sce: grammar + Earley parser + realizer in `spoon-lang/src/sce/`
- plan: planner (AND/OR search) + executor in `spoon-mind/src/plan/`
- mouth: templates + LLM renderer + faithfulness check in `spoon-lang/src/mouth/`
- data: seed lexicon, slang, dialog phrasings, bAbI probes, normalizer prompt, ATTRIBUTIONS.md
- research: prior-art algorithm memo (lexicon induction, SymSpell, Duckling,
  AND/OR planning, OE synthesis, Stitch, ISU dialog)

NEXT (orchestrator, after batch 1 lands)
1. Wire: `spoon-mind/src/brain.rs` turn loop = ears -> discourse -> dispatch ->
   plan/execute -> ResponsePlan -> mouth; `spoon` bin `repl`.
2. Batch 2 subagents: ears (normalize + native recognizer + LLM normalizer +
   phrasing induction), discourse (referents, facts QA, corrections),
   grow/synth (budgeted typed OE enumeration), teacher (spec + curriculum),
   server (OpenAI API SSE + stdio + debug + inspector).
3. Demo lines for M1: `John owns a dog.` / `Who owns a dog?` /
   `Assistant, calculate (3 / 500) * 3600!` / `hey whats up`.

GOTCHAS
- A parallel run scaffolded this dir at 21:00 with a different ears design
  (JSON-schema codec, "not ACE"). Superseded: ears = SCE controlled language
  with a parser gate (see PLAN.md). If you see stray files from that run, they
  are not authoritative.
- `reqwest::blocking` panics inside tokio; kernel primitives run blocking IO
  on a spawned std thread. The Brain runs turns via `spawn_blocking`.
- Ollama serves an OpenAI-compatible API at `http://localhost:11434/v1`; that is
  the only LLM transport. Local models: qwen3.5:{0.8b,2b,4b}.
