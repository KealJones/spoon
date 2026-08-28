# Front Language Analysis

Date: 2026-08-28

Status: Draft for review

## Goal

Put a small language model at the front of the Spoon cycle so every utterance is segmented, cleaned, and annotated before lexical match or Teacher. Multi-part speech acts are dispatched independently, replies concatenate instead of dropping halves of the utterance, and successful language structure is admitted so the model can be weaned.

The model is a language teacher. It proposes knowledge. The Engine admits it. Model JSON is never knowledge by itself, never authority, and never execution.

## Non-goals

- Replacing the original utterance with cleaned text
- Mutating cleaned text after a part executes
- Letting the front model author new executable procedures
- Dumping Wikidata, ConceptNet, or MASSIVE wholesale into the graph
- Compact reply folding (`Hey it's 8`) as the default renderer

## Cycle

```text
utterance
  -> tokenize original (ground truth)
  -> Engine LanguageContextPacket
       same-session turns, catalog aliases, terminology, safe env
  -> Front Language Interpreter          always first when enabled
       UtteranceAnalysis proposal
  -> ground + validate
  -> per-part dispatch (toposort by derived depends_on)
       known intent key -> catalog -> execute
       dialogue act with response procedure -> execute
       unknown executable part -> Teacher for that part only
  -> compose ResponsePlan fragments in source order
  -> admit language writes (Provisional)
```

When the interpreter is disabled, the existing local-match / compose / Teacher path remains.

The interpreter may request one supplemental-context round against allowlisted packet operations. It cannot query the graph, invent durable IDs, or pull arbitrary episodes.

## Trusted IR

Original `TokenStream` is the only source of provenance. Cleaned text is a derived aligned document. Every cleaned token maps back to original byte spans. Downstream never treats the rewrite as source.

```text
UtteranceAnalysis
  original: TokenStream
  cleaned: AlignedDocument
  parts: [Part]
  language_writes: [LanguageWrite]

Part
  id: part_0
  spans: original token ranges
  template: "Hello {e0}. What is {v0}?"
  mentions: [Mention]
  context_bindings: [Mention]     // from packet, inferred=true
  intent: IntentFrameSet          // Execute | Clarify | Abstain for THIS part
  residual: [ResidualClaim]       // facts/conditions not executable this turn

Mention
  key: e0 | v0 | x0
  kind: entity | value | expression | result
  surface: original token ranges  // empty if fully inferred
  inferred: bool
  resolved:
    literal     { value }
    part_ref    { part, role: mention | result }
    context_ref { packet path }
    unresolved  { ambiguity }
```

Competing hypotheses live inside a part. Parts are sequential speech acts, not alternate meanings of the whole utterance.

`IntentFrame`, `IntentFrameSet`, token-index proposals, and span grounding stay as they are today. The new grain is `UtteranceAnalysis` wrapping per-part frame sets.

## Part dependencies

Two resolutions:

**Available now.** Surface mentions in other parts, or prior-turn material already in the packet, are filled into cleaned text and mention values at analysis time. `"it"` becomes `"the file from part_0"` or `"Pierre (from last turn)"`. No graph IDs. If the referent is not in the packet, the mention stays `unresolved` and that part Clarifies or uses the one supplemental-context round.

**Not computed yet.** `"what's 2+2, now double that"` does not rewrite cleaned to contain `4`. Bind `part_ref { part: p1, role: result }`. Engine derives `depends_on` from those refs, rejects cycles, toposorts, runs the producer, then binds the consumer.

Independent parts may run in any order. Reply concat always follows source order.

A part that Clarifies or Abstains does not run. Dependents of that part do not run. Independent siblings still do.

## Episode storage

Never replace the original situation. Never mutate cleaned after dispatch.

Each episode stores:

- `situation` — original utterance
- `analysis.cleaned` — analysis-time rewrite, placeholders intact
- `analysis.parts[]` — spans, template, mentions, intent, result
- rendered reply as the ResponsePlan output

Bound results live on part outcomes and `ObservedFact`s, not in cleaned text.

## Response composition

Execution deps do not drop answers. User-facing parts render in source order.

`"hey whats 2+2 and then double that"` renders:

`Hey. 2 + 2 is 4. Double that is 8.`

Compact folding is a later `ResponsePlan` variant, not v1 default, and is never the only stored answer.

## Graph contribution

No new tables for parts, cleaned text, or templates. Those are episode IR.

Existing types stay: Concept, Relationship, Procedure. Add a closed set of language relationship kinds:

| kind | meaning |
|---|---|
| `alias-of` | surface form for an existing term, including paraphrases |
| `termed` | phrase names a concept |
| `intent-of` | semantic key for a concept/procedure |

Admission rules:

- Model proposes names and request-local aliases. Engine mints IDs.
- New surface form for a known concept: `alias-of` or `termed`, Provisional, this episode as evidence.
- New intent key that resolved to an existing catalog procedure: catalog observation plus `intent-of`, only after that part actually executed.
- New entity with no matching concept: Particular concept, Provisional.
- Residual world facts: existing `ObservedFact` / Particular path. Not a fake constant procedure.
- New executable behavior: Teacher `reusable_lesson` for that part only. The front model does not author procedure bodies.

## Grammar and Intent Catalog

Every part carries a template with placeholders keyed to mentions. That is required analysis structure.

Successful part dispatch also writes a **surface pattern** onto the Intent Catalog entry, not a graph node:

```text
intent key: arithmetic.multiply
  slots: [v0, v1]
  patterns:
    "what is {v0} times {v1}"
    "multiply {v0} and {v1}"
```

Admit only after successful dispatch. Dedup by normalized placeholder skeleton. Cap patterns per key. Lifecycle starts Provisional.

`alias-of` covers words. Catalog patterns cover slotty phrases. That is the weaning fuel for a later local matcher.

The catalog remains Engine data: semantic key -> slot schema -> local concept -> exact procedure version. Training and import artifacts use stable keys such as `arithmetic.multiply` or `verbnet.give-13.1`, never database UUIDs.

## v1 public seed import

v1 consumes existing linguistic resources as catalog/alias seed. Mapping onto local procedures is bounded. The files are not dumped into the graph.

### VerbNet syntactic frames

Import VerbNet 3.4 class frames as unbound Intent Catalog rows:

- semantic key `verbnet.<class_id>` (example: `verbnet.give-13.1`)
- slot names from the class thematic roles
- `surface_pattern`s from the class syntactic frames, rewritten with `{slot}` placeholders
- no procedure version until linked

Linking happens when:

- Teacher admits a procedure whose name/lemma is a VerbNet class member, or
- a front-analysis part executes through an already-bound catalog key and also matches a VerbNet member lemma

Unbound rows never execute. They are retrieval and key vocabulary for the interpreter.

### WordNet synonyms

Do not import synsets as concepts.

For each Active or Validated concept whose name matches a WordNet 3.1 lemma, admit `alias-of` edges for the other lemmas in that synset. Provisional, `source=import`, pinned WordNet version. Skip synsets larger than 12 lemmas. Skip aliases that collide with a different existing concept name.

### What is not v1

- Wikidata dumps
- ConceptNet assertion dumps
- MASSIVE / SLURP / ATIS as catalog keys
- Creating a concept per WordNet synset
- Auto-authoring procedures from VerbNet semantics

### Import mechanics

Checked-in seed artifacts (JSON) generated by a repo-owned importer from pinned VerbNet and WordNet snapshots. Engine load is idempotent on `(source, source_key)`. Re-import does not clone edges. Imported rows carry provenance `{ source: "import", resource: "verbnet-3.4" | "wordnet-3.1", version: <pin> }`.

The importer is a real program in-tree. It reads the pinned snapshots, emits the seed JSON, and the Engine applies that JSON through the same admission path as live language writes.

## Components

- **Language Interpreter** (`@spoon/intent`): small-model structured output. New schema is `UtteranceAnalysisProposal` (token indexes, no byte offsets, no durable IDs).
- **Grounding** (`spoon-core` language): proposal -> `UtteranceAnalysis` with byte spans, alignment checks, mention resolution against the current stream and packet.
- **Dispatch** (`spoon-engine` cycle): interpreter first; per-part catalog/Teacher; derived dependency graph; ResponsePlan concat.
- **Catalog** (Engine data, persisted): semantic keys, slot schemas, surface patterns, bound procedure versions.
- **Seed importer**: VerbNet/WordNet -> seed JSON -> Engine admission.
- **Teacher**: unchanged lesson kinds, invoked per unknown executable part rather than for the whole utterance.

## Error handling

- Malformed or ungroundable proposal: reject, record diagnostic on the episode, fall through to local match then Teacher for the whole utterance.
- `depends_on` cycle: reject the analysis the same way.
- Model-emitted durable IDs: reject.
- Span that is not a complete token range in the original stream: reject that mention; if it was required for Execute, that part becomes Abstain.
- Unresolved mention on an Execute part: coerce that part to Clarify.
- Teacher failure on one part: that part Abstains; siblings still complete.
- Seed import conflict (alias collides with another concept): skip that alias, keep a receipt, do not fail Engine open.

## Testing

- Fixture `"hey whats 2+2 and then double that"` produces three parts, `p2` `part_ref`s `p1.result`, greeting renders first, both numeric claims render, cleaned still contains `{p1.result}` after dispatch.
- Alignment: every cleaned token maps onto original UTF-8 boundaries.
- Catalog: successful multiply part writes one deduped surface pattern.
- WordNet seed: concept `double` gains `twice` as `alias-of` without creating a new concept.
- VerbNet seed: unbound catalog row exists; it does not execute until linked.
- Weaning: after seed plus a successful episode, a later teacher-off / interpreter-off cycle still hits the catalog for the same skeleton.
- Independent sibling still runs when a dependent part Clarifies.

## Weaning

Same metric family as Teacher-off. Front-model calls should decline per domain as alias, pattern, and catalog bindings accumulate. Competence must survive interpreter-off for seeded and previously successful skeletons.
