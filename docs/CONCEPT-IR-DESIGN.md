# Concept & IR Design

## Status

Design discussion (2026-09-03) about the core representation for Spoon v2.
Covers Concepts, evaluation, how concepts infer relationships, how the ears
work, and how the concept vocabulary self-ranks.

This is the reasoning behind the architectural direction in PIVOT_PLAN.md.

---

## The Core Realization: There Is No Separate IR

The Spoon whitepaper talks about concepts and then separately about an IR that
"lowers" concepts to execution. But look at what "lowering" actually does:

```
Tallest<FriendGroup>
  -> MaxBy<FriendGroup, Height>
  -> First<Sort<FriendGroup, Height, Descending>>
  -> [native sort] -> [native first] -> Greg
```

Each step replaces one concept application with another. The "IR" is just
concept applications whose heads happen to have executable realizations.
There's no compilation boundary. It's **term rewriting all the way down**
until you hit ground operations.

---

## The Concept

Everything in Spoon is a Concept. Concepts come in three shapes.

```typescript
type Concept =
  | { kind: "atomic";   id: ConceptId }                    // Greg, Add, 42
  | { kind: "compound"; head: Concept; args: Concept[] }   // FriendWith<Greg, Keal>
  | { kind: "hole";     id: number }                       // placeholder in patterns

type ConceptId =
  | { kind: "named";  name: string }    // greg, add, sort, friend-with
  | { kind: "ground"; value: Ground }   // 42, "hello", true, {json}

type Ground =
  | { type: "bool";     value: boolean }
  | { type: "int";      value: number }
  | { type: "float";    value: number }
  | { type: "text";     value: string }
  | { type: "json";     value: unknown }
  | { type: "datetime"; value: Date }
  | { type: "bytes";    value: Uint8Array }
```

Rust equivalent:

```rust
enum Concept {
    Atomic(ConceptId),
    Compound { head: Box<Concept>, args: Vec<Concept> },
    Hole(HoleId),
}

enum ConceptId {
    Named(SymbolId),
    Ground(Ground),
}
```

### Atomic Concepts

Indivisible. Has identity, nothing to decompose.

```
Greg   -> Atomic(Named("greg"))
Add    -> Atomic(Named("add"))
Sort   -> Atomic(Named("sort"))
42     -> Atomic(Ground(Int(42)))
"abc"  -> Atomic(Ground(Text("abc")))
true   -> Atomic(Ground(Bool(true)))
```

**Named vs ground identity.** A named concept's identity is an interned symbol;
its meaning lives in the store (surface forms, realizations, relationships).
A ground concept's identity *is* its value, so it is self-describing: look at it
and you know everything.

This is why ground concepts do not need a database row to exist. `Add<42, 1>`
touches the store zero times. A row appears only if something gets asserted
*about* that value:

```
Synonym<"forty-two", 42>
PullRequestNumber<160>
HttpStatusMeaning<404, NotFound>
```

Named concepts always need a row, because a name alone tells you nothing.

### Compound Concepts

One concept applied to others. This is where structure happens.

```
FriendWith<Greg, Keal>
= Compound {
    head: Atomic(Named("friend-with")),
    args: [Atomic(Named("greg")), Atomic(Named("keal"))]
  }

Add<42, 1>
= Compound {
    head: Atomic(Named("add")),
    args: [Atomic(Ground(Int(42))), Atomic(Ground(Int(1)))]
  }

Height<Greg>
= Compound {
    head: Atomic(Named("height")),
    args: [Atomic(Named("greg"))]
  }
```

They nest arbitrarily:

```
Sum<Map<Friends, Height>>
= Compound {
    head: Atomic(Named("sum")),
    args: [Compound {
      head: Atomic(Named("map")),
      args: [Atomic(Named("friends")), Atomic(Named("height"))]
    }]
  }
```

A compound is a full citizen, exactly like an atomic concept. It can be stored,
carry realizations, participate in relationships, and accumulate activation
stats. `Employment<Greg, Workiva>` is a compound concept with its own attached
knowledge:

```
Employment<Greg, Workiva>
  StartedOn<..., 2019-03-01>
  Role<..., SoftwareEngineer>
  Status<..., Active>
```

A compound with zero args is a bare mention or assertion of its head in context.
A compound whose head has a realization is executable. A compound whose head has
no realization is purely semantic: still meaningful, just not computable yet.

### Holes

The only shape that is not itself a concept. A gap in a pattern waiting to be
filled. Holes exist inside rewrite rules and inside partially resolved
expressions.

```
Symmetric<R> means: R<X, Y> -> R<Y, X>
  where R, X, Y are Hole(0), Hole(1), Hole(2)
```

During matching, holes bind to concrete concepts. During evaluation, a bound
hole resolves to whatever filled it.

### Summary Table

| Written | Shape | Representation |
|---|---|---|
| `Greg` | atomic, named | `Atomic(Named("greg"))` |
| `42` | atomic, ground | `Atomic(Ground(Int(42)))` |
| `FriendWith<Greg, Keal>` | compound | head + 2 args |
| `Sum<Map<Friends, Height>>` | compound, nested | head + 1 compound arg |
| `R` in a rule | hole | `Hole(0)` |

---

## Where Relationships Live

If relationships are concepts, and concept-to-concept links are compounds,
where do they actually live?

**The store is a flat bag of concepts, indexed many ways.**

When you assert `FriendWith<Greg, Keal>`, the store persists that compound and
indexes it by:

- **Head**: `FriendWith` (so "find all friendships" works)
- **Participants**: `Greg`, `Keal` (so "find everything about Greg" works)
- **Structure**: full structural hash (so exact-match lookup works)
- **Embedding**: vector, for fuzzy semantic retrieval
- **Time**: asserted-at, validity window

The compound does not "belong to" Greg or FriendWith. It exists independently.
Both can be found through it.

---

## How Concepts Inherently Infer Other Relationships

Consider these stored concepts:

```
Symmetric<FriendWith>
FriendWith<Greg, Keal>
```

Someone asks "Is Keal friends with Greg?", which is a query for
`FriendWith<Keal, Greg>`.

Direct lookup finds nothing. Inference kicks in:

1. `Symmetric` has a **Rule** realization:

```
Rule {
  pattern:   Hole(R)<Hole(X), Hole(Y)>
  condition: Symmetric<Hole(R)>
  produce:   Hole(R)<Hole(Y), Hole(X)>
}
```

2. The query matches the pattern with `R=FriendWith, X=Keal, Y=Greg`
3. Condition check: does `Symmetric<FriendWith>` exist? Yes.
4. Produce the flipped form and look it up: `FriendWith<Greg, Keal>` exists.
5. Inferred: `FriendWith<Keal, Greg>` holds.

**The inference IS evaluation.** `Symmetric` is a concept whose realization
happens to be a logical rule rather than executable code. Same mechanism,
different realization kind.

More meta-concepts that infer:

```
InverseOf<ParentOf, ChildOf>
  + ParentOf<Alice, Bob>
  -> infers ChildOf<Bob, Alice>

TransitiveClosure<AncestorOf>
  + AncestorOf<A, B> + AncestorOf<B, C>
  -> infers AncestorOf<A, C>

DefaultExpectation<Person, HasArms, 2>
  + Person<Greg>
  -> infers HasArms<Greg, 2>   (defeasible: direct evidence overrides)
```

None of these are magic. They are concepts with rule realizations, same as
`Sort` or `Fetch` are concepts with native realizations.

---

## Realizations

A realization attaches to a concept and says "here is how to operationalize
this".

```rust
enum Realization {
    // Rust function
    Native(fn(&[Concept], &Store) -> Result<Concept>),

    // Composed from other concepts (learned programs)
    // "double" realized as Add<Hole(0), Hole(0)>
    Composed(Concept),

    // Pattern rewrite (inference, simplification)
    Rule { pattern: Concept, condition: Concept, produce: Concept },

    // LLM call (fuzzy reasoning, language tasks)
    Neural { prompt: Concept, parse: Concept },

    // External (HTTP, shell, process)
    External { effect: Effect, spec: Concept },
}
```

### Multiple Realizations Compete

A concept can have many realizations at once. They are stored as ordinary
concepts:

```
RealizedBy<Sort, NativeQuickSort>
RealizedBy<Sort, NativeMergeSort>
RealizedBy<Sort, LearnedPreSortedCheck>
```

Each realization concept carries its own stats and contextual fit:

```
NativeQuickSort
  stats: { uses: 847, successes: 840, failures: 7 }
  WorksWellWith<NativeQuickSort, LargeUnsorted>    conf 0.9
  WorksPoorlyWith<NativeQuickSort, NearlySorted>   conf 0.6

LearnedPreSortedCheck
  stats: { uses: 23, successes: 23, failures: 0 }
  WorksWellWith<LearnedPreSortedCheck, NearlySorted>  conf 0.85
```

Selection scores candidates by `activation * success_rate * context_match`,
picks a winner, executes, records the outcome, updates stats. The context-fit
concepts are themselves stored concepts: learnable and revisable.

Selection itself can eventually be a concept with a realization, so Spoon can
improve how it chooses.

---

## The Evaluation Model

```
evaluate(concept, store, context) -> concept
  match concept:
    Atomic(_) -> return concept              // self-evaluating
    Hole(id)  -> resolve from context bindings
    Compound { head, args } ->
      1. evaluate head
      2. evaluate args (lazy or eager: design choice)
      3. if head resolves to an atomic concept, look up its realizations
      4. if a realization exists:
           select best one (evidence-weighted, contextual)
           apply it:
             Native:   call Rust fn with evaluated args
             Composed: substitute args into template, evaluate result
             Rule:     check condition, produce replacement, evaluate
             Neural:   build prompt from args, call LLM, parse to concepts
             External: check effect permissions, execute, wrap result
           evaluate the result recursively
         if no realization:
           return the unevaluated compound (it is still meaningful)
           optionally flag a capability gap
```

That is the entire execution model.

- "Lowering" is repeated evaluation where high-level concepts rewrite into
  lower-level ones until natives are reached.
- "Inference" is evaluation where the realizations are logical rules.
- "Planning" is evaluation where the realizations are search procedures.

Same loop.

---

## The Intermingling Is Correct

Consider `Height<Greg>`:

- **As a semantic concept**: "the concept of Greg's height". No computation.
- **As a query**: "what is Greg's height?" Needs store lookup.
- **Inside a computation**: `MaxBy<Friends, Height>`. Needs evaluation.
- **As an unfilled slot**: meaningful even when Spoon does not know the answer.

The same compound plays all four roles depending on context. That is not a
design flaw. It is the core architectural bet. The concept is the universal
joint between meaning and computation.

---

## Open Design Questions

1. **Concept identity for compounds.** Is `FriendWith<Greg, Keal>` the same
   concept every time it appears, or a fresh one per assertion? Probably:
   structural equality for lookup, separate provenance/timestamp per assertion.

2. **Evaluation strategy.** Eager or lazy? For `MaxBy<FriendGroup, Height>`
   over 1000 members, do all heights evaluate upfront? Probably lazy with
   caching.

3. **Rule application order.** When several rules could fire, which goes first?
   Probably activation-weighted, with depth limits against infinite chains.

4. **Named vs positional args.** `Employment<Greg, Workiva>` or
   `Employment<employee=Greg, employer=Workiva>`? Named roles are more semantic
   but heavier. Probably both: positional for simple binary relations, named
   for complex n-ary ones.

5. **Ground type set.** How small can `Ground` stay? Bool, int, float, text,
   bytes, datetime, json is the current bet. Everything else composes.

---

## Ears: Messy Language In

### The LLM Produces Concepts, Not Text

The LLM decomposes messy English into a flat sequence of concept operations.
Given a base vocabulary of roughly 200 to 400 concepts, it emits structure:

```
Utterance<
  // "grab" -> establish the task
  Task<Retrieve>,

  // "the weights" -> bind target field
  Bind<X, Field<Weight>>,

  // "no wait the scores" -> correction retracts and rebinds
  Correction<Bind<X, Field<Score>>>,

  // "from those probe things" -> source is probe-shaped but fuzzy
  Source<Fuzzy<Probe>>,

  // "I sent you" -> constrain to prior user-provided data
  Qualify<Source, SentBy<User>>,

  // "not the first batch" -> negate first
  Qualify<Source, Negation<Ordinal<1>>>,

  // "the second one" -> positively select second
  Qualify<Source, Ordinal<2>>,

  // resolve the narrowed source into concrete data
  Let<Y, Resolve<Source>>,

  // "and like add em up or whatever"
  Let<Z, Map<Y, X>>,
  Let<B, Sum<Z>>,

  // "and tell me"
  Task<Present>,

  // "if thats more than last time"
  Let<A, PriorResult<Sum<Map<Probe, Score>>>>,
  Let<Answer, GreaterThan<B, A>>,

  Present<B, Answer>
>
```

Why this beats a parse tree:

1. **Sequential with corrections** mirrors how humans actually talk: bind,
   rebind, qualify, qualify.
2. **Variables are explicit.** `Let<Y, Resolve<Source>>` makes data flow visible.
3. **Qualifiers stack.** The source narrows progressively; nothing has to
   resolve before context is complete.
4. **Fuzzy references stay fuzzy.** `Fuzzy<Probe>` defers resolution.

The LLM emits flat sequences. Spoon reshapes them into trees during resolution.

### Unknown Vocabulary

```
User: "yeet that config into staging"

LLM emits:
  Task<Unknown<"yeet">>,
  Target<Config>,
  Destination<Staging>
```

`yeet` is flagged unknown but structurally placed: a task-shaped verb acting on
a target toward a destination. Then:

1. Store checks synonyms. Is `yeet` mapped? No.
2. Teacher gets context: "User said 'yeet' in a Task position, targeting Config
   toward Staging. Probable meaning?"
3. Teacher answers: `Synonym<"yeet", Deploy>` at confidence 0.8.
4. Store it. Next time, `yeet` resolves without the Teacher.

The constraint ladder:

```
Level 1: LLM maps to known concepts    -> immediate execution
Level 2: LLM emits Unknown<"word">      -> structural hole, Teacher fills it
Level 3: Teacher maps to a synonym      -> stored, done
Level 4: Teacher mints a new concept    -> new concept with relationships
Level 5: New concept needs realization  -> synthesis or Teacher-provided
```

Most utterances stay at Level 1. Levels 2 and 3 fire occasionally. Levels 4 and
5 are rare. Every Level 2+ resolution becomes a Level 1 lookup next time.

---

## Self-Ranking Vocabulary

The ears prompt is not a static list. It is assembled per turn from activation
stats.

```
Tier 1 (always in prompt), by frequency x recency x success_rate:
  Retrieve  [847 uses, 99% success]
  Bind      [1203 uses, 97% success]
  Filter    [412 uses, 95% success]
  Sum       [289 uses, 98% success]

Tier 2 (included when context suggests):
  Deploy    [34 uses, 91% success]
  Rollback  [12 uses, 88% success]

Tier 3 (not in prompt, retrieved on demand):
  Synonym<"yeet", Deploy>  [3 uses]

Never prompted (deprecated):
  OldBrokenConcept  [8 uses, 12% success, superseded]
```

- Constantly used concepts stay visible, so recognition is fastest.
- Domain concepts surface when domain context is active.
- Rare concepts do not waste prompt space but remain retrievable when
  `Unknown` triggers a lookup.
- Failed concepts sink and eventually drop off.

The ranking is ACT-R base-level activation, the same mechanism that selects
between competing realizations. One curation system, two uses.

The prompt window becomes a **working memory** of Spoon's most useful current
language. A DevOps Spoon has Deploy/Rollback/Scale in Tier 1. A data-science
Spoon has DataFrame/Correlate/Plot in Tier 1. Same engine, different
top-of-mind vocabulary.

The ears get better at understanding what this user actually says, not language
in general. "yeet" starts Unknown, becomes a synonym, reaches Tier 2 with
repeated use. Spoon develops an idiolect that mirrors its user.
