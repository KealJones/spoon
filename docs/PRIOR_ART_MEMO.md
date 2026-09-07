# Spoon - Prior Art & Algorithm Reference

Algorithms and systems relevant to the v2 unified-concept architecture.
See `CONCEPT-IR-DESIGN.md` for the core design rationale.

---

## 1. Term Rewriting Systems

The v2 evaluator is fundamentally a term rewriting system. Key references:

- **Maude** (Clavel et al.): rewriting logic, conditional rewrite rules,
  narrowing-based search. Closest formal model to what Spoon's evaluator does.
- **MeTTa / Hyperon** (OpenCog): atoms, grounded atoms with executable behavior,
  self-modifying atom programs. Most architecturally similar existing system.
  Their grounded atoms map to our Native realizations; MeTTa programs stored in
  the AtomSpace map to our Composed realizations. Spoon drops the "atom"
  vocabulary: what MeTTa calls an atom, we call a concept, because in our model
  compounds are full citizens rather than a separate structural layer.
- **Lambda calculus + graph reduction**: the Compound { head, args } -> evaluate
  -> rewrite cycle is essentially graph reduction with named concepts instead of
  lambdas.

## 2. Pattern Matching & Unification

Needed for inference rules (Stage 3 of PIVOT_PLAN.md).

- **First-order unification** (Robinson 1965): the foundation. Our concepts are
  first-order terms; a Hole unifies with anything.
- **E-unification**: unification modulo equational theories. Relevant when
  concepts have algebraic properties (commutativity of Add, etc.)
- **Discrimination trees** (McCune): fast index for finding which stored rules
  match a given concept. Important for performance when the rule set grows.

Practical pick: standard first-order unification with occurs check, indexed
by discrimination tree over rule trigger patterns.

## 3. ACT-R Activation

Used for realization selection, vocabulary ranking, and memory retrieval.

```
B_i = ln(sum over j: (t_now - t_j)^(-d))
```

Where t_j are past access timestamps, d = 0.5 (decay parameter).

- Anderson et al. 2004: https://doi.org/10.1007/978-3-540-24622-4_3
- Combines recency and frequency into one score
- Spoon adds success_rate as a multiplicative factor for realization selection

## 4. Program Synthesis (for learning Composed realizations)

The v1 synthesizer's core approach carries forward - typed enumerative search
with observational equivalence (OE) pruning is still how we learn Composed
realizations from I/O examples.

**Core algorithm (from v1, still valid):**

- BFS by cost over a typed grammar of primitives
- OE pruning: hash outputs on all examples, skip programs with identical
  output signatures to already-seen programs
- Constants from utterance context + small pool (0, 1, "", [], true, false)
- Budget: max_nodes + max_ms + max_memory

**References:**
- BUSTLE: https://arxiv.org/abs/2007.14381
- DreamCoder: https://arxiv.org/abs/2006.08381
- Escher: https://dl.acm.org/doi/10.1145/2737924.2737977
- FlashMeta/FlashFill (Gulwani)

**Change from v1:** Primitives are now concepts with Native realizations instead
of kernel functions. The synthesizer searches over concepts, not IR expressions.
Output is a Composed realization (a Concept tree), not an IR Program.

## 5. Library Consolidation (Stitch-style)

When repeated compositions appear, extract reusable abstractions.

- **Stitch**: https://arxiv.org/abs/2211.16605
- Anti-unification (Plotkin 1970): find most-specific generalization of two terms
- Utility = cost_before - cost_after - cost_of_definition
- Evict single-use abstractions after EVICTION_AGE

Still applicable. The terms being anti-unified are now Concepts instead of
IR Programs, but the algorithm is identical.

## 6. Cognitive Architecture Precedents

| System | Relevance to v2 |
|---|---|
| **Soar** | Episodic/semantic/procedural memory split. Chunking = our consolidation. Validates the "learn from successful reasoning traces" approach. |
| **NARS** | Reasoning under insufficient knowledge. Non-axiomatic logic with experience-based truth values. Closest to our "confidence is multidimensional" stance. |
| **OpenCog/Hyperon** | AtomSpace as unified substrate. MeTTa's grounded atoms. ECAN attention allocation. Most architecturally similar prior system. Key differences in CONCEPT-IR-DESIGN.md. |
| **DreamCoder** | Learned program library with sleep-phase abstraction. Validates the "repeated patterns -> reusable building blocks" principle. |
| **Cyc** | Warning: hand-curated ontology does not scale. Lesson: keep the privileged vocabulary minimal. |
| **Semantic Machines** (Andreas et al. 2020) | Dataflow graphs with revision. Their correction operators map to our Correction<...> concepts in the ears output. https://arxiv.org/abs/2009.11423 |

## 7. Lifelong Skill Systems

Validate the realization evolution approach:

- **Voyager** (Wang et al.): growing executable code library in Minecraft
- **SkillWeaver**: learns reusable APIs through autonomous exploration
- **SkillSmith**: co-evolves skills and tools, including modification and retirement
- **MUSE-Autoskill**: skill lifecycle management (create, reuse, evaluate, refine)

These are narrower than Spoon but prove that executable knowledge can persist,
improve, compose, and evolve through experience.

## 8. Structured Knowledge at Scale

- **NELL**: hundreds of millions of web pages, tens of millions of beliefs
- **Knowledge Vault** (Google): web-scale probabilistic knowledge base
- **Wikidata**: structured facts, useful as an external knowledge source

Lesson: enormous structured stores of facts alone are not sufficient for broad
intelligence. Spoon needs interpretation, episodic memory, procedural learning,
and self-modification on top of any knowledge store.

## 9. Value Spotting (carried from v1)

Still useful for the ears: extracting typed values from messy text before
concept construction.

- Regex pipeline: quoted strings > URLs > paths > emails > JSON > arithmetic > dates > numbers
- `chrono` + `chrono-english` for dates, `evalexpr` for inline arithmetic
- Overlap resolution: prefer longer span, higher confidence

## 10. Typo/Slang Normalization (carried from v1)

Pre-processing before the LLM ears, if used:

- SymSpell (Wolf Garbe): O(1) lookup via delete-neighborhood. Good for
  correcting against the concept surface-form lexicon.
- Slang table: ~200 entries covers most SMS-style input
- Guard: skip tokens < 3 chars, skip capitalized words in entity registry

## 11. RDF / RDF-star

Demonstrates first-class statements and provenance about statements.
Relevant to how we might represent `Stated<Keal, IsSad<Greg>>` and
attach provenance/confidence to any concept assertion.

## 12. Conceptual Graphs (Sowa)

Compositional concepts and n-ary relations with formal logical semantics.
Historical precedent for treating concepts and conceptual relations as
a unified logical structure. Spoon differs by making the relational
vocabulary itself plastic and learnable.
