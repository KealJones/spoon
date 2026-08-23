# Spoon seed curricula

These are the first designed seed curricula for the seed-forge described in
`IMPLEMENTATION-PLAN.md` (P0F.8). They are inspectable manifests, not installed
knowledge and not an executable forge. Every artifact is explicitly
`Declared/design-only` until a curriculum runner, independent reconstruction,
and Teacher-OFF evidence exist.

Files:

- [`curriculum.schema.json`](curriculum.schema.json) — strict, versioned JSON
  Schema for curriculum manifests.
- [`language-kernel-intent.json`](language-kernel-intent.json) — grapheme and
  span semantics, letter-count ambiguity, paraphrase families, clarification,
  and response plans.
- [`structured-data-transforms.json`](structured-data-transforms.json) — JSON
  parsing, strict/optional paths, null-versus-missing semantics, and collection
  transforms.
- [`programming-foundations.json`](programming-foundations.json) — grounded
  repository explanation and a scoped inspect–hypothesize–patch–test workflow.

Each manifest separates demonstrations, counterexamples, exercises, and held-out
generalization. It also declares semantic learned-structure expectations (no
engine IDs), native operations and locally granted capabilities, explicit
Teacher-OFF gates, reconstructible-only export filters, and clean-import
validation. Activities describe input shapes and variation families rather than
shipping canned utterances or opaque answer dumps.

The lesson metadata follows the current preferred Teacher contract:
`pure_expr_v2` drafts
may propose concepts, relationships, procedures, contracts, and invocation
inputs; the engine owns IDs, lifecycle, versions, timestamps, confidence,
mutability, and test cases. Legacy `pure_rpn_v1` remains readable but cannot
express these curricula's structured operations. Operations named by these
designs remain declared curriculum requirements until the runner verifies their
current runtime support rather than trusting the manifest.

Validation is intentionally read-only. Once a JSON Schema validator is available
locally, validate all three manifests against `curriculum.schema.json`; until
then, JSON parsing plus the repository's existing schema-validation code may be
used for a focused check. No command in this directory runs a curriculum or
promotes imported knowledge.
