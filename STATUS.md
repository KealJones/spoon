# Spoon v2: status log

Newest entry first. Each entry: DONE / IN PROGRESS / NEXT.
"Done" means it runs, not that a type exists.
Decisions in PIVOT_PLAN.md; rules in AGENTS.md; design in docs/CONCEPT-IR-DESIGN.md.

---

## 2026-09-03  Stage 2 complete: evaluator and 98 bootstrap natives (380 tests, 0 warnings)

DONE
- `spoon-eval`: the evaluation loop, budgets, realization selection with
  exploration, effect authority, per-turn purity-gated caching, and the trace.
  Written by the orchestrator; it is the architectural heart.
- `spoon-natives`: 98 bootstrap concepts across arithmetic, logic, collections,
  text, store access, JSON, and time. None privileged: each is an ordinary
  concept that ships with a Native realization.
- `seed_bootstrap` stores a realization plus metadata for every registered
  native, so a fresh brain can actually reach them. Registering code is not the
  same as the concept existing.

HIGHER-ORDER FUNCTIONS NEED NO MACHINERY, which is the payoff of one
representation
- `Map`, `Filter`, `Reduce`, `SortBy`, `Find`, `All`, `Any`, `GroupBy` apply
  their function argument by building `Concept::apply(f, args)` and handing it
  back to the evaluator.
- `Map<List<1,2,3>, double>` where `double` is a stored `Composed` body of
  `Add<Hole(0), Hole(0)>` gives `List<2,4,6>`. `triple` built on `double` maps
  too. Nothing in `Map` knows the difference between a native and something
  Spoon learned yesterday.

THE THREE OPEN QUESTIONS ARE SETTLED (recorded in docs/EVALUATION.md section 10)
- Outermost-first rewriting. Innermost cannot express a conditional at all:
  `If<true, 7, Boom<>>` would reduce the branch it never takes.
- Context is an explicit list of situation concepts matched against stored
  `WorksWellWith` / `WorksPoorlyWith` claims, so contextual fit is learned.
- Exploratory failures count at full weight. The trace marks which applications
  were exploratory, so the data to revisit it exists.

FURTHER DECISIONS WORTH REMEMBERING
- No realization is not an error. `FriendWith<Greg, Keal>` is a fact and
  reduces to itself; `Height<Add<1,2>>` becomes `Height<3>`. `Outcome::Stuck`
  is reserved for realizations existing and all of them failing.
- Missing machinery excludes a realization from selection rather than failing
  it at apply time, so a brain with no LLM seat is coherent and the
  alternatives still get their turn.
- Effect is the maximum of the realization's claim and its native's
  declaration. There is a test for the attack: a `Pure` claim over a
  network-touching native is still gated.
- Evidence is committed explicitly via `commit_evidence()`. A speculative
  evaluation that gets thrown away teaches Spoon nothing.
- Int and Float are distinct identities, so arithmetic widens rather than
  conflating. Int/Int comparison stays in i64: past 2^53 an f64 cannot separate
  adjacent integers and `Lt` would quietly answer false for two different
  numbers.

OPEN, worth revisiting when a call site pushes back
- `index-of` and `find` error when nothing matches rather than returning a
  sentinel. Defensible (a sentinel is a value the caller can forget to check)
  but it may force awkward double traversal. There is no `Maybe` concept yet;
  introducing one is a design decision, not a patch.
- `to-text` refuses named concepts, since a name's meaning lives in the store
  rather than in its spelling. The mouth may want a different answer.

TESTS: 380 passing. 153 in `spoon-concept`, 41 in `spoon-store`, 24 in
`spoon-eval`, 162 in `spoon-natives`. 63 of those are orchestrator property
suites written against the laws rather than the implementations, including the
one that matters most here: a write buried inside `Map` is still gated, so the
permission layer cannot be laundered through a higher-order call.

NEXT (Stage 3: inference)
The evaluator already applies `Rule` realizations forward when the pattern
matches the concept in hand. Stage 3 is the backward direction and the
machinery it needs.
1. `spoon-infer`: full two-way unification with an occurs check, and a
   discrimination-tree index so rule lookup does not scan.
2. Backward chaining: when a query has no direct assertion, collect rules whose
   `produce` could yield the shape and try them, sharing the evaluator budget.
3. Bootstrap meta-concepts: `Symmetric`, `InverseOf`, `TransitiveClosure`,
   `DefaultExpectation` (defeasible: direct evidence about Greg beats an
   inherited expectation about people), `Synonym`.
4. Cycle handling is already in place via the in-progress goal set; confirm it
   holds for backward chains too.

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
