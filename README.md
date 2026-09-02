# Spoon

Spoon is a conversational AI that is not an LLM. Planning, choosing, executing,
learning and answering are ordinary Rust code over a typed Concept Action
Network (CAN) that Spoon grows itself by composition and program synthesis. The
CAN and planner follow the Viv Labs / Bixby lineage (patents US20140380268 and
US9292262B2): typed concepts and actions, a planner that searches from the
signals in an utterance to a goal, placeholders that become prompts. Small local
LLMs (Ollama, 4B or smaller) sit in exactly three seats and nowhere else: the
ears (messy English -> Spoon Controlled English), the mouth (structured reply ->
prose) and the teacher (specs, examples, phrasings, stances; never programs).
Every seat is behind a trait with a native fallback, and `--offline` removes all
three. The interior never calls a model: `TurnMetrics.interior_llm_calls` is
asserted to be 0 in tests, and `spoon bench` aborts if a run reports otherwise.

## What it does today

Condensed from the 2026-09-02 entries of `STATUS.md`; replies shortened. All of it
runs offline with 0 LLM calls and is the `bench demo` regression corpus (19/19).

```
> Hello!                                  yo. what do you need?
> John owns a dog.                        noted, john owns a dog.
> Who owns a dog?                         John. (source: memory)
> Assistant, calculate 3 / 500 * 3600!    21.6
> Assistant, double 21!                   I can't double yet. Teach me with
                                          examples like 'The double of 3 is 6.'
> The double of 3 is 6.                   (asks for one more example)
> The double of 5 is 10.                  42   learned: double = add(x, x)
> Assistant, triple 4! (+ two facts)      12   learned: triple = mul(x, 3)
> What is the triple of 7?                21. (source: computed)
> "pup" means "dog".                      learned: "pup" means "dog"
> User means Mary.                        got it: Mary owns a dog
> Assistant, concatenate "ab"!            what b should i use?
> cd                                      abcd
```

`double` was synthesized from two example facts; there is no teaching syntax, facts about an
unknown verb are the spec. An earlier build synthesized `triple` as `add(x, double(x))`,
composing the learned `double`; the current synthesizer finds the smaller `mul(x, 3)` first.
All of it survives a restart. Messy input, same binary, Ollama running but 0 ears LLM calls:

```
yo whats up                                              -> User greets Assistant.
ok so john has this dog right and like the dog is brown  -> John owns a dog. The dog is brown.
can u double 21 for me                                   -> Assistant, double 21!        (42)
im feeling kinda down today                              -> User is sad.                 (i hear you...)
```

## How it works

One turn, top to bottom (`crates/spoon-mind/src/brain.rs` is the orchestrator):

```
 user text
    |
 [ears]       messy English -> SCE clauses. Native first (learned phrasings, typo
    |         repair against the live lexicon, rewrite rules, Earley parser); Ollama
    |         normalizer only on a miss, and its output must re-parse or the turn fails
 [discourse]  referents, working memory, corrections ("User means Mary.")
    |
 [dispatch]   per clause act:  Assert -> facts     Question -> facts QA / CAN action
    |                          Command -> Intent   Rule -> policies
 [planner]    AND/OR search over the CAN from signals to goal, budgeted in
    |         nodes + time + memory; unfilled inputs become prompts
    |-- no path --> [grow]  compose / synthesize from example facts / teacher spec / ask
 [executor]   typed IR on kernel primitives (math, text, list, json, time, fs,
    |         http, shell, mem, dialog, know) with permission round trips
 [grow]       new program -> Tier::Learned action; phrasings, credit, consolidation
 [mouth]      ResponsePlan -> prose. Ollama render under a no-new-facts check;
    |         deterministic template realizer offline and on any failure
 reply
```

**Three seats.** `Seat::Ears`, `Seat::Mouth`, `Seat::Teacher` in `crates/spoon-core/src/llm.rs`:
one client speaking Ollama's native `/api/chat`, or any OpenAI-compatible endpoint for a
frontier teacher (`SPOON_TEACHER_URL/KEY/MODEL`). Each seat has a call counter; the interior has none.

**The SCE gate, and why SCE.** Spoon Controlled English (`docs/SCE.md`) is an ACE-shaped
English subset with an open lexicon: nouns are CAN concepts, verbs are CAN actions, `User`
and `Assistant` are the fixed speaker names. Every sentence parses to exactly one `Clause`,
so the interior is deterministic, the parser is a cheap Earley grammar, and the LLM is a
codec at the edges, not the thing that decides. Whatever the ears produce must parse, so
hallucinations fail loudly. Each accepted LLM normalization is stored as an (utterance, SCE)
pair and becomes native phrasing data; the headline metric `ears_llm_rate` should fall.

**Learning by example.** `The double of 3 is 6.` is an ordinary property fact
`rel.double(3, 6)`. When a command uses an unknown verb, the facts about that verb are
the synthesis examples: typed enumerative search with observational equivalence pruning,
constants drawn from the utterance, verified on the examples, registered as a learned
action. Online, the teacher can supply the example spec instead.

**What persists.** One SQLite brain (`~/.spoon/spoon.db`, `--db`, or `$SPOON_DB`): concepts,
actions, learned programs, facts, episodes, phrasing pairs, synonyms, stances, teach ledger.
`spoon export` writes a git-friendly JSON seed (teacher facts in, per-session user facts
out); `spoon import` loads one. Restart survival is tested with an open/close/open cycle.

## Quick start

Prerequisites: stable Rust. Optional: [Ollama](https://ollama.com) with
`qwen3.5:4b` pulled for the three seats; without it pass `--offline`.

```bash
cargo run -p spoon -- repl                         # persistent db in ~/.spoon
cargo run -p spoon -- --ephemeral --offline repl   # in-memory, no LLM at all
```

Global flags: `--db PATH`, `--ephemeral`, `--offline`, `--debug`, `--permissions
always-ask|ask-writes|bypass` (default `ask-writes`), `--ears-model`, `--mouth-model`, `--teacher-model`.

OpenAI-compatible server: `/v1/chat/completions` (SSE when `stream` is set), `/v1/models`,
`/debug/metrics`, `/debug/snapshot`, `/health`, `/inspector`. Session id: `X-Spoon-Session`
header, then `user`, then a hash of the first message. Non-streaming replies carry a `spoon` block:

```bash
cargo run -p spoon -- serve --port 8787 --host 127.0.0.1
curl -s localhost:8787/v1/chat/completions \
  -H 'Content-Type: application/json' -H 'X-Spoon-Session: demo' \
  -d '{"model":"spoon","messages":[{"role":"user","content":"John owns a dog."}]}'
# {"object":"chat.completion","model":"spoon","choices":[{"message":{"content":"noted, john owns a dog.",...}}],
#  "spoon":{"episode_id":1,"mouth_path":"...","metrics":{"interior_llm_calls":0,"ears_llm_calls":0,...}}}
```

```bash
# JSON lines on stdin/stdout; `session` defaults to "stdio"
echo '{"session":"s1","text":"Who owns a dog?"}' | cargo run -q -p spoon -- --offline stdio
# {"text":"...","episode_id":..,"mouth_path":"...","metrics":{...},"trace":[...]}
```

`bench` prints a table plus a weaning line (`ears_native / ears_llm / ears_failed`,
`mouth_llm`, `teacher_llm`) and bails if `interior_llm_calls != 0`; `teach` resumes via a `teach.done` ledger.

```bash
cargo run -p spoon -- --offline bench demo      # the lines above; must be 19/19
cargo run -p spoon -- bench convo20|ace|babi    # results in data/bench/results/ (gitignored)
cargo run -p spoon -- teach --lessons 6         # online only; --themes a,b --max-minutes 30 --dry-run
cargo run -p spoon -- export --out seed.json    # --name labels the seed; stdout if --out omitted
cargo run -p spoon -- import seed.json
cargo test --workspace
```

## Layout

| Path | Holds |
|---|---|
| `crates/spoon-core` | `types/` (the contract between modules), in-memory CAN index, kernel (typed IR evaluator + Stage 0 primitives), SQLite `Store` + seed export/import, the one `LlmClient` with seat counters |
| `crates/spoon-lang` | `sce/` (grammar, Earley parser, realizer), `ears/` (normalize, values, phrasings, recognizer, induction, LLM normalizer, ACE bench), `mouth/` (templates, LLM render, faithfulness check) |
| `crates/spoon-mind` | `discourse/`, `dispatch/`, `plan/` (planner, executor, cost), `grow/` (synth, consolidate), `teacher/`, `brain.rs` orchestrator |
| `crates/spoon` | binary: `repl`, `stdio`, `serve`, `teach`, `bench`, `export`, `import`; axum server; inspector HTML |
| `docs/` | `SCE.md` (the controlled language contract), `PRIOR_ART_MEMO.md` |
| `data/` | `seed/` (lexicon, slang, phrasings, facts, self model), `prompts/`, `bench/` (convo20, ACE 158, bAbI probes, demo) |

## Status

M0 to M3 are demoable; M4 (pretrain + ship) is in progress. As of the top `STATUS.md`
entry (commit 0f50eb7): 252 workspace tests, 0 warnings.

Works: the 19 demo lines offline (19/19); `convo20` messy conversational lines 20/20
native; learn-by-example synthesis, corrections and synonyms persisting across restarts;
placeholder round trips; `serve` and `stdio` smoke-tested. `spoon teach` on qwen3.5:4b
(6 lessons, 24s): 1/2 facts, 1/1 phrasings, 1/1 opinion, 1/1 concept, 0/1 capability (the
teacher miscounted its own examples, so no program fit); a steered run learned `square = mul(x, x)`.

Does not yet:
- bAbI probes: 1/21. No location state (`Mary moves to the bathroom.` then `Where is
  Mary?`), prepositional predicates do not parse or store, no three-argument relations
  (`gives X to Y`), no possession counting, story names not added to the gate lexicon,
  plural universals fail.
- ACE 158 messy corpus: 30/158 structural hits native-only, 53/158 (104 parsed)
  live with qwen3.5:4b; the parser alone covers 136/158 of the target SCE.
  Misses: reported speech 0/20, for-each variables 0/20, context/ellipsis 1/22.
- Ears LLM invents SCE from garbage (`zxqv flarp wibble` -> `Zxqv is a wibble.`);
  a garbage guard is queued. Mouth LLM can add stance the plan did not contain.
- Teacher on a 4B model drops JSON fields, miscounts examples and repeats the
  same lessons per prompt; failed lessons are not retried.

`PLAN.md` holds the locked decisions, architecture, milestones and predecessor lessons.
`STATUS.md` is the pick-up log, newest first; its top NEXT list is the queue. STATUS wins on conflict.

## Attributions

Seed data, the bAbI taxonomy, WordNet-derived lexicon entries and the normalizer prompt
lineage are recorded with sources and licenses in `ATTRIBUTIONS.md`. There is no LICENSE file yet.
