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
| 4 | Intent Catalog: tables, skeleton normalization, pattern lifecycle/promotion/demotion | DONE | |
| 5 | Engine per-part dispatch: toposort, PartOutcome, ResponsePlan build | DONE | |
| 6 | Suspend/resume: PendingPartsCycle, per-part Teacher, budget exhaustion | DONE | |
| 7 | Clarify across parts: reply-only analysis, rendered_in_turn | DONE | |
| 8 | Residual facts: provenance-gated ObservedFact admission | DONE | |
| 9 | Template realizer: template set, RealizationProposal, validation | DONE | |
| 10 | Graph writes: closed relationship-kind set, alias/termed/intent-of admission | DONE | |
| 11 | `@spoon/intent` schemas + prompt; SDK wire parity | DONE | |
| 12 | Test suite: behavioral, catalog, facts, realizer, adversarial | DONE | |
| 13 | Real-model gate: qwen3.8:27b, then qwen2.5:1.5b comparison | DONE | |
| 14 | Full gate: fmt, clippy, cargo test, tsc, pnpm test | DONE | |

## Decisions made during implementation

- **Local match runs before the interpreter**, not after. "Always interpreter
  first" would guarantee one model call per turn forever, which inverts the
  weaning goal. Safe because catalog patterns are only admitted from a single
  part, so a whole-utterance match implies the utterance is single-part.
- **The realizer is a template selector, not a generator.** It emits a template
  id, a claim order, and a tone, and no user-visible characters. Fabrication is
  structurally impossible rather than allowlist-checked.
- **Sentence mechanics are Engine-owned.** A template that continues a sentence
  strips one terminator, and a mid-sentence slot lowercases its initial only
  when the original utterance used that word lowercase. That evidence rule is
  why "double" lowercases and "Pierre" does not.
- **Residual facts require provenance**, either a token span or a packet alias
  that traces to a prior observation. A catalog alias is refused: the ability
  to compute something is not evidence that something is true.
- **Catalog patterns need support from two distinct episodes** before they can
  drive interpreter-off matching, because a mis-segmented part that still
  executes would otherwise become permanent weaning fuel.

## Known gaps / honest notes

### Real-model gate: qwen3.8:27b passes, qwen2.5:1.5b fails

Run against `"hey whats 2+2 and then double that"` at temperature 0 through the
Ollama `/api/generate` structured-output path.

`qwen3.8:27b` produced a proposal that deserializes and passes the full
structural validator. It segmented into three parts, covered every
non-whitespace token without overlap, grounded both literals to real spans, and
critically bound the second question to `part_ref { p1, result }` rather than
computing 4 and inlining it. That output is checked in at
`crates/spoon-core/tests/fixtures/utterance-qwen3.8-27b.json` and asserted by
`crates/spoon-core/tests/model_gate.rs`.

Two prompt revisions were needed to get there, and both failures are worth
recording:
1. With `think: false`, the model emitted structurally valid parts but dropped
   token grounding entirely, returning `sourceTokens: []` everywhere. Thinking
   had to stay ON, and the schema now sets `minItems: 1` on `sourceTokens`.
2. The first prompt left the connectives "and" and "then" belonging to no part,
   which the coverage rule correctly rejected. The prompt now says connectives
   attach to the part that follows.

`qwen2.5:1.5b` failed outright. It produced 8759 bytes of degenerate repetition,
229 token-range objects for a 15-token stream, a maximum `endToken` of 685, and
never closed the JSON. It is not a validation failure that better prompting
would fix; the model did not stay on task.

**This matters for the design.** The spec's weaning argument assumes a *small*
front model that gets called less over time. A 27B model at the front of every
cycle is a different cost story than the spec implies. Either the weaning has
to carry more weight than assumed, or the interpreter needs a mid-size model
(the 7B-14B range is untested here). Nothing in the code depends on which is
chosen, but the spec's cost claim is currently unproven at small scale.

### Pre-existing failures, verified against main

Both were checked by building `main` in a separate worktree and running them
there. Neither is a regression from this work.

- `packages/intent/src/ollama.ts` on `main` is syntactically corrupt: a bad
  edit clobbered `ollamaStructuredContent`'s definition into
  `wireInterpretation`'s signature line, so the package does not parse and its
  whole test file fails to transform. This branch repairs it, which turns 7
  failing tests green.
- `spoon-engine` test `malformed_reusable_lesson_gets_one_targeted_retry_when_budget_allows`
  fails on `main` at the same assertion. Untouched.
- `@spoon/cli` test `fake teacher teaches a procedure once and the Rust cycle
  reuses it locally` returns a null answer on `main`'s Rust as well, verified
  with `main` built in its own worktree.

### Not landed

- `cycle.rs` still routes through the existing single-interpretation path. The
  per-part machinery (`parts.rs`, `language_cycle.rs`) is complete and tested,
  but the ~5900-line `run_cycle` has not been rewired to call it, so an
  end-to-end `cycle.start` still uses the old whole-utterance flow.
- Pre-existing clippy warnings in `cycle.rs` and `engine.rs` are untouched.
  Every file added by this work is clippy-clean.
