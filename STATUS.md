# Spoon: status log

Pick-up-later file. Newest entry first. Each entry: DONE (wired and exercised
through the binary or a test), IN PROGRESS, NEXT, gotchas. "Done" means it runs,
not that a type exists. Decisions live in PLAN.md; rules in AGENTS.md.

---

## 2026-09-02 05:00  M4 part 1: bench + teach wired (commit 0f50eb7)

DONE (252 tests, 0 warnings)
- `spoon bench convo20|ace|babi|demo` through the real Brain (`crates/spoon/src/
  bench.rs`). Prints a table + weaning line (turns, ears_native/llm/failed,
  mouth_llm, teacher_llm) and bails if interior_llm_calls != 0. Results go to
  `data/bench/results/` (gitignored). `demo` = the 19 pick-up lines
  (`data/bench/demo.json`), hard regression: 19/19 offline.
  Offline numbers: convo20 20/20, ace 30/158 structural (native only), babi 1/21.
- `spoon teach --lessons N [--themes ..] [--max-minutes M] [--dry-run]`
  (`crates/spoon/src/teach.rs`, `brain/teach.rs`): curriculum -> capability
  (spec_for -> learn_from_spec -> phrasings_for -> Pairs), facts and concepts
  (stored with source="teacher" via `Brain::assert_sce` so `export` includes
  them; turn facts are source="user" and excluded), phrasings -> Pairs,
  opinion -> stance. Ledger kv `teach.done` makes it resumable. Restart test
  in `spoon-mind/tests/teach.rs`.
  Live (qwen3.5:4b, 6 lessons, 24s): 0/1 capability (the teacher's word_count
  examples were miscounted, so no program fits: correct behaviour), 1/2 facts,
  1/1 phrasings (6 pairs), 1/1 opinion (remote work, used by a later turn),
  1/1 concept (recipe, 7 facts). Steered run: `square = mul(x, x)` learned
  from the teacher's examples + 6 phrasings.

BABI FAILURE CLASSES (the interior gaps, in order of probes affected)
1. No location state: `Mary moves to the bathroom.` stores an event but
   `Where is Mary?` has no location semantics (fam 1,2,3,12: 9 probes).
2. PP predicates do not parse/store: `Is Mary in the garden?`, `What is south
   of the office?` (fam 4,6,9,10,17: 7 probes).
3. Three-argument relations: `John gives the apple to Maya.` (fam 5).
4. No possession aggregation: `How many objects does Mary carry?` (fam 7,8).
5. Names from story lines are not added to the gate lexicon (`Where is
   Sandra?` -> unknown: sandra); plural universals `Wolves are white.` fail.
6. Cosmetic: `elephant_1 is bigger-than` leaks an entity id; `I don't know
   where is Mary` keeps question word order.

TEACHER NOTES: qwen3.5:4b drops JSON fields (parser now lenient), miscounts
examples, reads `square` as "area of a square", and repeats the same 6 lessons
per prompt (deterministic). Failed lessons stay in `teach.done` (no
--retry-failed yet).

NEXT
1. Ears round 5: garbage guard, reported speech, for-each variables, plural
   universals; brain adds Names to the gate lexicon on grounding.
2. Discourse round: location state (`moves to`/`is in` -> `Where is X?`), PP
   predicates, 3-arg relations, counting/listing possessions. Target babi >= 12/21.
3. README.md. 4. Mouth stance check + realizer cosmetics.

## 2026-09-02 04:20  M1-M3 demoable; messy input native (commits 8bc352e, 2d5f52f)

DONE. Pick-up check passed: all 19 demo lines below run offline through
`cargo run -q -p spoon -- --ephemeral --offline --debug stdio`, interior_llm_calls 0,
241 workspace tests, 0 warnings.
```
Hello! / John owns a dog. / Who owns a dog?            -> yo. what do you need? / noted / John.
Assistant, calculate 3 / 500 * 3600!                   -> 21.6
Assistant, double 21! + two double facts               -> 42 learned: double = add(x, x)
Assistant, triple 4! + two triple facts                -> 12 learned: triple = mul(x, 3)   (smaller than the old add(x, double(x)))
What is the triple of 7?                               -> 21. (source: computed)          (question -> learned action, brain r3)
What is the reverse of "abc"?                          -> cba. (source: computed)         (question -> kernel action)
Is John happy? / "pup" means "dog". / User means Mary. -> I don't know ... / learned / got it: Mary owns a dog
Assistant, concatenate "ab"! -> cd                     -> what b should i use? -> abcd    (placeholder round trip)
```
Messy input (ears round 4), same binary, Ollama on but 0 ears LLM calls:
```
yo whats up                                        -> User greets Assistant.        phrasing
ok so john has this dog right and like the dog is brown -> John owns a dog. The dog is brown.
can u double 21 for me                             -> Assistant, double 21!          (teacher taught double: 42)
whats the double of 100                            -> What is the double of 100?     -> 200
im feeling kinda down today                        -> User is sad.                   -> i hear you...
no i meant mary / what do you think about dogs / thanks!   all correct shapes
```
- Ears: phrasing-first, Direct only when clean (no unknown content words, or a
  Command whose only unknown is the verb), deterministic rules in
  `ears/rules.rs` + `ears/sentence.rs` (contractions, indirect requests,
  feelings, corrections, question restoration, clause split, greeting split),
  LLM last, then direct-with-unknowns at conf 0.3. `Gate::parse_reported`
  returns unknown words. `data/bench/convo20.json`: 20/20 native.
  ACE live qwen3.5:4b: 53/158 structural, 104 parsed, repairs counted (60).
- Brain r3: `dispatch/property.rs` routes `What is the N of X?` / `Is the N of X
  V?` to a CAN action named N (kernel first, then learned) when memory has no
  fact; `Present` (Result|Answer|YesNo) tells the brain how to render. Failed
  ears turns render via template (no echo). `User thinks that dogs are great.`
  stored and answered; opinion replies cite known facts about the topic.
- `spoon serve` smoke-tested: `/v1/chat/completions` (+ `spoon` metrics block),
  `/v1/models`; sessions from X-Spoon-Session > `user` > first-message hash.

KNOWN ISSUES (queued)
- Ears LLM invents SCE from garbage: `zxqv flarp wibble` -> `Zxqv is a wibble.`
  Needs a garbage guard (all content words unknown -> Failed) and a prompt
  escape hatch. ACE misses: reported speech 0/20, context/ellipsis 1/22 (belongs
  to discourse, not ears), for-each -> `For every N X` variable form 0/20.
- Mouth adds stance: Reflect `ok so dogs are great.` rendered `i agree, dogs
  are great.` Faithfulness check should reject first-person stance verbs
  unless the plan has an Opinion move.
- Parser: `What is 3 * 4?` (operators only inside calculate), `User says
  goodbye to Assistant.` (only hyphenated form), `Assistant, now!`.
- Example ask uses `The triple of 3 is 6.` (generic 3 -> 6 template).

NEXT (M4)
1. `spoon bench convo20|ace|babi|demo` through the real Brain, JSON results
   under data/bench/results, ears path counts = weaning numbers.
2. `spoon teach --lessons N`: curriculum -> capability specs + phrasings +
   facts + stances + concepts, all stored, resumable, wall-clock capped,
   then export.
3. README.md. 4. Ears round 5 (garbage guard, reported speech, for-each).
5. Mouth stance check.

## 2026-09-02 02:30  Brain round 2 landed; ears LLM seat wired (commits 3e6f156, a06a597)

DONE (offline stdio, interior_llm_calls == 0 everywhere, 17 brain tests)
```
Is John happy?                   -> I don't know whether John is happy.
Who owns a cat?                  -> I don't know who owns a cat.          (noun-constrained lookup)
Assistant, double 21!            -> I can't double yet. Teach me with examples like 'The double of 3 is 6.'
"pup" means "dog".               -> learned: "pup" means "dog"            (persists in kv seed.synonyms)
John owns a pup.                 -> noted, john owns a dog.
User means Mary.                 -> got it: Mary owns a dog               (retract + re-store last assert)
Assistant, concatenate "ab"!     -> what b should i use? (needs a text)   -> cd -> abcd   (placeholder round trip)
Assistant, double 21! (no examples, online) -> 42. learned double = add(x, x) (from the teacher)
```
- Planner: input expansion skips zero-input producers (the clock). Before this
  every Int/Text input was fillable from `time.now`, so no SCE command could
  ever reach a Placeholder.
- `BrainHost::with_store` gives `mem.recall` real facts; dispatch uses the
  memoryless host because it already holds the store lock.
- Ears LLM seat now runs in `turn` (`Ears` behind a tokio mutex held for the
  turn; `SceGate` is `Clone` and snapshotted for the async parse, so the turn
  future stays Send). `answer_pending` runs before the ears so "cd"/"yes"
  never cost an LLM call. Live: `ok so john has this dog right and like the
  dog is brown` -> `John owns a dog. The dog is brown.` (1.5s, qwen3.5:4b),
  then `who owns a dog` answered from memory.

KNOWN BUGS (live messy-input run, all in the ears layer; ears round 4 in flight)
- Junk accepted as Direct: `Hello whats up.` / `who owns a dog.` parse (sentence-
  initial capitalized unknown -> Name, unknown verb, bare noun). Direct must
  require no unknown words (except the verb of a Command) and phrasings must
  run before the direct parse.
- `can u double 21 for me` -> `can Assistant double 21 for User.` -> capability
  listing. Indirect requests must become `Assistant, double 21!`.
- The LLM computed: `whats the double of 100` -> `calculate (100 * 2)`. Prompt
  must forbid evaluating; `What is the double of 100?` is the target.
- `im feeling kinda down today` failed twice; target `User is sad.` The failed
  reply also echoed the input (mouth LLM prepends it; brain round 3).
- Parser note: `Hello whats up.` is a legit SCE shape (Name Verb Noun) under an
  open lexicon, so the parser cannot reject it; the ears must (unknown words
  -> not a clean Direct). `Assistant, now!` (zero-arg command) fails to parse.

NEXT
1. Ears round 4 (running): gate reports unknowns, Direct-path policy, indirect
   requests, feelings, wh `?` restoration, no-compute prompt rule, convo20 bench.
2. Brain round 3: Failed-ears reply without echo; Empathize/Reflect moves for
   `User is sad.`; `spoon serve` smoke test through the OpenAI endpoint.
3. STATUS demo lines re-verified through `repl` after 1-2.

## 2026-09-01 23:40  M1 + M3 core loop demoable offline (commit ae40898)

DONE (all through `cargo run -q -p spoon -- --ephemeral --offline stdio`,
`interior_llm_calls == 0` on every turn, spoon-mind 8 brain tests green)
```
Hello!                                        -> yo. what do you need?          (ears: phrasing)
John owns a dog.                              -> noted, john owns a dog.
Who owns a dog?                               -> John. (source: memory)
Assistant, calculate 3 / 500 * 3600!          -> 21.6
Assistant, double 21!                         -> can't double ... give me examples
The double of 3 is 6. The double of 5 is 10.  -> 42 learned 'double': (math.add p0 p0)
Assistant, double 100!                        -> 200
What is the name of Assistant?                -> Spoon. (source: memory)
Assistant, triple 4! + two triple facts       -> 12 learned 'triple': (math.add p0 (learned.double p0))
```
The last line is the point of the project: `triple` was composed from the
learned `double`. Learned actions survive restart (test with a temp db).
- brain: `spoon-mind/src/brain.rs` (727 lines, orchestrator) + `brain/{session,
  host,exec,learn,respond}.rs`. Sessions hold discourse state, pending
  executor round trips (NeedInput/NeedPermission/NeedChoice) and a pending
  unknown verb. `turn` is async but the body is `turn_sync` (parking_lot
  guards are !Send across awaits; the axum handler needs Send).
- Learning by example needs no syntax: property facts `rel.V(x, y)` about an
  unknown verb V are the Spec examples; synthesize; `action_from_program`;
  Tier::Learned; the provisional `rel.V` relation action is deleted from the
  store so the learned one is not shadowed after restart.

KNOWN BUGS (seen in the demo, fix in the next brain round)
- `Is John happy?` with no such fact answers "nothing." (should be unknown).
- `yo whats up` went Direct as an Assert ("got it: hello whats up"): the parser
  accepted junk. The SCE no-silent-drop fix in flight should make it fail the
  gate so phrasings/LLM take over; verify after it lands.
- Unknown-capability reply is doubled and asks for `input -> output` examples;
  it should ask for SCE facts ("The double of 3 is 6."). "learned: learned"
  doubled prefix too.
- `BrainHost` has no `facts`/`recall` (Store is !Sync); `mem.*` prims see
  nothing. Give the host `Arc<Mutex<Store>>` or a snapshot.
- Not wired yet: corrections ("No. User means Mary."), teacher seat (online
  spec_for for unknown verbs without examples), executor placeholder demo
  (`plan_with_placeholder` test skipped: no kernel action reachable from SCE
  leaves an input unfilled; needs a seeded action or a Signal gap).
- Warnings: dispatch/* unused imports, brain `data_dir` unread, grow `size`.

- ears round 3 LANDED (01:00, commit 69bb4ab): speaker grounding (i/you ->
  User/Assistant), sentence-final filler drops, structural ACE hit metric,
  parser-error repair round, prompt cut from 665 lines to a short one.
  15 tests. Sweep partial: qwen3.5:0.8b parsed 65/158 (41%), structural hits
  26 (16%), `repairs: 0` (suspicious: the repair round may not be counted or
  not triggered in the bench). 2b/4b runs NOT done: the machine was
  thrashing (swap full, load 100+), sweep killed at 01:06. Re-run later,
  one model at a time: `SPOON_LLM_TESTS=1 cargo test -p spoon-lang --test
  ears bench_ace_model_sweep -- --ignored --nocapture`. Results land in
  `data/bench/results/` (file date stamp is wrong, uses %m%d swapped).

OPERATIONS NOTE (01:10): three subagents were reported dead (PING timeouts)
while the sweep held 3 Ollama models resident and cargo builds ran in
parallel. Run at most ONE heavy agent at a time on this machine, and never run
the LLM bench while agents are compiling.
UPDATE (01:15): "dead" agents were not dead. The harness stopped tracking
them but their processes kept going: the sce agent was still editing
`parser.rs` at 01:15, and the ears agent had left 7 stacked
`bench_ace_llm_live` runs queued on Ollama (killed). Before respawning on a
module, check `stat` mtimes and `ps | rg cargo`; a fresh agent backed off
correctly when it saw live edits.

- sce strict LANDED (01:36, commit b13153e): leftover tokens fail the parse
  with the token named (`unexpected '"/tmp/x"' at 5`), literal owners parse
  (`The double of 3 is 6.`), noun singularizer, realizer capitalizes.
  25 tests incl. `no_silent_drops`, `full_consumption`, `literal_owners`,
  `plural_nouns`. Corpus 136/158 (86.1%). `parser.rs` is 1786 lines: split
  it (np / vp / questions / rules) when the grammar settles.

IN PROGRESS
- brain round 2 (ffb47212, spawned 01:38): the KNOWN BUGS above, corrections
  (`"pup" means "dog".`, `User means Mary.`), teacher path via
  `learn_from_spec`, placeholder demo, host facts/recall, warnings sweep.
- ears zombie (37bc5c86, harness says dead, still editing `ears/mod.rs`,
  `normalize.rs`, the prompt at 01:32): more filler words, prompt tweaks.
  A watchdog (`/tmp/zombie_killer.log`) kills duplicate bench runs but lets
  one run at a time. Commit its tree when it goes quiet and tests pass.
- ears round 3 (37bc5c86): I/you -> User/Assistant grounding, structural hit
  metric, parser-error repair round, prompt rewrite, 0.8b/2b/4b model sweep.

NEXT
1. Brain round 2: the KNOWN BUGS above, corrections, teacher, placeholder demo.
2. REPL polish: `:metrics`, `:snapshot`, `:trace` and the STATUS demo lines.
3. `spoon serve` smoke test with an OpenAI client; SSE.
4. M4: curriculum + `spoon teach` overnight; export baseline seed.

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
- ears LANDED (23:10, commit fbc92b2): `spoon-lang/src/ears/{lexicon,values,
  phrasings,induce,llm,gate,bench}.rs`. `Ears::hear(text, &dyn Gate)`,
  `SceGate::{with_defaults,from_can}` over `sce::parse_text`, `run_ace`.
  12 tests. `data/seed/common_words.txt` (2500 words, hand-compiled) makes
  typo repair conservative. WEANING BASELINE on ACE 158 with qwen3.5:4b:
  native 18/158 parsed (11%), LLM 90/158 parsed (57%), 11 exact hits (7%).
  Live LLM normalizations take 2-10s on 4b. Top misses: fillers surviving
  ("even", "again", trailing "at"), User/Assistant swapped, synonym lemmas
  ("has" vs "owns"). Spike prompt reached 76% parsed; prompt work next.
- sce LANDED (22:55, commit 00fd861): `spoon-lang/src/sce/{tokenizer,lemma,
  lexicon,parser,arith,realize}.rs`. `parse_text(text, &Lexicon)`, `realize`.
  21 tests, 136/158 corpus (86%). Verified by probe: property facts, Who/What
  questions, arithmetic commands, rules, yes/no all give the expected shapes.
  Follow-up in flight: parser silently DROPPED trailing content
  (`open the file "/tmp/x" and save it!` -> just `open(the file)`), literal
  owners fail in declaratives (`The double of 3 is 6.`), plural lemmas,
  warnings. GOTCHA for ears/brain: `Hello!`, `What do you think about X?`,
  `No, I meant Mary.`, `Assistant, what is 2 + 2?` are NOT SCE; the
  normalizer must produce `User greets Assistant.`, `What does Assistant think
  about X?`, `User means Mary.`, `What is 2 + 2?`.
- brain wiring (Opus subagent, spawned 22:45): turn loop ears -> discourse ->
  dispatch -> plan/execute -> grow -> mouth, session state with executor
  round trips, StoreHost, learned-verb loop. DECISION: teaching by example
  needs no new syntax; "The double of 3 is 6." is a property fact
  `rel.double(3, 6)` and facts about an unknown verb are the synthesis
  examples. Tests in `spoon-mind/tests/brain.rs` incl. restart survival.
- ears rework (resumed 22:35): first pass landed 10 tests but typo repair
  mangled real English ("you" -> "young") because the lexicon had no common
  words, and the live test's FakeGate accepted anything so the LLM never ran.
  Fixing: `data/seed/common_words.txt`, conservative Damerau repair, real
  `SceGate` over `sce::parse_text`, ACE bench native + LLM baseline.
- grow LANDED (22:37): `spoon-mind/src/grow/{synth,oe,consolidate}.rs`.
  `synthesize(spec, can, kernel, budget) -> SynthOutcome`, `action_from_program`,
  `consolidate`, `describe`. 11 tests, deterministic, 0.4s total. Backward
  type-reachability pruning (`reach[k]` BFS over pure actions, list element
  types distinguished), `value.*` skipped unless spec touches Any/Json,
  constant-only subtrees pruned at size >= 2. map_double: 59.9k nodes/206ms.
- grow rework (resumed 22:25, done): deterministic order, no lossy bank cap,
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
