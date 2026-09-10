# Spoon: a development plan toward general intelligence

Written 2026-09-06. Saved incrementally during the session.

## The objective

Build an agent whose experience accumulates into reusable knowledge, better interpretation, executable abilities, and better decisions. It should progressively handle unfamiliar tasks by composing and adapting what it already knows, seek teaching when useful, and retain the consequences of success and failure across restarts.

The target is developmental generality. Spoon is allowed to be ignorant, ask questions, use its Teacher, make mistakes, and change during every measurement. Learning is part of its operation.

This is an engineering hypothesis, not a claim that a known recipe guarantees AGI. Its value depends on identifying the mechanisms that actually produce increasing competence and revising those that do not.

## Non-negotiable direction

- Keep Concepts as the shared representation for knowledge, programs and relationships.
- Keep evaluation, inference, candidate selection and planning in ordinary code. LLM work belongs to Ears, Mouth or Teacher.
- Let the Teacher provide examples, concepts, relationships, explanations and executable concept bodies. Choose the useful teaching route for the gap. Do not impose spec-only teaching as doctrine, and do not make direct program generation the only acquisition mechanism.
- Treat negative user feedback as evidence that must change behavior and can initiate relearning.
- Keep learning on during development and use. Measure the learning process without requiring a frozen brain.
- Preserve uncertainty and disagreement instead of silently turning a guess into an established fact.
- Prefer extending and connecting existing machinery to replacing it with another grand rewrite.

## What exists, and where this plan starts

The workspace has a real Rust Concept substrate, SQLite storage, term evaluator, native operations, backward inference, enumerative synthesis, consolidation, learned phrasings, three model seats and public interfaces. The full workspace tests passed before this session's implementation work.

Historical overnight logs report:

- General graded suite: 1,000/1,153 before experience, 1,022/1,153 afterward.
- Model readings on those passes: 367 before, 127 afterward.
- Trained facts suite: 147/234, with inverse relationships at 0/36 and transitivity at 22/60.

These are earlier-run observations, not measurements of the current edits. They support investigating reuse and identify failures; they do not establish a capability ceiling.

## Resume here: current working tree

The user requested a checkpoint before implementation. Commit `27d0633` adds three failing conversation-level regression tests in `crates/spoon-brain/tests/feedback_turns.rs`. The implementation before that checkpoint was already committed at `c47e7c0`.

Subsequent changes are UNCOMMITTED and INCOMPLETE. Do not describe them as finished:

- `crates/spoon-brain/src/brain.rs` now delegates hearing, teaching and execution to files under `crates/spoon-brain/src/brain/`.
- Added episode fields for the effective request, originating phrasing, created assertion row IDs and the episode being corrected.
- Connected negative feedback to the prior answer and Teacher context.
- Teacher-corrected readings can replace the current attempt.
- Question-shaped computations now retain their realization trace.
- Added exact assertion retraction and phrasing penalties.
- New phrasing entries reload with their durable identity and standing.
- Adjusted mouth accounting to distinguish returned template output from a recorded model exchange.

Latest focused result: two of three new feedback tests pass. The restart test still fails: the corrected answer is produced immediately, but reopening the brain loses the learned reading and returns the old behavior.

Investigation found that startup's stale-pair cleanup can remove valid phrasings if their structural wrapper symbols, such as `ask`, were never registered durably. Also, `seed_bootstrap` supplies fresh Activation values on every startup. A regression test in `crates/spoon-natives/tests/seed.rs` now confirms that restart seeding erases accumulated realization evidence: `restart_seeding_preserves_the_evidence_from_user_feedback` FAILED. Its output is in `/tmp/spoon-seed-before.log`. This is an independently reproduced blocker to durable learning, not just a suspected issue.

Other session outputs, if still present:

- `/tmp/spoon-feedback-baseline.log`: original workspace test run.
- `/tmp/spoon-feedback-before.log`: all three feedback reproductions failed before changes.
- `/tmp/spoon-feedback-after.log`: latest focused results.

First implementation work after this plan:

1. Fix valid learned phrasings disappearing on startup by persisting the symbols they legitimately reference. Do not simply disable stale-reference cleanup.
2. Verify and fix startup resetting acquired realization evidence and relevant concept metadata.
3. Finish the feedback loop's edge cases: correct target selection, feedback without a Teacher, repeated rejection, another session's facts, explicit replacement after intervening questions, and effectful operations that must not be replayed automatically.
4. Verify that a corrected reading exposing a genuinely missing capability can proceed to capability teaching in a bounded additional pass. The current early return after a reading correction does not complete that chain.
5. Format only changed files; run focused tests, then the workspace tests. Update STATUS only after this is wired and verified.
6. Review the uncommitted refactor carefully. It also changes inference error handling and retry behavior; preserve useful behavior rather than accepting the refactor wholesale.

## The architectural change with the largest potential payoff

Replace the mostly single-pass turn sequence with a persistent, bounded learning-and-action loop:

```
Observe input or outcome
  -> form candidate interpretations and goals
  -> retrieve relevant knowledge and abilities
  -> propose executable plans
  -> predict outcomes and check authority
  -> act or run an informative experiment
  -> compare observation with prediction and intent
  -> update the responsible knowledge and methods
  -> repair, continue, ask for help, or finish
```

Today the turn loop often treats a model reading, a computation and a response as sufficient structure. General agency requires goals that survive a turn, intermediate outcomes, explicit expectations and a way to revise the current approach.

Keep all semantic contents of this loop as Concepts. Rust structs may hold runtime scheduling, indexes, counters and trace bookkeeping. They should not become a second semantic model competing with Concepts.

The loop must have explicit per-turn and per-goal budgets. A failed approach should create useful evidence and a next option, not unbounded retries.

## Workstream 1: close the experience loop

This comes first because additional training is wasted when feedback does not reach the behavior that produced an answer.

### Durable causal records

For every attempt, retain:

- The actual request, candidate reading and selected reading.
- The phrasing and interpretation evidence that supplied it.
- The realization and rule choices, arguments and alternatives.
- The exact assertions and effects produced.
- Expected outcomes, observed outcomes and any user feedback.
- Links to attempts that this one repairs or supersedes.

A correction should not infer what happened from the wording of the Mouth's response. It should follow these links.

Keep rejected attempts. They are training material, counterexamples and explanations of why an alternative is preferred. A failure counts once for its observed event, not once per nested trace entry containing the same realization.

### Repair policy

On negative feedback:

1. Identify the target attempt using explicit references, task context and recency. If two materially different targets remain plausible, ask one short clarification.
2. Record the feedback even when there is no realization to penalize.
3. Distinguish likely interpretation, missing-knowledge, execution, selection and presentation failures. Preserve multiple hypotheses when necessary.
4. Reduce trust in the implicated route. A failed interpretation should not indiscriminately teach arithmetic that addition is unreliable.
5. Ask the Teacher with the original request, previous reading, actual result and full feedback. Sending only “that's wrong” discards the useful context.
6. Apply a corrected reading immediately. If it reveals a capability gap, teach that capability next, within the remaining budget.
7. Verify against available observations or examples, persist the acquisition, and retry the unresolved portion.
8. Never silently replay completed external writes. Record their receipts and request an explicit new action when compensation or repetition is needed.

When the Teacher is unavailable, retain a pending repair and avoid treating the rejected answer as newly confirmed merely because it executes again. Resume repair when resources are available.

### Acceptance evidence

A running brain gives a wrong answer, receives negative feedback, changes the responsible behavior, handles a related input, restarts, and retains the change. Include a misleading interpretation and a faulty learned program as separate cases. Scripted seats prove the wiring; a real Teacher run then tests whether the prompts and model actually supply useful repairs. Neither substitutes for the other.

## Workstream 2: persistent goals and executable plans

Add a small goal executive above evaluation, inside `spoon-brain`, using `spoon-eval` and `spoon-infer` as its execution and derivation services.

Possible ordinary Concept vocabulary:

```
goal<g, desired-condition>
subgoal<g, child>
requires<action, condition>
predicts<action, outcome>
achieved-by<condition, action>
blocked-by<g, missing-condition>
attempt-of<a, g>
observed-after<a, observation>
```

These are proposed names, not claims that the current resolver supports them.

Start with bounded backward search from a desired condition to available actions. Use native and learned realizations as candidates. Bind parameters from facts and earlier results. Prefer executable plans that satisfy the current goal with fewer unsupported assumptions and manageable cost.

Do not start by solving unrestricted planning. First make this sequence real:

- A user supplies a small dataset and asks for a derived report.
- Spoon identifies required fields, computes intermediate results, notices missing information, asks for it, and resumes the same goal.
- If a transformation is missing, the Teacher and synthesizer add it.
- If a later result contradicts an earlier assumption, only affected work is reconsidered.

Completion must be tied to a checkable goal condition, including partial completion and unresolved conditions. A fluent response is not itself completion.

Persist goals, bindings, completed steps and effect receipts so an interrupted task resumes without repeating external work.

## Workstream 3: make interpretation a revisable proposal

The Ears need not deliver a single irrevocable meaning. For ambiguous inputs, let them produce a small candidate set with unresolved references and explicit uncertainties.

The interior can compare candidates using available facts, argument contracts, active goals, conversation history and observed contradictions. This is code choosing among structured proposals, not an interior LLM call.

Begin with two or three candidates only when ambiguity is detected. Do not multiply model cost for obvious inputs.

Add Concept-level callable contracts such as:

```
accepts<text-reverse, text>
accepts<list-reverse, list>
returns<text-reverse, text>
role<employment, employee, 0>
role<employment, employer, 1>
```

Implement a validator over these declarations. Contracts narrow candidate interpretations and synthesis search, but unknown contracts remain unknown rather than becoming fabricated certainty.

Keep entity identity separate from surface similarity. A spelling near another entity's name is evidence to consider, not permission to merge them. Entity slots in learned phrasings must bind the new entity rather than retain the training example's identity.

Teach reusable language rules through the existing Teacher and phrasing store. Keep dependencies from a phrasing to the concepts and contracts it assumes, so a changed concept triggers revalidation instead of arbitrary deletion or permanent stale reuse.

## Workstream 4: acquire programs at multiple scales

Keep both direct Teacher composition and synthesis from examples. They have different useful search properties and should support each other.

For each capability request:

1. Search the existing library for a directly usable realization.
2. Try adapting a related learned body by binding arguments or replacing a small subtree.
3. Request a Teacher candidate and/or examples according to the uncertainty.
4. Validate candidates through the real evaluator.
5. Retain competing realizations with their applicability conditions and evidence.
6. Use later feedback to specialize, repair or demote them.

### Improve synthesis before enlarging its budget

The current enumerator builds from a broad operator set. Increasing size limits alone can spend more time exploring nonsense.

Add these changes in order:

- Contract-guided candidate construction so text operators are not repeatedly tried on unrelated values.
- Target-relevant operator retrieval from descriptions, relationships and successful nearby programs.
- Local repair around an existing candidate before global enumeration.
- A minimum set of diverse distinguishing examples, with counterexamples accumulated from real failures.
- Hierarchical synthesis that invokes previously learned subprograms instead of repeatedly rediscovering their bodies.

Example: a learned report transformation almost works except that one field uses the wrong aggregation. Search for a replacement subtree under the existing transformation, rather than enumerate complete report programs from scratch.

A useful test of acquisition is whether teaching an intermediate capability makes a later, different capability cheaper to obtain. Record which earlier program was reused. A native added by the developer should be tracked as a developer contribution, not attributed to Spoon's learning.

## Workstream 5: learn conditions, not just global winners

A realization's global success rate conflates different situations. A program may work for nonempty lists and fail for empty lists, or perform well in one domain and poorly in another.

The evaluator already supports situation Concepts. Populate them from actual argument properties, active goals and domain context, then attachbserved success or failure to that situation. Verify the public turn path supplies this context; isolated selector tests are insufficient.

Use the existing `works-well-with` and `works-poorly-with` relationships where appropriate. Learn simple applicability conditions from contrasting examples before inventing a complex context model.

Keep exploration for computations where experimentation is acceptable. Do not run speculative external writes merely to collect evidence.

Handle distribution changes explicitly: recent repeated failures should be able to overcome a large historical success count. Preserve the history, but allow a changed environment or revised concept to define a new applicability context.

## Workstream 6: maintain beliefs and consequences

Facts, interpretations and rules need support links. A conclusion should be traceable to the assertions and rules that support it.

Implement dependency-aware revision:

- Retracting one assertion invalidates conclusions whose support depended on it.
- Independent support can keep a conclusion alive.
- Direct contradictory observations can defeat defaults.
- Exhausted search is distinct from “false” and from “not currently known.”
- Multiple competing claims can coexist with provenance while a task requests clarification or gathers evidence.

The current inference engine cuts recursive branches and can under-answer. Add proper tabling and fixpoint handling for supported recursive rules when the tests that need it are in place. Share resource accounting across inference, evaluation and planning, even if they remain separate modules.

Do not equate a high scalar confidence with a proof. Retain the actual supports and the circumstances in which they were observed.

## Workstream 7: learn from a world that can disagree

General competence needs more feedback than a Teacher judging its own outputs. Give Spoon environments in which actions produce independently observable consequences.

Begin with inexpensive, bounded environments:

- Files and structured data: transformations can be checked against source records and user constraints.
- A simulated inventory or scheduling world: actions change explicit state, and goals have observable completion conditions.
- A small code workspace: proposed changes face build results, tests and observed behavior.

The reason for simulation is access to many informative interactions at low cost. Success there establishes competence in that environment, not general competence everywhere.

For each action, store a prediction before executing and compare it with the resulting observation. A mismatch is a learning signal even when the user says nothing. Distinguish an incorrect world model, an execution failure and an unobserved outcome.

Learn transition relationships from repeated observations and use them to simulate candidate plans. Begin with explicit deterministic transitions, then add competing or probabilistic outcomes where evidence requires them. Do not assume observational correlation establishes causation. Use permitted interventions or retain the ambiguity.

All environments should use the same goal, action, observation and evidence interfaces. Domain-specific adapters provide observations and effects; they do not choose the agent's plan.

## Workstream 8: direct learning toward useful uncertainty

Add a durable agenda with three sources:

1. Unfinished user goals and explicit corrections.
2. Repeated costly failures or repeated Teacher dependence.
3. Missing distinctions that block several otherwise promising plans.

Select learning work using a small, inspectable code policy. Estimate its value from the number and importance of blocked goals, likelihood of obtaining a useful distinction, expected reuse and resource cost. Treat those estimates as revisable heuristics, not mathematically justified probabilities before evidence exists.

When two programs fit the available examples but differ elsewhere, search for an input on which their predictions disagree. Ask the user, Teacher or environment for that outcome. This makes the next teaching interaction informative rather than merely longer.

For example, two learned shipping rules may agree on small packages but diverge above a weight boundary. A discriminating weight is a better next lesson than another small package.

Persist the agenda and cap its consumption. Background study should not make ordinary requests wait indefinitely. Implement scheduling first as an explicit Spoon command or service component; do not create outside automations as a substitute for an executive.

## Workstream 9: compress experience into transferable methods

Consolidation should produce usable abstractions, not just additional concept names.

Extend the existing anti-unification and program consolidation machinery to:

- Identify repeated subprograms across successful capabilities.
- Extract parameters and applicability conditions.
- Rewrite existing users to call the abstraction.
- Verify behavior preservation on stored examples and counterexamples.
- Make the abstraction discoverable to planning and synthesis.
- Retain its benefit and failure history so unused or misleading abstractions can lose standing.

Then try structural transfer between domains. Match relational structure and program roles rather than just similar names.

Example: grouping transactions by account and grouping measurements by device can share a grouping-and-aggregation method while retaining different domain meanings. The transfer is real when the second capability uses the first method with new bindings and requires less new teaching.

Keep analogy proposals provisional. Shared structure suggests a candidate program or relationship; it does not license asserting that the domains behave identically.

Longer term, represent search and teaching strategies as selectable methods with their own observed costs and outcomes. Spoon can then improve how it seeks a solution using the same evidence machinery. Start with a finite set of inspectable strategies. Do not begin with unrestricted self-modification of the Rust runtime.

## Model seats and resource allocation

No particular model name is the architectural solution. Model quality matters at the boundaries, and the system should be able to allocate it where it changes the outcome.

### Ears

Use the cheapest route that reliably maps the current request into usable structure: proven learned phrasings, a capable parser model, then escalation for unresolved ambiguity. Let stronger readings improve the reusable native path.

Record which concept vocabulary, contracts and contextual facts were supplied. A model blamed for not using a capability it was never shown is an avoidable integration failure.

### Teacher

Use the strongest affordable Teacher for difficult acquisition or diagnosis when cheaper attempts stop producing useful changes. A slow call that creates a reusable ability can be more valuable than many cheap calls that repeat failed advice.

Judge Teacher use by acquisitions that are subsequently executable and reused, repairs that actually change behavior, and distinctions that unblock goals. Count all attempts, latency and cost, including malformed replies and examples that cannot be used.

Before changing providers, verify effective runtime configuration and record it in the episode. The project already suffered from settings that were not read. A model comparison without verified routing compares unknown conditions.

Do not request every teaching form on every gap. Reading repair, additional facts, a candidate body and discriminating examples are different needs. Begin with a rule-based route selector; accumulate enough history to improve it from outcomes.

### Mouth

Use direct rendering for simple results and model wording when it contributes something useful. Preserve the structured answer and its uncertainty. Surface negative feedback and incomplete repairs plainly.

### Shared budget

Track time and calls per active goal, not just per individual evaluation. Avoid a series of individually bounded phases producing an effectively unbounded task.

Reserve part of the budget for verification and recovery. Persist unfinished learning requests when the budget expires so the next session can continue rather than repeat all discovery.

## Develop through expanding experience

Start with a domain that offers repeated useful tasks, clear outcomes and room for composition. Structured-data work is a strong candidate because Spoon already has collection, arithmetic, text and store operations.

A developmental progression:

1. Learn the user's vocabulary, entities and basic transformations through actual use.
2. Combine transformations into reusable reports and queries.
3. Introduce missing data, changing facts and corrections.
4. Acquire a missing operation through examples or a Teacher body.
5. Reuse that operation inside a larger task.
6. Introduce a related domain with different entities and terminology but shared structure.
7. Add a domain requiring genuinely different actions and outcome models.
8. Revisit earlier work and repair regressions or context-specific failures.

This progression is an experience generator, not a demand that an untrained system immediately pass a broad capability exam. The pace should follow what it learns and what repeatedly blocks it.

Persist a curriculum as ordinary tasks plus expected or observed outcomes. Where a lesson supplies an answer, record it as teaching. Where the system obtains an answer from an environment, record the observation. Both are legitimate learning, and their sources should remain distinguishable.

## Know whether development is moving

Keep learning enabled. Observe the trajectory rather than requiring a fixed model snapshot.

Useful records include:

- Which tasks became possible after a specific teaching event.
- Which earlier capabilities a new solution reused.
- How many repeated corrections were needed before behavior changed.
- Whether the corrected behavior survives restart and later interference.
- Teacher calls and time spent per completed goal over successive encounters.
- Which past capabilities deteriorate after new experience, and whether context specialization repairs them.
- How often the agent notices a failed prediction and repairs it without user prompting.
- What prevents a goal from finishing: knowledge, interpretation, planning, execution, resources or authority.

Measure each prediction or attempt before its resulting feedback is incorporated, then let the feedback update the brain immediately. This records online learning honestly without stopping it. Include help received and cost incurred, rather than treating assistance as disqualifying.

Do not judge progress from library size, assertion count or native-ear percentage alone. A larger library can contain unreachable programs, and a lower model-call count can mean a wrong phrasing is being reused confidently.

For investigating a particular implementation change, reproducible component tests and matched small experiments are useful diagnostic tools. They are not the definition of Spoon's intelligence, and they do not require turning off its lifelong learning in normal operation.

## Build order and concrete deliverables

### A. Preserve what experience teaches

Finish the in-progress feedback work and startup persistence fixes. Deliver a conversation trace showing a user correction reaching the responsible episode, a revised attempt, and subsequent reuse after restart. Cover unavailable teaching and external effects. Bring STATUS and runnable examples into agreement with the code.

### B. Give tasks continuity

Implement persisted goals, bindings, outcomes and resumable plans. Demonstrate a useful task interrupted by missing information, then resumed with that information. Add budget accounting across the complete goal.

### C. Turn errors into targeted acquisition

Unify the repair agenda with Teacher requests and local program repair. Retain counterexamples and retry the unresolved subgoal. Demonstrate one case requiring interpretation repair followed by capability acquisition, and another requiring a changed realization under the same interpretation.

### D. Make reuse compound

Add contracts, contextual applicability and local synthesis repair. Connect consolidation output to actual retrieval and planning. Demonstrate a new capability built using something learned earlier, with explicit dependency links.

### E. Ground and transfer

Add outcome-producing environments and prediction errors. Expand across domains while revisiting earlier abilities. Demonstrate structural transfer, automatic detection of an incorrect prediction, and a repaired plan.

These milestones are dependency gates, not calendar promises. Continue ordinary teaching throughout. Do not wait for the entire architecture before giving Spoon useful experience.

## Where to implement

- `spoon-concept`: retain shared structure; add no separate Fact/Action/Program ontology. New semantic vocabulary is seeded or learned as Concepts.
- `spoon-store`: assertion and episode lineage, resumable goals, evidence dependencies, and schema-compatible persistence. Preserve learned state across bootstrap and import/export.
- `spoon-eval`: execution traces, contextual dispatch, shared budgets and effect receipts where appropriate.
- `spoon-infer`: derivation supports, clear exhaustion results, dependency-aware revision and recursive tabling.
- `spoon-brain`: goal executive, feedback targeting, bounded repair cycles and coordination between existing modules. Split modules by responsibility rather than regrowing `brain.rs`.
- `spoon-learn`: counterexamples, contract-guided search, candidate repair, transferable abstractions and learning agenda policies.
- `spoon-ears`: candidate readings, durable phrasing identity, entity binding, ambiguity and structured contracts in prompts.
- `spoon-teach`: targeted teaching requests, verified routing, candidate programs and discriminating examples.
- `spoon-mouth`: faithful rendering of results, uncertainty and incomplete repairs.
- `spoon`: developmental traces, resume commands, curriculum execution and inspection of what was actually learned.

## Decisions deliberately left experimental

Do not precommit to one scalar confidence formula, one exploration algorithm, one curriculum, one model or one synthesis grammar as the answer. Record the hypothesis and test the smallest consequential behavior.

Examples of reasons to change course:

- Contracts reduce invalid candidates but exclude useful compositions: represent uncertain or polymorphic contracts instead of making them rigid types.
- Global negative evidence damages valid contexts: specialize evidence and preserve the good contexts.
- Teacher-generated examples repeatedly endorse the same wrong assumption: seek an environment observation or a user distinction.
- More memory reduces retrieval quality: improve relevance and dependency handling before generating more memory.
- A planner repeatedly cannot finish despite having all the operations: inspect goal decomposition and binding, not just the model.
- Broad search remains expensive after narrowing: change the representation of search states or use hierarchical methods before multiplying the budget.

## The central bet

Spoon's opportunity is to make each useful interaction leave behind something operational: a corrected interpretation, a supported belief, a program, an applicability condition, a predictive model, a reusable plan or a better search method.

If those artifacts remain reachable, compose with one another, and change when experience contradicts them, development can accumulate. The next architecture should be built around making that accumulation reliable and increasingly self-directed.
