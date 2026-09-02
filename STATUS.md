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
- sce: grammar + Earley parser + realizer in `spoon-lang/src/sce/` (started
  writing files at 21:54 after a long read; do not respawn)
- batch 2 (spawned 22:05): ears `spoon-lang/src/ears/` (Gate trait decouples
  it from sce), grow `spoon-mind/src/grow/` (synth + consolidate), teacher
  `spoon-mind/src/teacher/`, bin `crates/spoon/src/` (clap, repl, stdio, axum
  OpenAI API + SSE, inspector). Bin codes against the `Brain` API in
  `spoon-mind/src/brain.rs`, whose interior is a stub echo until wiring.
- dispatch `spoon-mind/src/dispatch/` (spawned 22:15): Clause -> Moves |
  Plan(Intent) | UnknownCapability | NeedsTeacher. Dialog verb mirroring,
  small-talk policy, self-model QA (`data/seed/self_model.json`), opinions and
  advice from `stances`, arithmetic compile+eval, command -> Intent.
- orchestrator: `types/spec.rs` (Spec, Example) added as the grow/teacher
  contract; `brain.rs` public API frozen: BrainConfig, Brain::open/turn/
  metrics/snapshot, TurnResult, BrainMetrics, Snapshot.
- plan: planner (AND/OR search) + executor in `spoon-mind/src/plan/`
- discourse: referents, facts QA, corrections in `spoon-mind/src/discourse/`

LANDED FROM BATCH 1
- kernel: `kernel/eval.rs` + 12 prim modules, ~147 Stage 0 primitives
  (math/logic, value, text, list, json, time, fs, http, shell, mem, dialog,
  know.wikidata_*) + 14 kernel concepts. 41 tests green. `dialog.*` prims
  return `Value::Json` that deserializes straight into `response::Move`, so
  the brain collects any `dialog.Move`-typed output into the ResponsePlan.
  `mem.*` goes through `Host::recall/now_ms` (store-backed Host is a brain job).
- store: SQLite impl, 15 tests green.
- mouth: `spoon-lang/src/mouth/{templates,faithful,render}.rs`, 10 tests green;
  live LLM render verified after the transport fix below (0.85s, path=Llm).
- data: `data/seed/*` 7 files, JSON valid, ATTRIBUTIONS.md, 0 leaks.
- research: `docs/PRIOR_ART_MEMO.md`.

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
- LLM transport: `llm.rs` speaks Ollama's native `/api/chat` with `think:false`
  by default (`LlmConfig::ollama`). Do NOT use the `/v1` OpenAI shim for local
  models: it ignores `think`, `/no_think` is unreliable on qwen3.5, and the 4b
  model burns 1500-3000 reasoning tokens then times out (mouth saw 100%
  template fallback). Native path renders in <1s. `Transport::OpenAi` exists
  for frontier teachers via `SPOON_TEACHER_URL/KEY/MODEL`.
  Local models: qwen3.5:{0.8b,2b,4b}.
- Mouth faithfulness check is structural (numbers, names, forbidden phrases).
  It does not catch semantic drift (LLM said "I can sort it" for an AskExamples
  move). Keep template output as the reference; open item for the mouth.
