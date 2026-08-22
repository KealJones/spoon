# EKG Implementation Plan

This is the phased implementation plan for building EKG (Executable Knowledge
Graph) as described in WHAT-IS-EKG-v3.md. It maps the conceptual architecture
to concrete engineering work.

## Decisions Made

- **Runtime**: Rust core engine + TypeScript interaction shell
- **Teacher**: Abstract provider interface. Claude via `claude -p` CLI (SSO auth),
  OpenAI via API, Ollama for local/cheap self-supervision, human via CLI
- **Bootstrap domains**: Math/logic (Tier 1 signal), programming (execution
  grounding), then general multi-domain
- **Goal**: Usable open-source product
- **Persistence**: SQLite for graph + episodes (simple, embeddable, inspectable)
- **Protocol**: JSON-RPC over stdio between Rust server and TS shell
- **Capability substrate**: A deliberately small, policy-enforced native
  primitive set (network request, scoped file access, observation, and
  sandboxed execution). Everything richer is acquired as typed, contracted,
  decomposable knowledge rather than added as a privileged one-off tool.
- **Capability portability**: Reconstructible, content-addressed bundles may
  carry procedures, dependency graphs, schemas, tests, and provenance. They
  never carry trust, ambient authority, secret values, or unverified
  environment assumptions. Imports enter quarantine as Provisional and must
  pass local permission checks and local revalidation before promotion.

### Execution model routing and spend policy

The coordinator should normally run on **GPT-5.6 Luna**. It owns the durable
handoff, selects the next bounded task, runs focused checks, and delegates only
when the risk or novelty warrants it. This keeps the long-running project
moving cheaply without treating a low-cost model as the final authority on
safety-critical design.

| Work type | Default model / effort | Escalate when | Completion standard |
| --- | --- | --- | --- |
| Task triage, repository reading, scratchpad/handoff updates, formatting, focused test runs, fixture generation, mechanical TypeScript/Rust edits | Luna / low–medium | The edit changes a security, concurrency, persistence, or public-contract invariant | Local test plus a diff review by the coordinator |
| Normal feature implementation, multi-file refactors, integration tests, debugging ordinary failures, API/schema design | Terra / medium–high | Two failed repair attempts, cross-crate invariants, or unclear plan/spec interaction | Focused suite and strict static checks |
| Threat modeling, adversarial audit, durable recovery/concurrency design, capability sandbox/permission policy, mutation authorization, ambiguous architecture decisions | Sol / high | Always use for the named risk areas; do not use merely for a larger mechanical task | Written invariant analysis plus adversarial regression tests |
| Independent final review of a phase | Terra / high, then Sol / high only for Phase 4–5 security/self-modification gates | A finding is disputed or the phase changes trust/authority boundaries | Requirement-by-requirement evidence, not only a green suite |

**Delegation protocol.** Luna may keep several independent cheap tasks moving,
but it should use at most one Terra or Sol task per shared mutable area at a
time. Before any handoff it records: exact files owned, invariant being proved,
commands to run, and the next recovery point in `.agents/scratchpad/ekg/HANDOFF.md`.
The coordinator integrates changes and reruns the affected tests; a subagent's
claim of success is never final evidence on its own.

**Phase routing.**

| Phase | Luna owns | Terra owns | Sol use (strictly limited) |
| --- | --- | --- | --- |
| P2 remaining gate | regression execution, metric fixtures, docs, mechanical API wiring | cross-crate integration and test repair | final trust/durability/adaptation audit |
| P3 intuition/self-supervision | corpus preparation, benchmark runners, data plumbing | ranking/retrieval implementation and evaluation | only if learned ranking could alter authority or promotion decisions |
| P4 consolidation/skill discovery | regression fixtures, reporting, routine package work | promotion gates, discovery pipeline, reconciliation | adversarial promotion and rollback review |
| P5 curiosity/self-modification and capability acquisition/sharing | bundle fixtures, docs, import/export round trips, routine tests | native primitive implementations, typed procedure generation, local revalidation | sandbox escape, permissions/effects, secrets, provenance, and self-modification authorization |
| P6 inspector/metrics | dashboard data transforms, snapshots, visual/test maintenance | server/SDK/dashboard integration and performance work | only for a security-sensitive exposure of evidence or authority |

The practical switch policy is: start a bounded task on Luna; retry once there
if the failure is mechanical; move to Terra for substantive code or a second
failed repair; reserve Sol for a named high-risk invariant or final adversarial
review. Do not use GPT-5.5 for this workflow: it is not the cost-efficient
choice relative to Terra for the same class of work. The model picker’s live
credit estimate remains authoritative for this account; this routing is a
quality/cost policy, not a guarantee of app-specific credit consumption.

## Repository Structure

```
ekg/
  crates/
    ekg-core/          # data model: concepts, relationships, contracts,
                       # mutability classes, confidence, scope, evidence
    ekg-graph/         # persistent knowledge graph (SQLite-backed)
    ekg-exec/          # procedure execution engine
    ekg-episode/       # episode recording, storage, replay
    ekg-credit/        # credit assignment: contracts, replay, statistics
    ekg-reason/        # reasoning engine: contract-guided composition
    ekg-adapt/         # adaptation + knowledge reconciliation
    ekg-capability/    # native primitives, interface discovery, capability
                       # validation, portable bundle import/export
    ekg-engine/        # orchestrator: the full cycle from section 11
    ekg-server/        # JSON-RPC server exposing the engine

  packages/
    @ekg/cli/          # TUI + REPL for interacting with EKG
    @ekg/teacher/      # teacher abstraction + provider adapters
    @ekg/inspector/    # web dashboard: graph viewer, episode browser,
                       # metrics dashboard (section 38)
    @ekg/sdk/          # TypeScript client for ekg-server

  tests/
    kitchen/           # the running example from the doc, as integration tests
    math/              # math/logic domain bootstrap tests
    programming/       # programming domain bootstrap tests
    falsification/     # section 38 metric measurement harness
```

---

## Phase 0: Seed

**Maps to**: Section 33, Stage 0
**Goal**: Primitives, execution, episode recording, evaluation. Nothing learned
can be graded until evaluation exists, nothing can be credited until episodes
are recorded.

### P0.1 - Core Data Model (`ekg-core`)

The foundational types everything else builds on.

```
Concept
  - id, name
  - mutability_class: Definitional | DefeasibleGeneral | Particular |
                      Procedural | Normative | CoreMachinery
  - confidence: { support, contradiction, scope, sources, last_tested }
  - lifecycle: Active | Validated | Provisional | Stale | UnderReview |
               Superseded | Retired | Invalid

Relationship
  - source, target, kind, strength
  - scope conditions
  - evidence (episodes that support it)

Procedure
  - id, name
  - body: a tree of PrimitiveOp | ProcedureCall | Conditional | Iterate
  - contract: { requires, promises, fails_when, costs }
  - version history
  - test cases (self-growing regression suite)

Episode (section 18)
  - situation, interpretation_candidates (including losers)
  - context_assembled (with assumptions marked)
  - knowledge_considered (surfaced + rejected)
  - reasoning_trace (steps with contracts matched)
  - prediction
  - action
  - observed_result
  - evaluation (tier, verdict)
  - cost (rung_reached, budget_spent)

Evidence
  - tier: Hard | Consensus | Deferred
  - source, timestamp
  - linked episode
```

**Deliverable**: A Rust crate with these types, serialization (serde), and
a small set of primitive operations (arithmetic, comparison, string ops).

### P0.2 - Knowledge Graph (`ekg-graph`)

Persistent storage for concepts, relationships, and procedures.

- SQLite-backed (single file, inspectable, embeddable)
- CRUD for all entity types
- Relationship traversal (typed, bounded hops)
- Basic query: find concepts by name, by relationship, by type
- Dependency tracking: "what depends on this concept/procedure?"
- Version history for procedures and contracts

**Not yet**: similarity search, learned ranking, compression. Those come in
later phases when episodes exist to train on.

**Deliverable**: A graph you can populate, query, and traverse. Inspectable
via `sqlite3` directly if needed.

### P0.3 - Execution Engine (`ekg-exec`)

Run procedures and produce results.

- Execute procedure bodies against a set of bound inputs
- Pure execution only (no side effects) - section 16's safe tier
- Primitive operations: arithmetic, comparison, string, list, lookup
- Procedure composition: call one procedure from another
- Contract checking: validate requires before execution, check promises after
- Execution trace capture: every step recorded for replay
- Timeout/budget: bounded execution, anytime interruption

The procedure representation should be:
- **Neutral**: not "TypeScript" or "Python" - an internal representation
  reducible to its parts (section 6)
- **Decomposable**: composed skills remain inspectable as constituent parts

**Deliverable**: Can define `DOUBLE(x) = x * 2`, execute it, capture the
trace, verify the contract.

### P0.4 - Episode Recording (`ekg-episode`)

The raw material of every learning mechanism downstream.

- Record full episodes per section 18's structure
- Persist to SQLite (same DB or separate - TBD)
- Query: by time, by concepts involved, by outcome, by rung
- Replay: re-run a stored trace with substitutions (foundation of
  credit assignment in P3)
- Never delete failed episodes

**Deliverable**: Episodes are recorded during execution, stored, queryable,
and the trace is replayable.

### P0.5 - Evaluation (`ekg-core` or `ekg-engine`)

The grading machinery. Without this, nothing learned can be judged.

- Verifiability tier classification (section 19)
  - Tier 1: deterministic check (test pass/fail, arithmetic, types)
  - Tier 2: independent methods agree, inverse recovers input
  - Tier 3: human judgment, deferred, weak
- Comparison: predicted vs observed result
- Surprise detection: did the result differ from prediction?
- Decomposition helper: break a weakly-verifiable goal into checkable
  subgoals (section 20's signal manufacturing)

**Deliverable**: Given a prediction and an observation, produce a tiered
evaluation with surprise signal.

### P0.6 - Server + Basic CLI

- `ekg-server`: JSON-RPC over stdio, exposes graph CRUD, procedure
  execution, episode query
- `@ekg/cli`: minimal CLI that connects to the server, lets you:
  - Define concepts and relationships manually
  - Define and execute procedures
  - Inspect the graph
  - View episodes
- `@ekg/sdk`: TypeScript client wrapping the JSON-RPC protocol

**Deliverable**: A running system you can interact with. Dumb, but
functional. "Stage 0" from section 33 is complete.

### P0 Exit Criteria

- Can define concepts, relationships, procedures with contracts
- Can execute procedures and capture traces
- Can record and query episodes
- Can evaluate results by tier
- Can replay a trace
- Kitchen test: manually define DOUBLE, execute it, record the episode,
  verify the trace, replay with a substitution

---

## Phase 1: Interpretation + Teacher

**Maps to**: Section 33, Stages 1-2
**Goal**: Understand input well enough to act, leaning heavily on a teacher.
Expect high teacher dependence - this is correct, not failure.

### P1.1 - Teacher Abstraction (`@ekg/teacher`)

```typescript
interface Teacher {
  propose(request: TeacherRequest): Promise<TeacherProposal>
  reliability(): SourceReliability  // tracked over time
}

interface TeacherRequest {
  situation: string
  context: KnowledgeContext
  specific_question?: string
  desired_output: ProposalSchema
}

interface TeacherProposal {
  content: StructuredProposal
  source: string
  status: 'unverified'  // NEVER automatically true
}
```

Provider adapters:
- **ClaudeTeacher**: spawns `claude -p`, sends structured prompts, parses
  JSON output. Uses system prompt that instructs Claude to output proposals
  in EKG's schema, not just answers (section 30: extract the lesson, not
  the answer)
- **OpenAITeacher**: standard HTTP API calls
- **OllamaTeacher**: local models, cheap, good for high-volume self-supervision
- **HumanTeacher**: CLI prompt, waits for human input

All teacher output goes through the validation pipeline (section 30):
```
proposal -> validate -> verified | rejected | provisional
```

Source reliability is tracked per teacher over time.

**Deliverable**: Can ask any teacher a structured question, get a proposal
back, validate it, and integrate or reject it with provenance.

### P1.2 - Interpretation (section 12)

Map natural language input to internal concepts.

Initially teacher-dependent: send input to teacher, get back candidate
interpretations with weights.

```
"Could you double that?" ->
  [
    { meaning: DOUBLE, weight: 0.91 },
    { meaning: REPEAT, weight: 0.06 },
    { meaning: UNKNOWN, weight: 0.03 }
  ]
```

Key properties:
- Ambiguity preserved, not collapsed (weights sum to 1)
- Unresolved ambiguity is a legitimate output
- Losing interpretations stored in the episode (needed for credit assignment)

Later (P3): learn interpretation from accumulated episodes, reducing
teacher dependence.

**Deliverable**: Natural language input -> weighted candidate meanings
referencing graph concepts. Teacher-powered initially.

### P1.3 - Context Assembly (section 13)

Build the active working context for a task.

- Current goal and why
- Entities under discussion
- Relevant knowledge (graph neighborhood of active concepts)
- Recent actions and results
- Active assumptions (MARKED as assumptions - critical for credit assignment)
- Environmental state
- Budget remaining

Initially: simple heuristic assembly (graph neighbors of mentioned concepts,
recent episodes). Later (P3): learned context selection.

**Deliverable**: Given an interpreted input, assemble a bounded context
from the graph and episode history.

### P1.4 - Minimal Reasoning Cycle (section 11, simplified)

Wire together interpretation -> context -> reasoning -> execution ->
evaluation into a single loop.

- Interpretation: teacher-assisted (P1.2)
- Context: heuristic assembly (P1.3)
- Intuition: SKIP for now, use everything in context
- Reasoning: try to match a known procedure's contract, or ask teacher
- Execution: run procedure (P0.3)
- Evaluation: grade result by tier (P0.5)
- Episode: record everything (P0.4)

Metacognition: simple escalation ladder (section 17)
  1. RECALL - do I already know?
  2. RUN - do I have a procedure?
  3. ASK - teacher
  4. ABSTAIN - say so

**Deliverable**: Give EKG a task, it attempts to solve it (badly), and
records the full episode. The kitchen test should partially work.

### P1 Exit Criteria

- Teacher integration working with at least one provider
- Can receive natural language, interpret it, attempt to solve it
- Full episodes recorded for every attempt
- The escalation ladder works (recall -> run -> ask -> abstain)
- Can learn from teacher proposals (with validation)
- Kitchen test: "what is double 7?" works end to end

---

## Phase 2: Credit Assignment + Adaptation

**Maps to**: Sections 21-23, the hardest part
**Goal**: When something goes wrong, identify what was responsible and make
a targeted correction. This is Claim 1 - if it doesn't work, nothing else
matters.

### P2.1 - Contract Violation Detection (section 21, mechanism 1)

Cheapest, sharpest credit assignment.

- Walk the reasoning trace
- Check each procedure's contract: was any precondition violated? Did any
  postcondition fail?
- If yes: that's the suspect, high confidence, one pass, no re-execution

**Deliverable**: Given a failed episode, detect contract violations in the
trace. Cost: O(trace length).

### P2.2 - Statistical Attribution (section 21, mechanism 3)

Cheap, blurry. Used to rank candidates for replay.

- Track per-element failure rates across episodes
- When a failure occurs, rank elements by historical failure rate
- Distinguish correlated elements (SCALE_ALL and linear scaling co-occur)
- Output: ranked suspect list, NOT conclusions

**Deliverable**: Given a failed episode, produce a ranked list of
suspects from cross-episode statistics.

### P2.3 - Counterfactual Replay (section 21, mechanism 2)

Strong, expensive. The mechanism that justifies executable knowledge.

- Take a failed trace
- For the top-K suspects (from P2.1 + P2.2):
  - Re-run the trace with exactly one element changed
  - Observe whether the outcome changes
- Deterministic replay (swap an operation) -> potentially decisive
- Simulated replay (swap a planning decision) -> strong evidence
- Track attribution confidence and provenance

Cost controls:
- Only replay top-K suspects (guided by mechanisms 1 and 3)
- Budget-bounded: stop if cost exceeds value
- Track attribution cost as a fraction of total cost (metric 8 in section 38)

**Deliverable**: Can identify the responsible element in a failed trace
with confidence levels. The three mechanisms compose: contract check first,
statistical ranking to prioritize, replay to confirm.

### P2.4 - Adaptation (section 22)

Turn attribution into targeted change.

Correction scope, narrowest first:
1. **Record only** - input was unusual, change nothing
2. **Fix assumption** - context was wrong, not the procedure
3. **Narrow scope** - add a discovered condition to the contract
4. **Replace procedure** - it fails inside its own scope
5. **Revise concept** - requires corroboration, offline

Evidence thresholds per correction width (section 22):
- Record only: 1 episode
- Narrow scope: 1-2 episodes, Tier 1 or 2 evidence
- Replace procedure: several episodes, must beat incumbent
- Revise concept: many episodes, corroborated, offline only

Attribution confidence gates the response:
- Contract violation, certain -> act now, narrowly
- Replay-confirmed -> act now
- Statistical suspicion only -> schedule a test, wait for more episodes

**Deliverable**: Failed episodes produce specific, local modifications
rather than general dissatisfaction.

### P2.5 - Knowledge Reconciliation (section 22)

When something changes, propagate consequences.

- Follow dependency structure outward from the changed element
- For each dependent: still valid? narrower scope? stale? invalid?
- Invalidation is NOT deletion - check for alternative justifications
- History records are immutable; current understanding is revisable
- Lifecycle states: active, validated, provisional, stale, under_review,
  superseded, retired, invalid

**Deliverable**: A concept revision propagates through dependent knowledge
without naive cascade deletion.

### P2.6 - Contradiction as Refinement (section 23)

When two sources disagree, default to scope refinement, not arbitration.

- Detect contradiction: two claims with conflicting implications
- Search for discriminating feature between supporting cases
- If found: split the claim into two scoped claims (both can be true)
- If not found: hold the contradiction as a first-class object
  - Reasoning that depends on it inherits uncertainty
  - It becomes a curiosity target (P4)
  - It may resolve later from unrelated observation

**Deliverable**: Contradictions produce scope refinements or held
contradictions, never silent averaging.

### P2 Exit Criteria

- Credit assignment works on injected faults (metric 7)
- Attribution cost is reasonable relative to total cost (metric 8)
- Adaptations are narrow and targeted
- Knowledge reconciliation propagates without cascading destruction
- Contradictions refine scope rather than destroy information
- Kitchen test: flat pancakes -> leavening rule scope correction

---

## Phase 3: Intuition + Self-Supervision

**Maps to**: Section 33, Stage 3 + section 24
**Goal**: First compounding step. Search gets cheaper, more problems
become reachable, more episodes accumulate.

### P3.1 - Recall Index (section 14, stage 1)

Cheap, broad candidate generation without scanning everything.

- Similarity index over concepts (embedding-based, using a local model
  or teacher-generated embeddings)
- Typed traversal: follow relationships of the right kind, bounded hops
- Activation spread: start from context, propagate with decay
- Recency and frequency weighting

Output: hundreds of candidates, unranked, cheaply obtained.

### P3.2 - Learned Ranking (section 14, stage 2)

Score candidates using accumulated experience.

- Training data: episodes record what was considered, what was used,
  what succeeded, what was considered and rejected
- Model: small learned ranker (could be a local model fine-tuned on
  episodes, or a simpler ML model)
- Output: ordered shortlist small enough to reason over

### P3.3 - Representation Self-Supervision (section 24)

Train on episode structure without changing beliefs.

Safe targets (grounded in actual episodes):
- Predict missing words/phrases from context
- Predict validated interpretation from utterance + context
- Predict useful knowledge from situation + goal
- Predict next successful step from reasoning prefix
- Predict which procedure succeeds from candidates

Boundary: may freely update representations, similarity, retrieval,
interpretation probabilities, ranking policies. May NOT declare world
claims true, resolve contradictions, promote abstractions.

### P3.4 - Epistemic Self-Supervision (section 24)

Self-generated challenges that terminate externally.

- Hide a computation, predict, execute to check
- Invert a known skill, check the round trip
- Stress a contract boundary
- Predict a consequence, compute to verify

Grounding requirement: a meaningful fraction must terminate in execution,
a test, or an observation. "The graph agreed with itself" is not
acceptable.

Rationed, not unlimited. Track belief provenance by depth - beliefs
supported only by other beliefs are flagged for re-verification.

### P3 Exit Criteria

- Recall index returns relevant candidates without scanning everything
- Learned ranking improves search (fewer candidates explored for same
  results - metric 5, rung distribution)
- Self-supervision trains representations without belief drift
- Grounding ratio is tracked (metric 10)
- Problems increasingly resolve at cheaper rungs (metric 5)

---

## Phase 4: Consolidation + Skill Discovery

**Maps to**: Section 33, Stage 4 + sections 26-28
**Goal**: Enough volume for patterns to be real. Regression suite large
enough to gate promotion. First test of Claim 2.

### P4.1 - Skill Discovery (section 26)

Three routes:

1. **From repetition**: detect shared structure across episodes
   (multiple scaling tasks -> SCALE_RECIPE skill)
2. **From a single success**: explain WHY it worked, generalize along
   the explanation (not surface similarity)
3. **From failure**: generalize the failure into a critic (a precondition
   check that prevents a class of future failures)

### P4.2 - Promotion Gate (section 27)

The most important safety mechanism.

Candidate must demonstrate measurable win against incumbent:
- **Correctness**: same or better on every replayed verified episode.
  NON-NEGOTIABLE.
- **Compression**: shorter traces for same results
- **Search cost**: fewer candidates explored
- **Coverage**: solves episodes the incumbent could not
- **Transfer**: helps in a domain it wasn't derived from (strongest signal)

Pipeline: replay against history -> must win on at least one, lose on
none -> shadow deploy alongside incumbent -> promote on live win

### P4.3 - Self-Growing Regression Suite (section 27)

Every episode with a Tier 1 or 2 verified answer becomes a permanent test.

- Grows automatically with use
- Free (episodes already stored)
- Directly counters catastrophic forgetting
- Makes "did we get better?" answerable

### P4.4 - Compression and Forgetting (section 28)

- Unique episodes: retain in full
- Repeated episodes: abstract the pattern, keep first, last, and every
  failure verbatim
- Superseded episodes: demote, summary retained, detail archived
- Failures: NEVER compressed away
- Compression requires extraction first (learn from it before compressing)
- What's forgotten is recorded as forgotten (a known gap, not a silent hole)

### P4.5 - Retirement

- A skill is a candidate when a newer skill subsumes it
- Retirement is not deletion: deprecated, reconstructible, removed from
  active ranking
- Stops costing search time without losing knowledge

### P4 Exit Criteria

- Skills discovered from repetition and single successes
- Promotion gate rejects bad abstractions, promotes good ones
- Regression suite growing automatically
- Metric 1 (compounding): cost of Nth skill declining with N
- Metric 2 (transfer): learning in domain A improves unseen domain B
- Metric 11 (abstraction survival): promoted abstractions stay in use

---

## Phase 5: Curiosity + Structural Self-Modification

**Maps to**: Section 33, Stages 5-6
**Goal**: Self-directed learning. Structural change gated by a regression
suite now large enough to make degradation visible.

### P5.1 - Curiosity and Value Model (section 29)

Gap detection:
- Structural gaps (expected relation missing)
- Functional gaps (can do X but don't know why)
- Repeated impasses (stuck at same subgoal N times)
- Held contradictions (section 23)
- Failed predictions (world surprised the system)
- Distance from grounding (beliefs supported only by beliefs)

Value ranking:
- Expected blast radius (how much depends on this?)
- Relevance to goals
- Learning progress (rate of improvement, not difficulty)
- Cost to close

### P5.2 - Goal System (section 29)

- Task goals: supplied externally
- Standing goals: supplied externally, persistent
- Instrumental subgoals: derived, in service of above
- Learning goals: from gaps, ranked by value, traceable to standing goals
- Standing goals are immutable (normative mutability class)

### P5.3 - Capability Acquisition (section 32)

Give EKG a minimal native substrate from which it can acquire richer tools:

- Native primitives are fixed, typed, and policy-enforced: scoped network
  request, scoped file read/write, observation, and sandboxed execution.
- Every primitive invocation declares effects, resource limits, permission
  requirements, redaction rules, and replayability. There is no ambient file,
  network, process, environment, or secret access.
- Interface discovery may inspect user-authorized API descriptions, schemas,
  command help, fixtures, or observed request/response examples. Discovery is
  an episode with provenance; it does not itself grant permission to call the
  discovered interface.
- Synthesis produces neutral typed procedures, input/output schemas, contracts,
  dependency pins, effect summaries, permission requirements, and tests.
- Candidate capabilities run in a sandbox against mocks or explicitly granted
  test targets. Promotion uses the Phase 4 gate and requires local contract,
  permission, regression, and effect checks.
- Capability execution resolves permissions at invocation time. Grants are
  scoped, revocable local objects; procedures contain permission requirements,
  never credentials.
- Failures preserve traces and feed normal credit assignment, contract
  refinement, repair, and retirement.

### P5.4 - Reconstructible Capability Bundles (sections 32 and 34)

Define a versioned, canonical bundle format containing:

- Manifest, stable capability identity, procedures in neutral IR, dependency
  DAG and exact versions/content hashes
- Contracts, typed schemas, permission/effect declarations, resource bounds,
  tests and fixtures, reconstruction recipe, compatibility constraints
- Provenance for authorship/discovery, source-interface fingerprints, build
  steps, validation episodes, and exported evidence references
- No secret values, bearer tokens, cookies, raw environment variables,
  machine-local paths, ambient grants, or local trust receipts

Export is deterministic and content-addressed. Import verifies structure,
hashes, dependency closure, schema compatibility, prohibited effects, and
resource bounds before storing the entire bundle in quarantine. Imported
concepts and procedures are always Provisional, imported evidence is historical
provenance rather than local authorization, and imported permission
requirements remain unsatisfied until the local operator grants them. Local
tests and locally grounded observations must revalidate the capability before
the ordinary promotion gate may activate it. Failed imports and failed
revalidations remain inspectable and cannot partially mutate the active graph.

### P5.5 - Structural Self-Modification (section 33, stage 6)

Attempted last, deliberately. The biggest gains and only truly
catastrophic failures live here.

- Reorganize concepts and search policies
- Gated by regression suite now large enough to make degradation visible
- Two rates of change enforced (section 25):
  - Fast (during use): local, additive, safe mid-conversation
  - Slow (offline): global, restructuring, unsafe mid-thought

### P5 Exit Criteria

- EKG directs its own learning within goal boundaries
- EKG can discover an authorized interface and synthesize a typed, contracted,
  sandbox-tested capability from the native primitives
- Exported bundles reconstruct the same dependency DAG and tests on a clean
  instance; deterministic re-export has the same content identity
- Imported capabilities cannot execute before local permission resolution and
  revalidation, and cannot inherit trust or secrets from the exporter
- Malformed, incomplete, over-permissioned, secret-bearing, or dependency-
  conflicting bundles fail atomically and remain outside the active graph
- Structural changes pass regression suite
- Goals remain immutable
- Metric 3 (weaning): teacher calls declining per domain
- Metric 9 (teacher ablation): competence survives disconnection

---

## Phase 6: Inspector + Metrics Dashboard

**Maps to**: Section 38
**Goal**: Make the flywheel measurable and the system inspectable.

### P6.1 - Web Inspector (`@ekg/inspector`)

- Knowledge graph visualization (concepts, relationships, procedures)
- Episode browser (search, filter, replay)
- Procedure inspector (contract, test cases, version history)
- Contradiction viewer
- Dependency graph (what depends on what)
- Human-readable episode narrative (“What happened?” view):
  - the original request and the escalation path taken;
  - whether a teacher was used, which provider/model/source answered, and the
    teacher's structured proposal summarized in plain language;
  - validation status, rejected/provisional/verified checks, and the exact
    reason a proposal was accepted, retried, or discarded;
  - what EKG learned (new concept/procedure/contract/test), what it deliberately
    did not learn, and the provenance episode that supports each change;
  - execution steps, contract checks, observed result, evaluation tier,
    confidence/surprise, cost, and the reason for any abstention;
  - capability permissions/effects and local revalidation status when a
    capability participated.
- Narrative rendering is a read-only projection over immutable episode,
  teacher-interaction, trust, and provenance records. It must redact secrets,
  bearer tokens, cookies, raw environment values, and sensitive payloads, and
  must retain a raw-JSON drill-down for forensic inspection rather than
  replacing the underlying evidence.
- CLI parity: `ask --explain` (or an equivalent human-readable episode command)
  should print the same bounded narrative without requiring the web inspector.

### P6.2 - Section 38 Metrics Dashboard

The twelve metrics, tracked continuously:

1. **Compounding**: cost of Nth skill vs N (THE most important number)
2. **Transfer**: learning in domain A improves unseen domain B
3. **Per-domain weaning**: teacher calls on Nth novel task vs 1st
4. **Trace compression**: steps needed for repeated task family
5. **Rung distribution**: which escalation rung resolves problems
6. **No regression**: verified episodes still pass after new structure
7. **Attribution accuracy**: on injected faults, does credit find the culprit?
8. **Attribution cost**: as fraction of total cost, as traces lengthen
9. **Teacher ablation**: disconnect teacher, re-run task history
10. **Grounding drift**: fraction of beliefs traced to external evidence
11. **Abstraction survival**: promoted abstractions still in use later
12. **Calibration**: when EKG says .9, is it right ~90%?

Anti-gaming rules (section 38):
- Held-out task families for transfer measurement
- Novel tasks only (caching is not capability)
- Report abstentions separately
- Publish the failures

### P6 Exit Criteria

- All 12 metrics tracked and visualized
- Anti-gaming rules enforced
- The flywheel is either turning or visibly not turning
- Can make the honest call: "the thesis is alive" or "the thesis is dead"

---

## Open Problems (from Appendix D)

Ranked by likelihood of sinking the project:

1. Credit assignment cost at scale (P2.3)
2. Non-replayable steps (P2.3)
3. Joint responsibility / interaction effects (P2.3)
4. Gate calibration - too strict or too loose (P4.2)
5. Correction scope - how broadly to fix (P2.4)
6. Open-ended tasks resisting decomposition (P1.4)
7. Recall without scanning everything (P3.1)
8. Grounding ratio - how much self-supervision per grounded check (P3.4)
9. Contract acquisition for new procedures (P2.4)
10. Discriminating feature discovery for scope refinement (P2.6)
11. Safe interface discovery under adversarial schemas and responses (P5.3)
12. Portable reconstruction across runtime and environment drift (P5.4)

These are where the experiment produces information regardless of outcome.

---

## Suggested Build Order

```
P0.1 -> P0.2 -> P0.3 -> P0.4 -> P0.5 -> P0.6    (seed, ~4-6 weeks)
                                           |
P1.1 -> P1.2 -> P1.3 -> P1.4                      (interpretation, ~3-4 weeks)
                          |
P2.1 -> P2.2 -> P2.3 -> P2.4 -> P2.5 -> P2.6     (credit, ~4-6 weeks)
                                          |
P3.1 -> P3.2 -> P3.3 -> P3.4                      (intuition, ~4-6 weeks)
                          |
P4.1 -> P4.2 -> P4.3 -> P4.4 -> P4.5              (consolidation, ~3-4 weeks)
                                  |
P5.1 -> P5.2 -> P5.3 -> P5.4 -> P5.5               (curiosity + capability,
                                  |                  ~5-8 weeks)
P6.1 -> P6.2                                       (inspector, ~2-3 weeks)
```

P6 (inspector) can start alongside P1 and grow incrementally - you'll want
to see the graph and episodes early. The timeline above is sequential
solo-developer pace.

Total rough estimate: 6-9 months to a working system that can measure
whether the flywheel turns. This is aggressive but not insane.

## What Success Looks Like

Metric 1 (compounding) slopes downward. Each new skill makes the next
cheaper to acquire. If it's flat after Phase 4, the thesis is dead and
the honest thing to do is say so publicly.

The second-best outcome is that credit assignment works but costs too
much at scale - that's a tractable engineering problem, not a
fundamental one.

The worst outcome is not being wrong. It's being unable to tell.
