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
  it from sce), grow `spoon-mind/src/grow/` (synth + consolidate). (teacher
  and bin landed, see below.)
- grow rework (resumed 22:25): deterministic order, no lossy bank cap,
  backward type-reachability pruning; targets map_double < 1 s at default budget.
- orchestrator: `types/spec.rs` (Spec, Example) added as the grow/teacher
  contract; `brain.rs` public API frozen: BrainConfig, Brain::open/turn/
  metrics/snapshot, TurnResult, BrainMetrics, Snapshot.
- plan: planner (AND/OR search) + executor in `spoon-mind/src/plan/`
- discourse: referents, facts QA, corrections in `spoon-mind/src/discourse/`

LANDED FROM BATCH 2
- dispatch: `spoon-mind/src/dispatch/{mod,dialog,selfmodel,opinion,arith,command}.rs`
  + `data/seed/self_model.json`. 15 tests green. Returns `Dispatched::{Moves,
  Plan{intent}, UnknownCapability, NeedsTeacher{ask, fallback}}`; opinion
  keywords are matched in both hyphenated and spaced forms.
- discourse fixes (orchestrator): property facts. "The X of Y is Z" now stores
  `rel.X(Y, Z)` (was `rel.is_a(minted_x, Z)`, unqueryable) and "What is the X
  of Y?" / "Is the X of Y Z?" read it back; yes/no `be` questions now see the
  clause referents (they got `&[]`, which made unknown objects answer true).
  Test: `tests/property_facts.rs`.
- teacher: `spoon-mind/src/teacher/{spec,stance,curriculum,prompts}.rs` +
  `data/prompts/teacher_*.md`. 10 offline tests green. Live on qwen3.5:4b:
  `spec_for("double")` gives a valid 8-example Spec in ~3s; `stance_for`
  gives stance + 4 reasons + 2 counterpoints; `curriculum(5)` gives mixed
  lessons. Wiring notes: `parse_pairs_json` leaves `Pair.at = 0` and
  `Store::insert_pair` does NOT fill it (brain sets `now_ms()`); concept
  provenance comes back with an empty lesson_id (brain fills); `Facts`
  lessons are loose English, run them through the parser gate and drop
  failures.

LANDED FROM BATCH 1
- plan: `spoon-mind/src/plan/{planner,executor,cost}.rs`. AND/OR search over
  producers_of with Placeholders (cost 10), Map insertion for One<-Many,
  projection through struct properties; executor with NeedInput /
  NeedPermission / NeedChoice round trips via `ExecState`. 12 tests green.
- discourse: `spoon-mind/src/discourse/{ground,facts,rules,correction,keywords}.rs`.
  Grounding mints `noun_k` entities; copula "X is smart" -> `rel.is(x, Text)`,
  "X is a dog" -> `rel.is_a(x, concept)`; provisional Relation actions are
  created on first use; universals become rules with forward chaining. One
  integration test with 11 scenarios green.
- bin: `crates/spoon/src/{lib,cli,repl,stdio,seedio}.rs` + `server/` (axum
  OpenAI chat completions with SSE, /v1/models, /debug/metrics, /debug/snapshot,
  /health, inspector.html). 6 tests green. `teach` and `bench` exit 2 "not
  wired yet". Verified: `--ephemeral --offline stdio` round-trips JSON lines
  through the stub Brain.
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
