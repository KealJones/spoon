# Spoon: agent instructions

Read in this order before doing anything: `STATUS.md` (what actually works,
newest entry first), `PLAN.md` (decisions, architecture, milestones, lessons),
`docs/SCE.md` (the controlled language contract), then the `types/` module in
`crates/spoon-core`.

## Hard rules

1. No LLM in the interior. Only three seats exist: `Seat::Ears`, `Seat::Mouth`,
   `Seat::Teacher` (see `crates/spoon-core/src/llm.rs`). Planning, choosing,
   executing, learning, and answering are Spoon code. `TurnMetrics.interior_llm_calls`
   must stay 0 and is asserted in tests.
2. Wire or delete. A module counts as done only when a REPL turn exercises it.
   Update `STATUS.md` when that happens, not before.
3. No god files. Keep modules under ~800 lines. The turn loop is a short
   orchestrator in `crates/spoon-mind/src/brain.rs`.
4. Teacher never writes executable bodies by default. It writes specs,
   examples, phrasings, concept models. The synthesizer writes programs.
5. Search budgets are nodes + time + memory. Never depth alone.
6. Types in `crates/spoon-core/src/types/` are the contract between modules.
   Changing them is an orchestrator decision; subagents do not edit them.
7. Every learned thing must survive a restart (SQLite) and be exportable
   (`spoon export`). Test with an open/close/open cycle.
8. No em-dashes in any text (code comments, docs, prompts, replies).
9. Third-party crates for plumbing only (parsing, storage, HTTP, search).
   Never for the choosing.

## Layout

```
crates/spoon-core   types, Can index, kernel (IR eval + primitives), Store, LlmClient
crates/spoon-lang   sce (grammar, Earley parser, realizer), ears, mouth
crates/spoon-mind   discourse, plan (planner, executor), grow (synth, consolidate), teacher, brain (orchestrator)
crates/spoon        bin: repl | stdio | serve | teach | bench | export | import; axum server; inspector
docs/               SCE.md, design notes
data/               seeds, curricula, benches (attributed in ATTRIBUTIONS.md)
```

## Commands

```
cargo check                       fast compile
cargo test --workspace            all tests
cargo run -p spoon -- repl        talk to it
cargo run -p spoon -- serve       OpenAI API on :8787
cargo run -p spoon -- bench ace   ears eval on the 158-item messy corpus
```

## Subagent protocol

Subagents get: exact files they own, the types they code against (read-only),
tests they must make pass, and a ban on touching other modules. They are told
they are subagents and must not re-delegate. Prefer Sonnet. Each prompt is
self-contained: the subagent has no access to this conversation.

## Pick-up procedure

1. `cargo test --workspace` and note failures.
2. Read the top entry of `STATUS.md`; its NEXT list is the queue.
3. Run `cargo run -p spoon -- repl` and try the STATUS demo lines. If they do
   not work, STATUS is wrong; fix STATUS first.
4. Continue the milestone. Append a STATUS entry when something is wired.

## Reference material

- `~/Git/Personal/ekg/tasks/spoon-from-scratch-v2.md`: Keal's own brief. Authoritative on intent.
- `/tmp/ekg-ai` (re-clone from github.com/kealjones/ekg-ai): seed lexemes, WordNet subset, EKGBench.
- `~/Downloads/ace_normalizer_spike`: 158-item messy-English corpus, APE landmine list, normalizer prompt that scored 116/152 on qwen3.5:4b.
- Viv patents US20140380268 (planning), US9292262B2 (NL); Bixby modeling docs.
