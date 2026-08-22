# Tiered Evaluation Plan

## Test Scenarios

- Identical deterministic values pass at Hard tier with zero surprise.
- Different deterministic values fail at Hard tier with maximum surprise; numeric types remain strict.
- Two distinct methods that agree pass at Consensus tier; disagreement fails.
- Fewer than two independent method names cannot establish consensus and is represented as Deferred.
- An inverse that recovers the input passes at Consensus tier; a mismatch fails with surprise.
- A pending Tier 3 judgment produces no verdict; a resolved human judgment produces a Deferred-tier evaluation.
- Surprise is zero for equality and one for inequality.
- Decomposition preserves explicit proposed subgoals, marks semantic review as required, and rejects empty goals/subgoals.

## Implementation

- Add a public `evaluation` module and re-export its API from the crate root.
- Keep comparisons deterministic and side-effect free.
- Model independent observations with method identifiers.
- Model Tier 3 pending/resolved states separately from `Evaluation`.
- Require explicit subgoal proposals and verification methods in the decomposition helper.

