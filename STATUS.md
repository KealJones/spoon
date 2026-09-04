# Spoon v2: status log

Newest entry first. Each entry: DONE / IN PROGRESS / NEXT.
"Done" means it runs, not that a type exists.
Decisions in PIVOT_PLAN.md; rules in AGENTS.md; design in docs/CONCEPT-IR-DESIGN.md.

---

## 2026-09-03  The config was never read, and the bench never checked answers

Two findings that invalidate most of what this log said about the Teacher.

**The Teacher was never the model it was set to.** `~/.spoon/config.json` has
said `qwen3.8:27b` for a while, and Spoon had never opened the file. All three
seats ran the `qwen3.5:4b` default. The entry above, "the model in the seat is
not good enough for this class of problem", was measuring a model nobody chose.
Flags now override the file, the file overrides the default, and `database.path`
is refused rather than honored: it points at a v1 brain whose schema this build
cannot read.

**The bench never checked whether an answer was right.** It reported which ears
path ran and how many gaps appeared, which is why "it cannot answer anything"
stayed invisible while every number on the report looked healthy. Suites now
carry expected results and the runner scores them by category, grading the
result concept rather than the reply, since the mouth is a language model and
grading its prose measures the mouth's mood.

BASELINE  280/336 (83%) on held-out cases, native ears 42%.

DONE
- Graded bench with per-category scoring, `--no-teaching`, and setup turns.
- 3787 generated cases over 38 categories, split 70/30 train and test, with the
  split taken per category and deduplicated by utterance first.
- Reverse works on text, which also collapsed `reverse-text` from an
  eight-node synthesis target to three nodes. The test asserting enumeration
  could not reach it now asserts that it can. Nothing about the search improved.
- `sort` exists under the name people use.
- A stall is no longer reported as an answer, in three separate places: a
  half-reduced ask, a stuck `Move::Do`, and a value containing holes.
- The turn re-runs the interior once after learning something, so a turn that
  synthesizes exactly what it needs stops answering "unknown".
- Retiring a native retires everything that still names it: its realization,
  the phrasings that produce it, and the Teacher's durable advice about it.
  A brain with history failed where a fresh brain succeeded, because only the
  old one had the bad memories.
- A greeting is a whole turn or it is not one. "hey quick one, 356 minus 43"
  used to answer hello.
- The inspector shows a concept in full: kind, description, realizations with
  their bodies and scores, declared properties, and where it appears.

NEXT
- Run `scripts/overnight.sh`: baseline, curriculum, train with teaching on,
  score the held-out half with teaching off. The number that matters is whether
  training moves the test score off 83%.
- The ears leave words as bare concept names, so `ends-with<parallel, "el">`
  fails where `ends-with<"parallel", "el">` works. Teaching should fix this by
  storing phrasings; if it does not, that is the next real bug.
- Turns that produce no result at all: the ears returned zero steps.

---

## 2026-09-03  Correction: the Teacher result was the prompt talking to itself

An earlier entry claimed the Teacher, once fixed, produced
`join<reverse<chars<?0>>, "">` for reversing text. That was true and worthless.
The prompt contained

```
COMPOSE reverse-text = join<reverse<chars<?0>>, "">
```

as a few-shot example, so the model was handing back the answer it had just been
shown. Keal spotted it; the measurement was teaching to the test and should
never have been reported as a capability.

MEASURED PROPERLY
The give-away was replaced with unrelated examples (`average`, `longest`) and
the same question asked five times: **0 of 5 correct**. The format is fine, the
reasoning is not. What it actually produces is
`COMPOSE reverse = map<chars<?0>, upper>`: syntactically perfect and reverses
nothing.

So with a 4B model in the Teacher seat, string reversal is not learnable by
either route. Search cannot reach eight nodes and the Teacher cannot write them.

WHAT THIS DOES SAY
Verification works, and is the reason a wrong answer costs nothing. Every one of
those bodies fails the examples and is discarded, so the outcome is an honest
"could not do it" rather than a stored realization that quietly returns
nonsense. The pipeline is sound; the model in the seat is not good enough for
this class of problem.

The Teacher defaults to qwen3.5:4b, the same local model as the ears and the
mouth. `SPOON_TEACHER_URL`, `SPOON_TEACHER_KEY` and `SPOON_TEACHER_MODEL` point
it at a frontier model, and that is the experiment worth running before drawing
any conclusion about what the architecture can learn.

LESSON, worth keeping
A few-shot example that contains the answer to the evaluation makes the
evaluation meaningless. Anything measured against a prompt has to be measured
against a prompt that does not contain the answer, and the check for that is
mechanical: search the prompt for the expected output before trusting a result.

---

## 2026-09-03  The Teacher was broken in three ways, and the learning loop is one fix short

WHAT WAS WRONG WITH THE TEACHER
Asked how to build a missing capability, it replied with a synonym. Asked for
examples, it produced "malformed synonym". Three causes, all mine:

1. **There was no EXAMPLES reply form at all.** The prompt offered SYNONYM,
   CONCEPT, COMPOSE and UNKNOWN. `TeacherReply::Spec` was therefore
   unreachable, which means the synthesizer had never once been fed in the
   entire history of this system. Synthesis was tested and worked and was wired
   into the brain, and nothing could reach it.
2. **Every question got the same menu of four forms**, and a 4B model handed a
   menu reliably picks the cheapest item on it. Each ask now names the one form
   that answers it, with UNKNOWN as the only alternative.
3. **The Teacher was never told what concepts exist.** Asked to build string
   reversal it answered, correctly given what it knew, that no concept turns a
   string into a list, while `chars` sat in the store unmentioned. It now gets
   the same activation-ranked vocabulary the ears get.

With those fixed it produces exactly the right things:
```
COMPOSE  -> join<reverse<chars<?0>>, "">
EXAMPLES -> "ab" -> "ba" ; "hello" -> "olleh" ; "a" -> "a"
```

THE ONE REMAINING BREAK, stated precisely
The Teacher answers about a concept it names itself. Asked how to reverse a
string it proposes `reverse-text`, while the gap the interior actually hit is
`reverse` applied to text. The composition is correct and gets stored, and
nothing ever calls it, because no utterance produces `reverse-text`.

The fix is to bind the Teacher's answer to the concept that failed rather than
to the name it invented, which is the same name-reconciliation problem already
solved for the ears and not yet applied here. That is the next thing to do and
it is a small change.

A SECOND FINDING, from a run that did learn
Before the example minimum was raised, synthesis returned
`replace<"helloworld", "hello", ?0>` for string reversal: a body that fits two
examples perfectly and has learned nothing. Constants drawn from the examples
are what make `mul<?0, 3>` reachable, and the same mechanism lets a body
memorize. The minimum is now three examples.

An explicit guard rejecting bodies that embed an expected answer was written,
tried, and reverted: it broke `at-least-ten`, where the constant 10 is both a
needed constant and an expected output. The comment predicting that exact cost
was written before the test proved it, which is the useful part. More examples
is the honest fix; a cleverer guard needs evidence this one does not have.

ALSO IN THIS ROUND
- Gaps now include realizations that exist and fail on the given arguments, not
  only heads with no realization at all. `reverse` can reverse a list and was
  handed a string, which is a gap in what Spoon can do.
- A regression I introduced and fixed within the hour: treating any irreducible
  expression as a fact turned unknown capability requests into stored facts, so
  no gap was reported and the Teacher was never asked. Only the declarative
  meta-vocabulary counts now.

---

## 2026-09-03  Stages 4-7: it holds a conversation (437 tests, 0 warnings)

DONE, verified against a real brain with a real local model (qwen3.5:4b)
- `spoon-seat`: one HTTP client, three seats, separate counters.
- `spoon-ears`: native path first, model on a miss. Model gets an
  activation-ranked vocabulary, not the whole store.
- `spoon-mouth`: template is the reference, model makes it human, output is
  checked against the values it was told to keep.
- `spoon-teach`: proposes synonyms, concepts, and compositions. Never code.
- `spoon-brain`: the turn loop, episodes, and the resolver that turns a flat
  heard sequence into moves.
- `spoon`: repl, stdio, serve (OpenAI-compatible + inspector with a live chat
  tab), bench, doctor, export, import, status.

THE THESIS, DEMONSTRATED END TO END
```
> greg is friends with keal      noted: friends<greg, keal>
> friends is symmetric           noted: symmetric<friends>
> is keal friends with greg?     yes
```
Nobody stored that answer. It was derived from one fact and one declared
property, through the general meta-rule, from messy typed English.

Also working, model on, against ~/.spoon/spoon-v2.db:
- `whats 2 plus 3` -> 5, `calculate 12 times 4` -> 48
- `can u double 21 for me` -> 42
- `john has a dog` -> `noted: owns<john, dog>`, then `who owns a dog?` ->
  `list<owns<john, dog>>`, and still answered after a restart.
- 36-utterance bench of real messages from the design conversation: 34 of 36
  produced a sensible reading, 2 outright failures, `interior_model_calls` 0.

BUGS THE FIRST REAL RUN FOUND (all fixed)
1. Chat went through evaluation, so `greet<>` found no realization, logged a
   capability gap, and turned a hello into a report about what Spoon cannot do.
2. Names printed as hex. Ids are derived from names, so parsing teaches the
   store nothing about spelling, and the ears were throwing away the table they
   had just filled.
3. The mouth held a symbol table snapshot from startup, so anything learned
   mid-conversation was unreadable exactly when Spoon had just learned it.
4. The mouth padded replies with invented trivia that passed the value check
   because the value was still in there somewhere.

KNOWN LIMITATIONS, stated plainly
- **The ears invent vocabulary.** "friendship is symmetric" and "greg is
  friends with keal" produced `symmetric<friendship>` and `friends<greg, keal>`,
  which do not connect. The meta-rule is fine; the names disagree. This is
  exactly what `Synonym` and the Teacher are for and neither is wired into the
  ears' naming yet. It is the single biggest gap.
- **No synthesis.** Stage 5's learn-from-examples is not built. The Teacher can
  hand back a composition and it is stored, but Spoon cannot yet infer a
  capability from worked examples.
- **No consolidation.** Repeated structure is never promoted to a named
  abstraction, so the library does not compress.
- **No credit assignment.** Episodes record what happened, including which
  realization failed, but nothing reads them to apportion blame yet.
- **Corrections are not wired.** `Episode::correction` exists and is always
  None; saying "no, wrong" does not yet retract anything.
- **The native ears are thin.** Social and arithmetic only, so the weaning
  number sits at 14 percent and will not move until phrasing induction exists.
- **Left-recursive rules under-answer.** Sound, not complete. See the Stage 3
  entry.

NEXT, in the order that would matter most
1. Wire `Synonym` into the ears so an invented name resolves to an existing
   concept instead of forking the vocabulary.
2. Corrections: retract, re-store, and update realization evidence.
3. Synthesis from examples, which is what makes the Teacher's specs useful.
4. Phrasing induction, so accepted model readings become native ones.
5. Consolidation.

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
