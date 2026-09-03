# Evaluation

The specification for `spoon-eval` (PIVOT_PLAN.md Stage 2) and the parts of
Stage 3 inference that share its machinery.

Read `docs/CONCEPT-IR-DESIGN.md` first. This document assumes the concept model
and only specifies how evaluation runs over it.

---

## 1. The loop

```
evaluate(concept, ctx) -> Outcome
  Atomic(_)  -> the concept, unchanged
  Hole(id)   -> the binding for id, or the hole unchanged if unbound
  Compound { head, args } ->
    1. evaluate head
    2. resolve realizations for the evaluated head
    3. if none: return the compound unevaluated, record a capability gap
    4. select one (section 4)
    5. evaluate args per the realization's arg strategy (section 3)
    6. check effect authority (section 6)
    7. apply the realization (section 5)
    8. evaluate the result, unless the realization declares itself terminal
    9. record the outcome against the realization's activation
```

Steps 2 through 9 are the whole execution model. Lowering, inference, planning,
and arithmetic are the same loop with different realization kinds behind it.

### Outcome

Evaluation never returns "nothing". It returns one of:

```rust
enum Outcome {
    /// Reduced to a value. Nothing further to do.
    Value(Concept),
    /// Meaningful but not computable: no realization for some head. The
    /// concept is returned as far as it reduced, with the gaps named.
    Stuck { concept: Concept, gaps: Vec<Gap> },
    /// A budget ran out. Partial result plus which limit was hit.
    Exhausted { concept: Concept, limit: Limit },
    /// Execution was refused or failed. Carries the realization that failed so
    /// credit assignment has something to blame.
    Failed(EvalError),
}
```

`Stuck` is not an error. `Height<Greg>` with no way to resolve a height is a
meaningful concept that Spoon simply cannot reduce today. The gap it reports is
the signal that drives Teacher-assisted self-extension in Stage 6.

---

## 2. Budgets

Budgets are **nodes, time, and depth together**. Depth alone is not a budget:
a shallow rule that fans out can exhaust memory without ever getting deep.

```rust
struct Budget {
    max_nodes: u64,     // realization applications attempted
    max_millis: u64,    // wall clock
    max_depth: u32,     // rewrite nesting, guards non-terminating rules
    max_result_size: usize, // node count of any intermediate concept
}
```

Every counter decrements in one place, in the evaluator core, not in individual
realizations. A realization cannot opt out of the budget.

Exceeding any limit produces `Outcome::Exhausted` naming the limit. It is never
a panic and never a silent truncation.

---

## 3. Argument strategy

Most realizations want their arguments already reduced. Some must not have
them reduced at all: `If<cond, then, else>` that evaluates both branches is
wrong, and `Quote<x>` exists precisely to stop evaluation.

The realization declares which it wants:

```rust
enum ArgStrategy {
    /// Evaluate every argument before applying. The default.
    Eager,
    /// Receive arguments untouched, plus an evaluator handle. The realization
    /// decides what to reduce and when.
    Lazy,
    /// Evaluate only the arguments whose bit is set.
    Selective(u64),
}
```

`Selective` covers the common middle case: `If` evaluates argument 0 and leaves
1 and 2 alone.

An `Eager` realization that receives a `Stuck` argument does not run. The whole
compound returns `Stuck`, propagating the inner gap outward, because running a
native on a half-reduced argument produces garbage rather than a useful answer.

---

## 4. Selecting among competing realizations

A concept may have many realizations. Selection is contextual and
evidence-weighted, and it must leave room for new realizations to earn
evidence.

### Score

```
score = success_rate * context_fit * (1 + activation_bonus)
```

- `success_rate` comes from `Activation::success_rate()`, Laplace-smoothed so
  an unused realization sits at a neutral 0.5 and one early failure does not
  bury a good implementation.
- `context_fit` is in `[0, 1]`, defaulting to **0.5 when unknown**. It is
  derived from stored `WorksWellWith` / `WorksPoorlyWith` concepts matched
  against the active context. Absence of evidence is neutral, not negative.
- `activation_bonus` normalizes `Activation::base_level(now)` into `[0, 1]`
  and is **0 when there is no history at all**. A fresh realization is ranked
  below a proven one but stays reachable.

Deprecated-tier realizations are excluded outright. Kernel-tier realizations
are never evicted but get no scoring privilege: a learned realization that
performs better in context wins, which is the point of the whole design.

### Exploration

Pure exploitation means a realization that is never selected can never
accumulate the evidence that would justify selecting it, so the first
realization to arrive wins forever. The evaluator therefore explores:

With probability `epsilon` (default 0.05, zero when the caller requests
deterministic evaluation), select uniformly among candidates within a factor of
the top score rather than taking the maximum.

Exploration is disabled for `Effect::Write` and above. Trying an unproven
realization to learn from it is reasonable for a pure computation and
irresponsible for something that spends money or deletes data.

### Determinism

`Budget` carries a `deterministic: bool`. When set, epsilon is zero and ties
break by a stable key (realization name), so benchmarks and regression tests
reproduce exactly. Tests run deterministic; live conversation does not.

---

## 5. Applying a realization

| Kind | Application |
|---|---|
| `Native` | Look up the registry key, call the function with the evaluated arguments and a context handle. |
| `Composed` | `substitute_positional(body, args)`, then evaluate the result. `Hole(0)` is argument 0. |
| `Rule` | Match `pattern` against the concept. If it matches and `condition` is derivable under those bindings, produce `substitute(produce, bindings)`. |
| `Neural` | Render `prompt` to text, call the LLM seat, parse the response back into concepts via `parse`. Counts against the seat's call counter, never against `interior_llm_calls`. |
| `External` | Check effect authority, run the external operation, wrap the result as a concept. |

### The native registry

Function pointers cannot be persisted, so a stored `RealizationSpec::Native`
holds a `NativeId` registry key. At startup the evaluator binds each key to a
live function.

**A key with no registered function is a load-time error, not a silent
no-op.** A brain that references a native Spoon no longer ships is broken and
has to say so rather than quietly losing a capability.

### Recursion and self-reference

A `Composed` body may reference its own target. Recursion is bounded by
`max_depth` and `max_nodes` like everything else. There is no separate
recursion check and no special-casing of self-reference: a concept realized in
terms of itself is legitimate as long as it terminates within budget.

---

## 6. Effect authority

Capability and authority are separate concerns. Knowing how to do something
does not grant permission to do it.

Before applying any realization the evaluator compares its `Effect` against the
active `PermissionMode`:

```rust
enum PermissionMode {
    AlwaysAsk,   // confirm anything above Pure
    AskWrites,   // confirm Write and above. The default.
    Bypass,      // run anything
}
```

A realization that needs confirmation suspends evaluation and returns a
pending-permission state carrying enough context for the caller to ask the user
and resume. It does not block, and it does not proceed on the assumption that
permission would have been granted.

A `Composed` realization's effective effect is the **maximum** over its parts
(`Effect::join`). A composition is as dangerous as its most dangerous step, and
it must not be possible to launder a shell call through a chain of pure-looking
wrappers.

---

## 7. Inference is evaluation

`Symmetric` is an ordinary concept whose realization happens to be a `Rule`.
There is no separate inference engine, and rule application shares the
evaluator's budget.

When a query for a concept finds no direct assertion, the evaluator collects
rules whose `produce` could yield the queried shape and tries them backward.
Applicable rules are ordered by activation, and each firing decrements the same
node budget as any other realization application, so a runaway rule chain hits
`Exhausted` rather than hanging.

Cycles are cut by tracking the set of goals currently being derived. Re-entering
a goal already on the stack fails that branch instead of recursing forever.

Defaults are defeasible. `DefaultExpectation<Person, HasArms, 2>` fires only
when no direct evidence contradicts it, so direct evidence about Greg always
beats an inherited expectation about people.

---

## 8. Caching

Within a single turn, `Effect::Pure` evaluations are memoized by
`Concept::content_id()`. Purity is what makes this sound: the same input cannot
produce a different answer, so reuse is free.

Anything `Effect::Read` or above is never memoized. Store contents and the
outside world change underneath us, and a cached read is a stale read.

The cache is per-turn, not persistent. A result that is worth keeping across
turns is worth asserting into the store, where it gets provenance and a
validity window, rather than hiding in an invisible cache.

---

## 9. What the evaluator records

Every application appends to the turn's evaluation trace: the concept, the
realization chosen, the alternatives considered with their scores, the
arguments, the result, elapsed time, and budget consumed.

The trace is what makes credit assignment possible in Stage 5. Without it,
"learning from mistakes" is wishful thinking: you cannot tell whether a wrong
answer came from the wrong realization, a wrong argument, or a correct
execution over a wrong interpretation.

`interior_llm_calls` must stay 0 across evaluation. `Neural` realizations
increment their own seat counter. If evaluation ever needs the LLM to make a
choice, the design is broken.

---

## 10. Open questions for the implementer

These are genuinely undecided. Pick one, implement it fully, and record the
choice in STATUS.md rather than leaving a switch.

1. **Normalization order.** Innermost-first (reduce arguments fully, then the
   head) or outermost-first (rewrite the head, then descend)? Outermost is
   required for lazy conditionals to work at all. Innermost is more predictable
   for arithmetic. The likely answer is outermost with `ArgStrategy` restoring
   eagerness where it is wanted, but confirm it against the `If` and recursion
   cases before committing.

2. **Context representation.** `context_fit` needs an active context to match
   `WorksWellWith` against. What is that context concretely: the current goal
   concept, the last N episodes, an explicit context concept passed down the
   evaluation stack? Start with the smallest thing that makes the scoring
   testable.

3. **Exploration accounting.** When exploration picks a worse realization and
   it fails, that failure is partly the explorer's fault, not the
   realization's. Decide whether exploratory failures are weighted less in
   `success_rate` and say why.
