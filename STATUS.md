# Spoon v2: status log

Newest entry first. Each entry: DONE / IN PROGRESS / NEXT.
"Done" means it runs, not that a type exists.
Decisions in PIVOT_PLAN.md; rules in AGENTS.md; design in docs/CONCEPT-IR-DESIGN.md.

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
