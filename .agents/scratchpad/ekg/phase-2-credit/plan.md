# Phase 2 implementation plan

## RED acceptance tests

1. Contract attribution walks a failed trace in O(trace length), distinguishes
   violated requires/promises/fails-when, and points to the exact versioned
   procedure/step with high confidence and no replay.
2. Statistical attribution computes exposure/failure counts per element,
   reports uncertainty/support, ranks but does not conclude, and discounts
   perfectly co-occurring elements rather than pretending independence.
3. Counterfactual replay changes exactly one top-K suspect, stops at replay and
   step budgets, distinguishes decisive deterministic evidence from simulated
   evidence, and reports attribution cost/total-cost ratio.
4. Assumption-caused failures produce record/fix-assumption decisions and do
   not modify procedures.
5. One strong scoped counterexample may narrow a contract; procedure
   replacement requires several verified episodes and a challenger that beats
   the incumbent regression suite; concept revision stays proposed/offline.
6. Reconciliation traverses dependency edges, retains immutable history,
   preserves dependents with alternative support, and marks only affected
   current knowledge stale/under-review/invalid as justified.
7. Contradictory scoped claims split on a demonstrated discriminating feature;
   unresolved contradictions persist explicitly and propagate uncertainty.
8. Injected-fault kitchen test attributes flat pancakes to the missing
   leavening condition, narrows the rule's scope, preserves prior history, and
   passes the corrected case.

## Sequence

1. Extend core attribution, correction, contradiction, and reconciliation data
   types with serde roundtrip/invariant tests.
2. Build `ekg-credit` contract scanner and statistical ranker.
3. Add replay candidate/substitution abstraction and bounded replay engine.
4. Build `ekg-adapt` evidence-gated correction policy.
5. Add dependency reconciliation and contradiction store/refinement.
6. Wire engine/server/SDK/CLI surfaces and the pancake kitchen scenario.
7. Run full gate, independent injected-fault audit, fix, commit.

