# Retrieval, recall, and language wiring - state of play

Last updated: 2026-08-29. Branch: `main`. Everything below is uncommitted.

## Landed this session

All ten of these are working and the test suites were green as of the last full
run. None of it is committed.

1. **Recall fast path.** Two separate bugs kept the Recall rung from ever
   firing. Episode lookup now matches the situation exactly through an indexed
   `find_by_situation` query, replacing a fuzzy-ranker prefilter that pushed the
   exact hit out of the window once the corpus passed ~260 episodes. Separately,
   trust digests canonicalize JSON before hashing, because `f32` confidence
   values were not round-trip stable through `serde_json` and every receipt was
   failing validation by one unit in the last place.
2. **Promotion path.** A `Provisional` procedure reaches `Active` after three
   clean executions with no recorded failures, via `ProcedureOutcomeCounts` and
   `promote_procedure_on_earned_evidence`. Recall was structurally dead before
   this, since it only accepts `Active` or `Validated`.
3. **Cycle ownership.** Removed the blanket `recover_pending_cycles` startup
   sweep, which let any newly opened engine seize live cycles belonging to
   another instance. Cycles are now adopted lazily by `adopt_pending_cycle` at
   the resume points.
4. **Interpreter timeout.** Ollama calls stream, carry `keep_alive: 30m`, and go
   over a `node:http` client instead of undici, whose 300 second header timeout
   was killing every slow generation.
5. **Interpreter prompt size.** Dropped the duplicated output schema and the
   unused procedure catalog, taking the prompt from about 120KB to about 32KB.
   The schema still constrains decoding through the request's `format` field.
   Sections are ordered stable-first so the KV cache can be reused across turns.
6. **Interpreter model.** `qwen3:30b-a3b`, aligned across `.env`,
   `.spoon/config.json`, and `~/.spoon/config.json`. Precedence is shell
   environment, then `.env`, then config files.
7. **Discriminator validation.** A discriminator is now satisfied by a feature
   demonstrated in observed-fact scope, not only in `context.environment`. This
   was the forge regression that the promotion work exposed.
8. **Kind-scoped retrieval.** `rank_of_kind` filters on document kind inside the
   SQL, so episodes can no longer consume the whole candidate window before any
   procedure is considered.
9. **Deterministic local resolution.** An unambiguous utterance resolves and
   executes without consulting the interpreter.
10. **Procedure search text.** Stopped Debug-printing params, contract, and body
    into the index. `Param`, `Some`, `BinOp`, and `Condition` were tokens shared
    by every procedure in the corpus, and length normalization meant they
    diluted the words a person would actually search for.

## In flight

**FTS5 and BM25 retrieval.** Designed, no code written yet.

- External content table over `recall_documents.rowid`, which exists implicitly
  because the table has a `TEXT PRIMARY KEY`.
- `porter unicode61` tokenizer, replacing the hand-rolled 3/4/5-character prefix
  postings in `tokenize`. Those bridge inflections crudely and produce false
  matches, since `count` and `country` share `cou`, `coun`, and `count`.
- Triggers for insert, delete, and update to keep the index in sync. FTS5 does
  not auto-sync external content. Guard the update trigger with
  `WHEN old.text <> new.text` so retrieval-count bumps do not reindex.
- `bm25()` returns negative values where more negative is better, so order
  ascending. Column weights let a name hit outrank a body hit.
- Existing databases need a one-time `INSERT INTO recall_fts(recall_fts)
  VALUES('rebuild')`. Note that `SELECT count(*)` on an external content table
  reads the content table, so it cannot be used to detect an empty index. Track
  the rebuild with an explicit marker instead.

## Queued

- **Embeddings.** `qwen3-embedding:0.6b` is pulled: 639MB, 1024 dims, 32K
  context, 70.7 on MTEB-eng-v2. Fill `vector_json` with real vectors, cosine
  rerank, fuse with BM25 through reciprocal rank fusion. The engine computes the
  vectors and passes them in, so `spoon-intuition` stays free of network I/O.
  Brute-force cosine over a few thousand documents is sub-millisecond, so no
  `sqlite-vec` dependency is needed. Falls back to BM25 alone when no embedder
  is configured.
- **Wire `admit_language_writes`** so `alias-of`, `termed`, and `intent-of`
  actually get written. Approved.
- **Teacher relationship kind gating.** Paused pending discussion. See below.
- **PRF expansion.** `unmodified_semantic_features` seeds from `ORDER BY
  document_id ASC LIMIT 256`, which picks arbitrary documents rather than
  relevant ones, and ranks expansion terms by raw document frequency with no
  IDF, so it adds "what", "is", and "the". Either rebuild it as a real RM3
  seeded from the top BM25 hits and weighted by IDF, or delete it. Today it
  causes query drift.
- **Signature and arity fit** as a structured ranking signal. Probably belongs
  in `cycle.rs`, which already holds the `Procedure` structs, rather than in the
  ranker, which only sees text.
- **Unify teacher and interpreter prompts.** `SPOONLANG_GRAMMAR`, the system
  prompts, and the intrinsics list are duplicated across Rust and TypeScript.

## Open findings, not yet work items

- `what is double 44?` answers 176 on the real database. Deterministic
  resolution is not firing the way it was intended and this has not been chased
  down. Suspicion is that `procedure_has_language_support` is permissive enough
  that several procedures look executable, which trips the ambiguity escalation.
- `embed()` is a hashing-trick bag of words over 64 buckets. `vector_json` is
  written on every index and read by nothing.
- The graph holds 24 relationships across 15 distinct kinds, nearly all
  singletons. It is not usable for query expansion in its current state.
- Everything is uncommitted on `main` across 20 modified files plus several new
  ones. This is one undifferentiated change set and should be split.

## The paused discussion: relationship kinds

There are two doors into the `relationships` table and only one is guarded.

The guarded door is `LanguageWriteKind` in `crates/spoon-core/src/utterance.rs`,
a closed set of `AliasOf`, `Termed`, and `IntentOf` mapping to `alias-of`,
`termed`, and `intent-of`. Its own doc comment says core stores the kind as a
free string, so this set is what the engine admits and anything outside it is
refused. There is a full admission layer around it: `admit_language_writes`,
`is_admissible_kind`, span validation, a per-utterance write cap, and a rule
that an `IntentOf` write only lands if something actually executed.

That door has never opened. `admit_language_writes` and `is_admissible_kind`
have zero callers outside `crates/spoon-engine/tests/admission.rs`. The
`language_cycle` module is declared in `lib.rs` and nothing in the live cycle
calls it. The database has zero rows across all three kinds. This is the same
failure mode as the composition gate: built, unit-tested, never wired.

The unguarded door is the teacher lesson path at `crates/spoon-engine/src/cycle.rs:2211`,
which passes `relationship_draft.kind` straight to `Relationship::new` with no
allowlist check. The teacher invents a fresh kind per lesson, which is where
`folds_with`, `complements`, `extracts_text_from`, and `computes` came from.

The fact that makes this decision tractable: almost nothing reads
`relationship.kind` for reasoning. `relationship_dependency_direction` uses it
for dependency ordering and `reconciliation.rs` filters on a matching
`proof_kind`. Everything else only stores and displays it. So a kind that no
consumer understands is inert data, and an open vocabulary is only worth its
cost if something is going to learn to read it.

## Why synonyms need more than BM25

BM25 is purely lexical, so "4 twice" will not reach "double" no matter how well
the weighting is tuned. Three mechanisms could bridge that, and they compose
rather than compete.

Embeddings know English on day one but know nothing about this user. Alias edges
are evidence-grounded, inspectable, and get more precise with use, but start
empty. Outcome mining over `recall_ranking_examples`, which already records
query, candidate, whether it was used, and whether it succeeded, is the most
project-native of the three and has a natural home in the existing
`learned_score` field. Once alias edges exist the graph finally becomes worth
expanding a query through, which it is not today.
