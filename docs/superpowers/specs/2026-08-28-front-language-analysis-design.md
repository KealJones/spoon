# Front Language Analysis

Date: 2026-08-28

Status: Draft for review

## Goal

Segment every utterance into parts, dispatch each part independently, and concatenate the replies so no half of an utterance is dropped. Admit the language structure that made a part work, so the front model can be weaned.

The model is a language teacher. It proposes; the Engine admits. Model JSON is never knowledge by itself, never authority, and never execution.

The same small model also acts as a **Language Realizer** on the way out. The realizer selects an Engine-owned sentence template and a claim order. It emits no user-visible characters of its own.

## Non-goals

- Replacing the original utterance with cleaned text
- Mutating cleaned text after a part executes
- Letting the front model author executable procedure bodies
- Letting the realizer author, drop, or reword claims
- Importing external lexical resources (VerbNet, WordNet, Wikidata, ConceptNet, MASSIVE). Catalog vocabulary comes from what this instance actually learned.
- Compact reply folding that omits user-facing claims

## Prerequisites specified here

None of the following exist in code today. This document specifies each one completely; none is deferred.

| Thing | Current state |
|---|---|
| `LanguageContextPacket` | Named in `LANGUAGE-IMPLEMENTATION.md`, absent from the tree |
| Persisted Intent Catalog | Absent. `cycle.rs` `procedure_catalog` is a per-request candidate list, not storage |
| `UtteranceAnalysis` IR and grounding | Absent. `InterpretationProposal` covers a single whole-utterance frame set |
| Per-part cycle suspend/resume | Absent. `CycleProgress` has no partial-completion state |
| Per-claim dialogue act | Absent. `ResponsePlan` carries one `dialogue_move` |
| `RenderVariant::Joined` | Absent. `Plain` joins claims with `\n` |
| Template-selector realizer | Absent |

## Cycle

```text
utterance
  -> tokenize original (ground truth)
  -> local catalog match on the whole utterance
       unique resolution -> execute, skip the interpreter
  -> Engine LanguageContextPacket                 (on miss or ambiguity)
  -> Front Language Interpreter
       UtteranceAnalysisProposal
       optionally one supplemental-context round
  -> ground + validate
  -> per-part dispatch (toposort over derived depends_on)
       known intent key   -> catalog -> execute
       dialogue act       -> utterance-grounded claim
       unknown executable -> Teacher for that part only (suspend)
  -> Engine ResponsePlan from part outcomes
  -> Language Realizer (template id + claim order, optional)
  -> validate realization; on any failure use ResponseRenderer
  -> admit language writes and residual facts (Provisional)
```

### Why local match runs first

The interpreter is a model call. Running it unconditionally would guarantee one model call per turn forever, which is the opposite of the weaning goal and makes the weaning metric meaningless.

A whole-utterance local hit is safe because catalog patterns are only ever admitted from a single part (see [Intent Catalog](#intent-catalog)). No pattern skeleton spans two speech acts. So a whole-utterance match implies the utterance is single-part.

Order is therefore: local match, then interpreter on miss or ambiguity, then Teacher per part. This preserves the existing escalation shape in `cycle.rs`.

## LanguageContextPacket

A read-only, bounded, redacted projection the Engine builds before inference. The model addresses everything in it by request-local alias. Durable IDs never enter the packet and are rejected if they come back out.

```text
LanguageContextPacket
  utterance:    TokenStream                  // exact stream; token indexes are the addressing scheme
  turns:        [PacketTurn]                 // same-session, newest first
  catalog:      [PacketCatalogEntry]
  terminology:  [PacketAlias]
  environment:  [PacketEnvFact]
  budgets:      PacketBudgets
  truncation:   [TruncationFlag]

PacketTurn        { alias: t0.., role: user | spoon, summary: string, facts: [PacketFactRef] }
PacketCatalogEntry{ alias: c0.., key: string, slots: [SlotSchema], patterns: [string] }
PacketAlias       { alias: a0.., surface: string, refers_to: c<n> }
PacketEnvFact     { alias: e0.., predicate: string, value: Value }
PacketFactRef     { alias: f0.., predicate: string, value: Value }
TruncationFlag    { group: string, dropped: u32 }
```

Bounds, all enforced before serialization:

| Group | Bound |
|---|---|
| turns | `budget.max_context_items`, max 8 |
| catalog entries | `budget.max_context_items`, max 32 |
| patterns per entry | 4 (highest support first) |
| terminology | 64 |
| environment facts | 32 |
| summary length | 512 bytes |
| whole packet | 16 KiB serialized |

Anything dropped by a bound emits a `TruncationFlag`. Silent truncation is a defect.

Excluded unconditionally: secrets, capability grants, permission state, machine paths, durable IDs, raw episode traces, Teacher prose, other sessions.

### Supplemental-context round

The first interpreter result is either a final `UtteranceAnalysisProposal` or exactly one typed request:

```text
SupplementalRequest
  catalog_detail { alias: c<n> }            // full slot schema and all patterns for one entry
  turn_window    { count: 1..4 }            // that many older same-session turn summaries
  terminology    { source_tokens: TokenRange }  // aliases for one grounded surface phrase
```

The Engine answers once inside the same bounds, then requires a final proposal. A second request, an unrecognized variant, a request naming an alias not already in the packet, or any free-form string field is a rejected analysis. There is no SQL, no path, no graph query, no capability request, and no durable ID at this boundary.

## Trusted IR

The original `TokenStream` is the only source of provenance. Cleaned text is a derived aligned document. Every cleaned token maps back to original byte spans. Downstream never treats the rewrite as source.

```text
UtteranceAnalysis
  original: TokenStream
  cleaned:  AlignedDocument
  parts:    [Part]
  language_writes: [LanguageWrite]

Part
  id: p0
  spans: [TokenRange]              // in the original stream
  template: "what is {v0} times {v1}"
  mentions: [Mention]
  context_bindings: [Mention]      // resolved from the packet, inferred = true
  intent: IntentFrameSet           // Execute | Clarify | Abstain, for THIS part
  act: DialogueAct                 // Inform | Ask | Clarify | Confirm | Correct | Acknowledge | Refuse | Abstain
  residual: [ResidualClaim]

Mention
  key: e0 | v0 | x0
  kind: entity | value | expression | result
  surface: [TokenRange]            // empty only when inferred = true
  inferred: bool
  resolved:
    literal     { value }
    part_ref    { part: p<n>, role: mention | result }
    context_ref { alias }          // an alias already present in the packet
    unresolved  { ambiguity }
```

Competing hypotheses live inside a part, in its `IntentFrameSet`. Parts are sequential speech acts, not alternate meanings of the whole utterance.

`IntentFrame`, `IntentFrameSet`, `TokenRange`, and the existing `ground_for` span grounding are unchanged. The new grain is `UtteranceAnalysis`, which wraps per-part frame sets.

There is no `Greet` dialogue act. Greetings map to `Acknowledge`.

### Structural validation

An analysis is rejected whole if any of these fail. Rejection falls through to local match and then whole-utterance Teacher.

- 1 to 8 parts.
- Part spans do not overlap.
- The union of part spans covers every non-whitespace token in the original stream. Dropping half an utterance is the exact bug this design exists to fix, so partial coverage is never accepted silently.
- Every `part_ref` names an existing part and is not self-referential.
- The `depends_on` graph derived from `part_ref` mentions is acyclic.
- Every non-inferred mention has at least one `TokenRange` that covers complete tokens.
- No durable ID appears anywhere in the proposal.
- Every `context_ref` alias was already in the packet.

Per-mention failures are narrower: a span that is not a complete token range rejects that mention. If the mention was required for `Execute`, the part becomes `Abstain`. An `unresolved` mention on an `Execute` part coerces that part to `Clarify`.

### Placeholders in cleaned text

Two mention classes behave differently, and the distinction is normative:

- `literal` and `context_ref` mentions are **materialized** into cleaned text at analysis time. `"it"` becomes `"the file from p0"` or `"Pierre (from last turn)"`. No graph IDs.
- `part_ref` mentions **stay as placeholders forever**. `"what's 2+2, now double that"` never rewrites cleaned to contain `4`. It binds `part_ref { part: p1, role: result }` and cleaned keeps `{p1.result}`.

Cleaned text is written once, at analysis time, and is immutable after dispatch. Bound results live on part outcomes and `ObservedFact`s.

## Part dispatch

The Engine toposorts parts by derived `depends_on`, breaking ties by source order. Independent parts may run in any order; the reply always concatenates in source order.

A part that Clarifies or Abstains does not run. Its dependents do not run and are marked `Blocked`. Independent siblings still run.

```text
PartOutcome
  part: p<n>
  state: Executed | Clarified | Abstained | Blocked
  value: Option<Value>
  evidence: [EvidenceReference]
  provenance: [string]            // "procedure:<id>@<version>", receipt ids
  rendered_in_turn: Option<TurnId>
```

## Cycle suspend and resume

Per-part Teacher requires partial-completion state, which `CycleProgress` does not have today.

```text
PendingPartsCycle
  input:      CycleInput
  analysis:   UtteranceAnalysis          // frozen
  order:      [part_id]                  // toposort result, frozen
  outcomes:   { part_id -> PartOutcome }  // completed parts, accumulating
  blocked_on: part_id
  reason:     Teacher | Clarification
  request:    TeacherRequestWire | ClarificationRequest
```

Added as a third `PersistedPendingCycle` variant alongside `Teacher` and `Intent`. `CycleProgress` gains no new variant; a parts cycle suspends as `NeedTeacher` and resumes through the existing `cycle.resumeTeacher`.

**The analysis is frozen.** Resume never re-runs the interpreter over the original utterance. If it did, the model could re-segment differently and orphan the outcomes already collected. Resume binds the lesson to `blocked_on`, records that `PartOutcome`, and continues the frozen `order` from that position.

Unknown parts are taught serially, one Teacher request per resume, in toposort order, so a producer is always taught before its consumer.

**Budget.** Each part-level Teacher request decrements the existing `max_teacher_turns`. When the budget reaches zero mid-utterance, every remaining untaught part becomes `Abstained` and the already-completed parts still render. A budget exhaustion never discards work already done.

**Cycle disposition.** `Verified` when every part executed. `Provisional` when at least one part executed and at least one did not. `Abstained` when no part executed.

## Clarification across parts

A clarification is a suspend, not a restart.

The first turn renders every part that completed, plus the question derived from the blocked part's ambiguity. The user's reply resumes the same `PendingPartsCycle`. Analysis runs on **the reply alone**, with the blocked part's ambiguity supplied in the packet, and its result binds the blocked part's unresolved mention. The original utterance is not re-analyzed.

**A part outcome renders exactly once**, in the turn where it completed, recorded in `rendered_in_turn`. So:

```text
user:  delete that file and tell me what 2+2 is
spoon: 2 + 2 is 4. Which file do you mean?
user:  notes.txt
spoon: Deleted notes.txt.
```

There is no double answer, because `p1` already recorded `rendered_in_turn`.

## Response composition

The Engine authors the `ResponsePlan`. Claims come only from execution, observations, and utterance grounding.

### Per-claim dialogue act

`GroundedClaim` gains one field:

```text
GroundedClaim
  id, text, evidence, provenance
  act: Option<DialogueAct>       // serde default None; None inherits plan.dialogue_move.act
```

The field defaults, so existing `language.render` payloads deserialize unchanged. This is the minimum change that lets one plan carry a greeting and two answers.

The plan-level `dialogue_move.act` is derived, by precedence, from what the turn demands of the user:

1. any part `Clarify` -> `Clarify`
2. else any part `Ask` -> `Ask`
3. else any part `Refuse` -> `Refuse`
4. else at least one grounded claim, and not all of them `Acknowledge` -> `Inform`
5. else at least one grounded claim -> `Acknowledge`
6. else -> `Abstain`

`uncertainty.level` is the maximum over per-part levels under the order `Certain < Qualified < Unknown`. `uncertainty.disclosure` is the per-part disclosures joined in source order with a single space, or `None` if there are none.

Disclosure stays **out of band** in both the deterministic and template paths, exactly as `ResponseRenderer` returns it today. Neither path splices it into `text`. Clients render it.

`tone` is one value for the plan, from session config. It is not per part.

### Evidence minting

`GroundedClaim` validation rejects empty evidence, so every claim needs a real reference. Sources map as follows, using only existing `SourceKind` values:

| Claim source | `source_kind` | `id` | `provenance` |
|---|---|---|---|
| Executed part, deterministic local procedure | `SelfVerified` | `<episode-id>:part:<part_id>` | `procedure:<id>@<version>` |
| Executed part backed by an observation or capability receipt | `Observed` | the `ObservedFact` id, `<episode-id>:<ordinal>` | receipt id |
| Dialogue claim grounded in the utterance (greeting, acknowledgement) | `Observed` | `<episode-id>:utterance:<startByte>-<endByte>` | none |
| Fact admitted from a residual claim | `Taught` | the new `ObservedFact` id | the grounding token span or packet alias |

The greeting is grounded in the observable fact that the user greeted. That is honest provenance, and it makes `"Hey."` a valid claim rather than a claim the renderer would silently omit.

Every `EvidenceReference` carries `linked_episode`.

### Deterministic renderer

`RenderVariant` gains `Joined`, which joins claim texts with a single space. `Plain` keeps its `\n` join and its current behavior for existing callers.

The weaned and fallback path for `"hey whats 2+2 and then double that"` is `Joined`:

`Hey. 2 + 2 is 4. Double that is 8.`

## Language Realizer

The realizer makes that read conversationally without being able to fabricate anything, because it emits no user-visible characters.

```text
RealizationProposal
  template_id: string          // from the Engine-owned template set
  slot_order:  [claim_id]      // a permutation of the plan's grounded claim ids
  tone:        Neutral | Direct | Warm | Formal
```

The Engine substitutes verbatim claim text into the selected template. The model chooses a shape and an order; it writes nothing.

### Template set

Versioned Engine data in-tree, not model output. Each template declares an arity, optional per-slot act constraints, and one connective string per tone.

| id | arity | shape (Neutral) | constraint |
|---|---|---|---|
| `join.sentences` | variadic | `{0} {1} ... {n}` | none |
| `join.and` | 2 | `{0}, and {1}` | none |
| `join.and.list` | 3 | `{0}, {1}, and {2}` | none |
| `join.then` | 2 | `{0} Then {1}` | slot 0 must precede slot 1 in `depends_on` |
| `join.lead.ack` | 2 | `{0} {1}` | slot 0 act must be `Acknowledge` |
| `join.ack.and` | 3 | `{0} {1}, and {2}` | slot 0 act must be `Acknowledge` |

So the worked example realizes as:

`Hey. 2 + 2 is 4, and double that is 8.`

via `join.ack.and` with `slot_order = [c0, c1, c2]`.

### Sentence mechanics

Claim text is verbatim in content, but a claim written to stand alone does not compose. `"2 + 2 is 4."` inside `{0}, and {1}` would give `"2 + 2 is 4., and Double that is 8."`

Each template therefore declares two mechanical rules per slot, both Engine-owned and applied to Engine-authored text:

- `strip_terminator`: drop exactly one trailing `.`, `!`, or `?` when the template continues the sentence. An ellipsis is left alone, because collapsing `...` to `..` would change what the claim said.
- `lowercase_initial`: lowercase the first character of a claim landing mid-sentence, **only when the original utterance itself used that word with a lowercase initial**.

That evidence condition is the whole point of the second rule. `"double"` appears lowercase in `"hey whats 2+2 and then double that"`, so lowercasing it is grounded in what the user typed. `"Pierre"` never appears lowercase, so it keeps its capital even in a slot the template would otherwise lowercase. Decapitalizing a name to make a sentence flow is a worse error than a capital letter mid-sentence.

Neither rule can add, remove, or reorder a word. The model does not select them; they are fixed per template.

### Validation

Checked before anything reaches the user. Any failure discards the proposal, records a diagnostic on the episode, and renders with `ResponseRenderer`. The stored plan is never modified.

- `template_id` exists in the pinned template set
- template arity matches the count of grounded claims in the plan
- `slot_order` is a permutation of the plan's grounded claim ids: no repeats, no omissions, no unknown ids
- no `Unsupported` claim id appears in `slot_order`
- every per-slot act constraint holds
- **no reorder crosses a `depends_on` edge**: a consumer part's claim never precedes its producer's claim

That last rule is why the realizer cannot produce `"Double that is 8, and 2+2 is 4."`

Fabrication, omission, negation injection, and hedging are all structurally impossible rather than checked for, because the model never supplies text. The tradeoff is real and accepted: replies are less varied than free generation would give. In exchange the output surface needs no allowlist of safe connective words, which was the whole attack surface of a generate-and-verify realizer.

The realizer sees the plan, part templates, and dialogue acts. It does not see Teacher drafts, ungrounded residuals, or graph write proposals.

Interpreter-off and realizer-off are independent flags. Competence must survive both off.

### Model call budget

`CycleBudget` gains `max_language_model_calls: u32`, default 3: interpreter, at most one supplemental round, realizer. Exceeding the budget or hitting a provider timeout takes the deterministic path rather than failing the cycle.

## Episode storage

The original situation is never replaced. Cleaned text is never mutated after dispatch.

Each episode stores:

- `situation` - the original utterance
- `analysis.cleaned` - the analysis-time rewrite, `part_ref` placeholders intact
- `analysis.parts[]` - spans, template, mentions, intent, act, residuals
- `outcomes[]` - per-part state, value, evidence, `rendered_in_turn`
- `response.plan` - the Engine `ResponsePlan`
- `response.realized` - the template id, slot order, and final text, or the deterministic output plus the rejection diagnostic
- `diagnostics[]` - every rejection, truncation, and fallback

## Facts from the interpreter

The interpreter may assert facts. It may not assert them without a source, and the only sources that exist in this design are the utterance itself and the packet. There is no retrieval here, so a fact with neither is model-weight recall, and a small model citing its own weights is a fabricated citation.

```text
ResidualClaim
  id: r0
  predicate: string                    // validated name, <= 256 bytes
  value: Value
  scope: { key -> Value }              // <= 8 entries
  polarity: Assert | Deny
  provenance: TokenRange | ContextRef  // REQUIRED, exactly one
```

Admission:

- `TokenRange` provenance grounds to a byte span in the original stream. The user said it. Admit as `ObservedFact` with `source_episode` set, `verifier: None`, `VerifiabilityTier::Deferred`, `SourceKind::Taught`.
- `ContextRef` provenance must resolve to a packet alias that itself came from a prior `ObservedFact`. The new fact records that prior fact id in `scope`. Tier `Deferred`.
- No provenance, or provenance that fails to ground: the residual is dropped with a diagnostic. The part still executes. One bad residual never costs the user their answer.

A residual is never `Validated` on model assertion. Promotion runs through the existing evidence path, unchanged.

Bounds: 8 residuals per part, 32 per utterance.

Contradiction: a residual whose `(predicate, scope)` already has an `ObservedFact` with a different value does not overwrite it. It admits as a new fact and flags the pair for the existing reconciliation path.

## Graph contribution

No new tables for parts, cleaned text, or templates. Those are episode IR.

Existing types stay: Concept, Relationship, Procedure. `Relationship.kind` is a free `String` in core, so the closed set below is enforced at Engine admission, not by a new core enum.

| kind | meaning |
|---|---|
| `alias-of` | surface form for an existing term, including paraphrases |
| `termed` | phrase names a concept |
| `intent-of` | semantic key for a concept or procedure |

Admission rules:

- The model proposes names and request-local aliases. The Engine mints IDs.
- New surface form for a known concept: `alias-of` or `termed`, `Provisional`, this episode as evidence.
- New intent key that resolved to an existing catalog procedure: catalog observation plus `intent-of`, and only after that part actually executed.
- New entity with no matching concept: Particular concept, `Provisional`.
- Residual world facts: the `ObservedFact` path above. Never a fake constant procedure.
- New executable behavior: Teacher `reusable_lesson` for that part only. The front model does not author procedure bodies.
- A proposed kind outside the three above is rejected, with the analysis otherwise kept.

## Intent Catalog

Persisted Engine data in the existing SQLite store. Two new tables.

```sql
intent_catalog_entry
  key            TEXT PRIMARY KEY   -- "arithmetic.multiply"
  slots          TEXT NOT NULL      -- JSON [{name, required, value_kind}]
  concept_id     TEXT
  procedure_id   TEXT
  procedure_ver  INTEGER
  lifecycle      TEXT NOT NULL
  created_at     INTEGER NOT NULL

intent_catalog_pattern
  key            TEXT NOT NULL      -- references intent_catalog_entry(key)
  skeleton       TEXT NOT NULL      -- normalized dedup key
  pattern        TEXT NOT NULL      -- "what is {v0} times {v1}"
  support        INTEGER NOT NULL   -- distinct episodes that produced it
  contradictions INTEGER NOT NULL
  lifecycle      TEXT NOT NULL
  first_episode  TEXT NOT NULL
  last_episode   TEXT NOT NULL
  PRIMARY KEY (key, skeleton)
```

Keys are stable strings such as `arithmetic.multiply`. Never database UUIDs, so training and export artifacts stay portable.

### Skeleton normalization

Fully determined, no lexicon required:

1. NFKC normalize.
2. Unicode simple lowercase.
3. Replace each `{name}` with `{i}`, where `i` is that slot's index in the entry's declared slot list. Positional, so `{v0} times {v1}` gives `{0} times {1}` and the swapped phrasing gives `{1} times {0}`. Argument order is preserved as a real distinction.
4. Collapse whitespace runs to one space, then trim.
5. Strip leading and trailing punctuation from the whole string. Interior punctuation is kept.

No stopword removal and no lemmatization. Both would need a lexicon this design deliberately does not import.

### Pattern lifecycle

A pattern is admitted only after the part it came from executed successfully.

- Admitted `Provisional` with `support = 1`.
- A later successful dispatch from a **distinct episode** with the same skeleton increments `support`.
- `support >= 2` and `contradictions == 0` promotes to `Active`.
- **Only `Active` patterns drive interpreter-off local matching.** One lucky dispatch is not enough to become weaning fuel, because a mis-segmented part can still execute and return something plausible.
- A cycle that matched a pattern and then took negative user feedback or failed its contract check increments `contradictions`. At 1 the pattern drops to `UnderReview` and stops matching. At 2 it is `Retired`.

Cap 16 patterns per key. When full, evict the lowest-support `Provisional` pattern. Never evict an `Active` one. If every slot is `Active`, refuse the new pattern and record a receipt.

`alias-of` covers words. Catalog patterns cover slotty phrases. Together they are the weaning fuel for the local matcher.

## Error handling

| Condition | Behavior |
|---|---|
| Malformed or ungroundable proposal | Reject, diagnostic on the episode, fall through to local match then whole-utterance Teacher |
| Structural validation failure (overlap, coverage gap, cycle, bad `part_ref`) | Same as above |
| Model-emitted durable ID | Reject the analysis |
| Second supplemental request, or an unrecognized one | Reject the analysis |
| Span that is not a complete token range | Reject that mention; if required for `Execute`, that part `Abstain`s |
| Unresolved mention on an `Execute` part | Coerce that part to `Clarify` |
| Teacher failure on one part | That part `Abstain`s; siblings still complete |
| Teacher budget exhausted mid-utterance | Remaining untaught parts `Abstain`; completed parts still render |
| Residual claim with no groundable provenance | Drop the residual, diagnostic, part still executes |
| Realizer validation failure or backend miss | `ResponseRenderer` with `Joined`; stored plan unchanged |
| Model call budget exceeded | Deterministic path; cycle does not fail |
| Proposed relationship kind outside the closed set | Reject that write, keep the rest of the analysis |

## Testing

Behavioral:

- `"hey whats 2+2 and then double that"` yields three parts; `p2` `part_ref`s `p1.result`; the greeting renders first; both numeric claims render; cleaned still contains `{p1.result}` after dispatch.
- The greeting claim carries `Observed` utterance evidence and is not omitted by the renderer.
- Alignment: every cleaned token maps onto original UTF-8 boundaries.
- Local match hits first: a single-part utterance with an `Active` pattern completes with zero model calls.
- An independent sibling still runs when a dependent part Clarifies.
- Suspend and resume: a two-part utterance where `p1` is unknown suspends, resumes through `cycle.resumeTeacher`, and `p0`'s outcome survives the round trip unchanged.
- Resume does not re-analyze: the frozen `order` and `outcomes` are byte-identical before and after suspend.
- Teacher budget exhaustion leaves executed parts rendered and untaught parts `Abstained`.
- Clarification renders each outcome exactly once across both turns. No double answer.

Catalog:

- A successful multiply part writes one pattern, `Provisional`, `support = 1`.
- The same skeleton from a second distinct episode promotes it to `Active`.
- A `Provisional` pattern does not match with the interpreter off; the promoted one does.
- Negative feedback on a pattern-matched cycle drops it to `UnderReview` and it stops matching.
- Skeleton normalization keeps `{0} times {1}` and `{1} times {0}` distinct.
- Cap enforcement evicts the lowest-support `Provisional` and never an `Active`.

Facts:

- A residual with a token-span provenance admits an `ObservedFact` with `SourceKind::Taught`, tier `Deferred`, and never `Validated`.
- A residual with no provenance is dropped, the part still executes, and a diagnostic is recorded.
- A residual contradicting an existing fact admits alongside it and flags reconciliation.

Realizer:

- `join.ack.and` produces `Hey. 2 + 2 is 4, and double that is 8.` with claim text verbatim.
- A `slot_order` that omits a grounded claim is rejected; deterministic `Joined` output is used.
- A `slot_order` that places a consumer before its producer is rejected.
- A `template_id` outside the pinned set is rejected.
- An act constraint violation (non-`Acknowledge` claim in `join.lead.ack` slot 0) is rejected.
- Realizer-off produces the deterministic `Joined` concat.

Adversarial:

- Overlapping part spans reject the analysis.
- A coverage gap over non-whitespace tokens rejects the analysis.
- A `part_ref` to a nonexistent part rejects the analysis.
- A self-referential `part_ref` rejects the analysis.
- A `depends_on` cycle rejects the analysis.
- A durable UUID anywhere in the proposal rejects the analysis.
- A `context_ref` to an alias absent from the packet rejects the analysis.
- Nine parts reject the analysis.
- Two parts writing the same catalog skeleton in one cycle increment `support` by one, not two, because support counts distinct episodes.
- A packet at its size bound emits `TruncationFlag`s rather than silently dropping groups.

## Weaning

Same metric family as Teacher-off. Front-model calls per domain should decline as aliases, patterns, and catalog bindings accumulate, and the local-match-first ordering is what makes that decline observable rather than nominal.

Competence must survive interpreter-off for any skeleton with an `Active` pattern, and realizer-off through `ResponseRenderer`.
