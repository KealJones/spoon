# Front Language Analysis - implementation progress

Spec: `docs/superpowers/specs/2026-08-28-front-language-analysis-design.md`
Branch: `claude/front-language-analysis-review-bd14d2`
Mode: commits + push to origin, no PRs. Real-model gate on `qwen3.8:27b`.

Rule: nothing here is marked DONE unless it compiles, is tested, and the test
actually ran. No skeletons, no `todo!()`, no deferred work.

## Slices

| # | Slice | State | Commit |
|---|---|---|---|
| 1 | Core IR: UtteranceAnalysis, Part, Mention, ResidualClaim, AlignedDocument, proposal + ground_for, structural validation | DONE | |
| 2 | ResponsePlan: per-claim `act`, `RenderVariant::Joined`, plan-act derivation, uncertainty merge | DONE | |
| 3 | LanguageContextPacket: types, bounds, truncation flags, SupplementalRequest | DONE | |
| 4 | Intent Catalog: tables, skeleton normalization, pattern lifecycle/promotion/demotion | TODO | |
| 5 | Engine per-part dispatch: toposort, PartOutcome, ResponsePlan build | TODO | |
| 6 | Suspend/resume: PendingPartsCycle, per-part Teacher, budget exhaustion | TODO | |
| 7 | Clarify across parts: reply-only analysis, rendered_in_turn | TODO | |
| 8 | Residual facts: provenance-gated ObservedFact admission | TODO | |
| 9 | Template realizer: template set, RealizationProposal, validation | DONE | |
| 10 | Graph writes: closed relationship-kind set, alias/termed/intent-of admission | TODO | |
| 11 | `@spoon/intent` schemas + prompt; SDK wire parity | TODO | |
| 12 | Test suite: behavioral, catalog, facts, realizer, adversarial | TODO | |
| 13 | Real-model gate: qwen3.8:27b, then qwen2.5:1.5b comparison | TODO | |
| 14 | Full gate: fmt, clippy, cargo test, tsc, pnpm test | TODO | |

## Decisions made during implementation

(append as they happen, with the reason)

## Known gaps / honest notes

(anything that did not land, and why)
