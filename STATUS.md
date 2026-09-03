# Spoon v2: status log

Newest entry first. Each entry: DONE / IN PROGRESS / NEXT.
"Done" means it runs, not that a type exists.
Decisions in PIVOT_PLAN.md; rules in AGENTS.md; design in docs/CONCEPT-IR-DESIGN.md.

---

## 2026-09-03  Stage 1 complete: concept substrate and store (194 tests, 0 warnings)

DONE, all wired and exercised through tests
- `spoon-concept`: the whole representation. `Concept` in three shapes,
  `ConceptId` split into `Named`/`Ground`, stable blake3 `ContentId`, symbol
  interning, `Realization`/`Activation`/`Provenance`/`Tier`/`Effect`.
- `spoon-concept::ops`: traversal, `Path` addressing, `Bindings`, substitution,
  `alpha_equivalent`, `generalizes` (one-way matching), `anti_unify` (Plotkin
  least general generalization). Unchanged subtrees reuse their `Arc`, asserted
  with `ptr_eq` rather than assumed.
- `spoon-concept::text`: the angle-bracket notation, render and parse, exact
  inverses. Grammar in `text/mod.rs`.
- `spoon-store`: SQLite with a real migration runner, participant index,
  bi-temporal assertions, concept metadata with a derived surface-form index,
  realizations, symbol names, deterministic seed export/import.

THE CORE PROPERTY HOLDS, with a test named after it
- `Add<42, 1>` writes zero rows for its literals. `put_meta` or an assertion
  about `42` materializes exactly one. Ground values inside a stored compound
  are still participant-indexed, so `concepts_containing(160)` finds the pull
  request without 160 owning a row.

BUGS FOUND DURING REVIEW (all fixed)
1. `JsonBlob` had `#[serde(skip)]` on its digest, so any `Ground::Json`
   deserialized with a zeroed digest and a different `content_id`. Every JSON
   concept in a reloaded brain would have stopped matching itself. Now
   serializes as the bare `Value` and rebuilds the digest on the way in.
2. `Ground::Float` could not survive JSON at all: `serde_json` writes NaN and
   the infinities as `null`, which then refuses to read back as `f64`. A
   concept carrying one was writable and permanently unreadable. Non-finite
   values now encode as `"NaN"` / `"inf"` / `"-inf"`; finite ones stay bare
   numbers so seeds remain readable. The store's defensive write-time rejection
   was removed once the contract was correct.
3. The store lowercased symbol names on write and used last-write-wins, while
   `SymbolTable::intern` preserves casing and is first-write-wins. The same
   brain printed a symbol differently before and after a reload. Aligned.

GOTCHA worth remembering
- `SymbolId::of` lowercases and trims but does not touch separators, because
  collapsing them would merge `co-op` with `coop`. So `FriendWith` and
  `friend-with` are two different concepts. Kebab-case is the convention for
  code and seeds; PascalCase in prose is readability only. Recorded in
  AGENTS.md.

TESTS: 194 passing. 153 in `spoon-concept` (40 identity contract, 43 ops, 19
orchestrator property checks, 35 text, 14 text property checks, 2 doctests),
41 in `spoon-store`. The property suites were written independently of the
implementations to check the laws the rest of the system leans on, not to
re-run the implementer's own cases.

NEXT (Stage 2: evaluator)
Contract is written: `docs/EVALUATION.md`. It settles budgets (nodes + time +
depth together), the selection score with explicit exploration, effect
authority that cannot be laundered through composition, per-turn caching gated
on purity, and the evaluation trace credit assignment needs. Three questions
are deliberately left open in section 10 for the implementer to settle and
record: normalization order, how context is represented for `context_fit`, and
whether exploratory failures should be weighted less.

1. `spoon-eval`: the evaluator core, `Outcome`, budgets, the native registry.
2. Bootstrap natives: arithmetic, comparison, collections, text, logic, store
   ops, IO. Roughly 50 to 100 concepts, each a concept with a Native
   realization, none of them privileged.
3. Realization selection with evidence and exploration.
4. `spoon-infer`: unification and rule application on top of the same loop.

---

## 2026-09-03  Architecture pivot begins

DONE
- `PIVOT_PLAN.md`: 7-stage re-architecture from rigid CAN to a unified Concept
  model.
- `docs/CONCEPT-IR-DESIGN.md`: core design - one Concept type in three shapes
  (atomic / compound / hole), evaluation as term rewriting, inference through
  meta-concept rules, LLM ears producing concept sequences, self-ranking
  vocabulary.
- `AGENTS.md`: rewritten for v2 architecture.
- `docs/PRIOR_ART_MEMO.md`: trimmed to algorithms relevant to v2.
- Removed: old STATUS.md (v1 log), PLAN.md (v1 decisions), docs/SCE.md
  (controlled English - replaced by concept-based ears).

DESIGN DECISION (settled after going back and forth)
- There is no separate "Atom" type. Everything is a `Concept`. Atomic and
  compound are shapes a concept takes, not different categories. The earlier
  4-variant sketch with a `Val` variant was a privileged primitive and got cut.
- Ground values (numbers, strings, bools, json) are atomic concepts whose
  identity IS their value: `Atomic(Ground(Int(42)))`. Self-describing, so no
  store row is needed until something is asserted about them. `Add<42, 1>` hits
  the store zero times; `Synonym<"forty-two", 42>` materializes a row for 42.
  This is how `#160` can become a real PullRequest concept without every integer
  in every computation earning a database entry.
- `Hole` is the only shape that is not a concept: a gap inside rule patterns.

v1 code in `crates/` remains as reference. It is not the active codebase.

NEXT (Stage 1: Concept Substrate)
1. New crate `spoon-concept`: Concept enum, ConceptId (Named/Ground), Ground,
   HoleId, symbol interning, structural equality + hashing.
2. New crate `spoon-store`: Store trait, SQLite impl, multi-index
   (head, participant, structure hash, time), lazy materialization for ground
   concepts.
3. Bootstrap: seed ~50 core concepts (arithmetic, comparison, collections,
   text, logic, store ops) with Native realizations.
4. Tests: insert/query/round-trip/restart-survival; verify ground concepts cost
   zero rows until asserted about.
5. Export/import for the new concept format.
