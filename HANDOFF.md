# Handoff

Written 2026-09-04, mid-session. Read this, then `STATUS.md` (top entry), then
`AGENTS.md`. Everything below is committed.

## What happened tonight

Two findings invalidated most of what `STATUS.md` used to claim:

1. **`~/.spoon/config.json` was never read.** It has said `qwen3.8:27b` for a
   while; all three seats ran the `qwen3.5:4b` default. Every earlier note
   about the Teacher being too weak was measuring a model nobody chose.
2. **The bench never checked whether an answer was right.** It reported which
   ears path ran and how many gaps appeared. That is why "it cannot answer
   anything" stayed invisible while every number on the report looked healthy.

Fixing (2) turned into the evening's work: a graded bench, a generated corpus,
and then a queue of real bugs that the grading exposed.

## The run that is going right now

```bash
scripts/overnight.sh          # or check target/overnight/full/
```

Six steps on a fresh brain, in this order:

| step | suite | teaching | why |
|---|---|---|---|
| `baseline-test` | `graded_test` (1153) | off | what Spoon does untrained |
| `teach` | `data/curriculum/basics.json` | on | 17 lessons |
| `train` | `graded_train` (2634) | on | this is the training |
| `train-facts` | `graded_facts_train` (564) | on | relations |
| `trained-test` | `graded_test` (1153) | off | **the number that matters** |
| `trained-facts` | `graded_facts_test` (564) | off | a cast it never met |

State when this was written: `baseline-test` at 507/1153, ~87% right,
~2.5s/case. Expect the whole thing to take about seven hours.

**The binary is pinned.** Each step execs `spoon` fresh, so editing the source
mid-run would score the trained half with different code than the baseline
half. `scripts/overnight.sh` copies the binary into the output directory and
records the commit in `target/overnight/full/commit`. Source edits during a run
are safe; **editing anything under `data/bench/` is not**, because the pinned
binary reads those files from disk at each step.

If the run died, just start it again. Nothing depends on the old one.

## Read the results like this

```bash
sed -n '/== summary/,$p' target/overnight/full.out
grep -E '^[0-9]+x ' target/overnight/full/trained-test.log   # the wrong ones
diff <(grep -E '^[0-9]+x ' target/overnight/full/baseline-test.log) \
     <(grep -E '^[0-9]+x ' target/overnight/full/trained-test.log)
./target/debug/spoon status --db target/overnight/full/brain.db
```

`status` reports phrasing health: how many were used, how they fared, how many
fell below half. That is the number to watch during training.

A case is scored on the **rendered result concept**, not the reply. The mouth
is a language model, so grading its prose measures the mouth's mood.

## Bugs fixed tonight, roughly in order of how much they mattered

Each of these has a commit message that explains it properly; `git log` is
worth reading rather than trusting this table.

- **Phrasings could not generalize over words.** Only numbers, quoted strings
  and capitalised names became slots, so "make COMMITTEE lowercase" taught
  Spoon nothing about "make REALIZATION lowercase". Arithmetic generalized
  because arithmetic is numbers; nothing word-shaped did. This is why training
  kept failing to transfer. The steps-linkage check is what keeps it safe:
  "reverse" in "reverse banana" stays a literal because the reading holds it
  as a head, not as text.
- **A question became an assertion.** "is carol friends with frank" replied
  "noted" and wrote a fact the speaker had only asked about. The facts suite
  scored 0/6; it now scores 6/6 on a cast it never met with teaching off.
  Inference was never broken. English marks a yes-or-no question by moving the
  verb to the front, which needs no model to see.
- **`record_pair_outcome` was never called outside tests.** The whole
  Laplace-smoothed demotion apparatus existed, unfed, so a phrasing that
  generalized badly fired forever and training could make the ears more
  confident but never better.
- **Phrasings were learned from assertions.** An assertion always succeeds
  whatever it asserts, so every misread statement became a stored phrasing.
  A reading is now learned only when the turn produced an answer.
- **Entity names crowded the verbs out of the ears prompt.** After training
  mentioned a few dozen people they outranked the operations on recency and
  pushed them past the 120-name limit, and sentences that worked an hour
  earlier stopped working. Capabilities now come first.
- **A bare word handed to a native is a word.** `reverse<kubernetes>` failed
  because the native wanted text. Largest single group of wrong answers.
  `owns<john, dog>` is untouched: nothing realizes `owns`, so those are
  entities.
- **Retiring a native left everything that named it in place.** `count-matching`
  was renamed away in code; the realization, the phrasing that produced it, and
  the Teacher's durable advice about it all survived, so a brain with history
  failed where a fresh brain succeeded.
- **A stall was reported as an answer**, in three places: a half-reduced ask, a
  stuck `Move::Do`, and a value still containing holes.
- **Elliptical repairs.** "reverse banana, actually no, possession" replaces
  one word and leaves the verb implied. Spliced by token count, chains recurse,
  and a mid-sentence repair no longer retracts the previous turn.
- **`reverse` now works on text.** Side effect worth knowing: `reverse-text`
  went from an eight-node synthesis target that four hundred thousand
  candidates could not reach to three nodes. The test asserting enumeration
  could not reach it now asserts that it can. The search did not improve.
- **The teacher fired on every model-read turn.** Fine at 4B, ten to thirty
  seconds at 27b. Now: always when the turn visibly went wrong, one in eight
  otherwise.
- **`sort` did not exist** under the name anyone uses.
- **Native docs did not say what shape comes back.** They are the ears prompt.
  "the first 3 letters of parallel" came back as `list<"p", "a", "r">` because
  `substring` described itself as "the characters of a string", which reads
  like `chars`.

## The corpus

`scripts/make_corpus.py` generates everything under `data/bench/`. Seeded, so
a corpus that finds a regression can be bisected. `SCALE` tunes the size
(default 3, about 3800 cases; `SCALE=5` gives roughly fifteen thousand, though
surface forms repeat before the count does and the duplicates get removed).

Deduplicated by utterance and split 70/30 **within each category**, so both
halves cover the same ground and no test sentence is in the training set.

Includes `messy` (openers, trailing filler, no capitals: "ok wait whats 47
times 8 lol") and `corrections`, where answering the first thing said is the
failure.

Suite formats: `cases` is graded, `lines` is the old ungraded shape and still
runs. A case may set `"setup": true` to be run but not scored, for questions
that only mean something after something else was said.

## What to do next

1. **Wait for the run and read `trained-test` against `baseline-test`.** If
   training does not move the score, the phrasing path is still not
   transferring and that is the thing to chase. Do not chase small differences
   on small suites: twelve cases cannot tell 83% from 67%, and several hours
   went into a two-case swing that turned out to be three separate causes.
2. Regenerate the corpus and run again, since three fixes landed after the
   binary was pinned and are not reflected in tonight's numbers: the bare-word
   coercion, the greeting guard, and the sharpened native docs.
3. Known and unfixed, from reading `baseline-test.log`:
   - `how many numbers are in 49, 5, 37, 47, 47, 4` answers `189`. It sums
     instead of counting.
   - `reverse the list 38, 35, 12, 1, 37` produces no steps at all.
   - `split keyboard,banana,science on commas` reaches `cannot-yet`.
   - `the numbers from 1 to 5` may be off by one at the reading, not in
     `range`.
   - The ears sometimes pick `or` where the sentence says "and".
4. `spoon doctor` prints gap concepts as raw JSON instead of rendering them.

## Ground rules that bit me

- Never edit `data/bench/` while a run is going.
- `SPOON_SCRATCH=path` gives a throwaway brain. Use it. The brain at
  `~/.spoon/spoon-v2.db` belongs to Keal and I destroyed it once already.
- `--no-teaching` keeps the ears and mouth and silences the Teacher. Use it
  when measuring whether an edit helped, because teaching both slows a run and
  changes the brain underneath it.
- Check a suspicion against the episode before theorising. `ears_path` in the
  episode is what separated "a learned phrasing hijacked this" from "the model
  got worse", and I guessed wrong twice before looking.
