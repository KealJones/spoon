use std::collections::HashSet;

use ekg_core::{Evaluation, Value, VerifiabilityTier};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The output of one independently identified verification method.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsensusObservation {
    pub method: String,
    pub result: Value,
}

impl ConsensusObservation {
    pub fn new(method: impl Into<String>, result: Value) -> Self {
        Self {
            method: method.into(),
            result,
        }
    }
}

/// A Tier 3 signal is either still pending or explicitly resolved by a human.
/// Pending work is not represented as an unsuccessful `Evaluation` because no
/// verdict has been made yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TierThreeJudgment {
    Deferred { reason: String },
    Human { accepted: bool, rationale: String },
}

/// A concrete way a proposed subgoal can eventually be checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VerificationMethod {
    ExpectedValue(Value),
    DeterministicCheck { description: String },
    IndependentMethods { methods: Vec<String> },
    HumanReview { criteria: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckableSubgoal {
    pub description: String,
    pub verification: VerificationMethod,
}

impl CheckableSubgoal {
    pub fn new(description: impl Into<String>, verification: VerificationMethod) -> Self {
        Self {
            description: description.into(),
            verification,
        }
    }
}

/// A caller-proposed decomposition. It always requires semantic validation:
/// this helper validates structure and records a proposal, but does not infer
/// what a weak goal means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalDecomposition {
    pub goal: String,
    pub subgoals: Vec<CheckableSubgoal>,
    pub requires_semantic_validation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecompositionError {
    #[error("goal cannot be empty")]
    EmptyGoal,
    #[error("at least one proposed subgoal is required")]
    NoSubgoals,
    #[error("subgoal {index} cannot be empty")]
    EmptySubgoal { index: usize },
    #[error("subgoal {index} does not define a usable verification method")]
    UncheckableSubgoal { index: usize },
}

/// Compare an expected value with an observed value using strict `Value`
/// equality. Numeric types are intentionally not coerced.
pub fn evaluate_deterministic(predicted: &Value, observed: &Value) -> Evaluation {
    let surprise = detect_surprise(predicted, observed);
    let success = surprise == 0.0;

    Evaluation {
        tier: VerifiabilityTier::Hard,
        success,
        details: comparison_details("deterministic comparison", predicted, observed, success),
        surprise: Some(surprise),
    }
}

/// Evaluate whether at least two explicitly distinct methods reached the same
/// result. Method identifiers make the independence claim inspectable.
pub fn evaluate_consensus(observations: &[ConsensusObservation]) -> Evaluation {
    let independent_methods: HashSet<&str> = observations
        .iter()
        .map(|observation| observation.method.trim())
        .filter(|method| !method.is_empty())
        .collect();

    if observations.len() < 2 || independent_methods.len() != observations.len() {
        return Evaluation {
            tier: VerifiabilityTier::Deferred,
            success: false,
            details: "consensus requires at least two distinct, named independent methods".into(),
            surprise: None,
        };
    }

    let success = observations
        .first()
        .is_some_and(|first| observations.iter().all(|item| item.result == first.result));

    Evaluation {
        tier: VerifiabilityTier::Consensus,
        success,
        details: if success {
            format!("{} independent methods agreed", observations.len())
        } else {
            format!("{} independent methods disagreed", observations.len())
        },
        surprise: None,
    }
}

/// Check that applying an inverse recovered the original value.
pub fn evaluate_inverse(original: &Value, recovered: &Value) -> Evaluation {
    evaluate_recovery("inverse", original, recovered)
}

/// Check the final recovery step of a forward/inverse round trip.
pub fn evaluate_round_trip(original: &Value, recovered: &Value) -> Evaluation {
    evaluate_recovery("round trip", original, recovered)
}

/// Turn a resolved human judgment into a weak Tier 3 evaluation. A deferred
/// judgment returns `None`, preserving the difference between pending and fail.
pub fn evaluate_tier_three(judgment: &TierThreeJudgment) -> Option<Evaluation> {
    match judgment {
        TierThreeJudgment::Deferred { .. } => None,
        TierThreeJudgment::Human {
            accepted,
            rationale,
        } => Some(Evaluation {
            tier: VerifiabilityTier::Deferred,
            success: *accepted,
            details: rationale.clone(),
            surprise: None,
        }),
    }
}

/// Binary surprise for structured deterministic values: unchanged is 0, any
/// observable difference is 1. The result is always bounded to `[0, 1]`.
pub fn detect_surprise(predicted: &Value, observed: &Value) -> f64 {
    if predicted == observed { 0.0 } else { 1.0 }
}

/// Validate and record an explicit decomposition supplied by a caller or
/// teacher. No semantic subgoals are invented by this function.
pub fn decompose_goal(
    goal: impl Into<String>,
    proposed_subgoals: Vec<CheckableSubgoal>,
) -> Result<GoalDecomposition, DecompositionError> {
    let goal = goal.into();
    if goal.trim().is_empty() {
        return Err(DecompositionError::EmptyGoal);
    }
    if proposed_subgoals.is_empty() {
        return Err(DecompositionError::NoSubgoals);
    }
    if let Some(index) = proposed_subgoals
        .iter()
        .position(|subgoal| subgoal.description.trim().is_empty())
    {
        return Err(DecompositionError::EmptySubgoal { index });
    }
    if let Some(index) = proposed_subgoals
        .iter()
        .position(|subgoal| !is_checkable(&subgoal.verification))
    {
        return Err(DecompositionError::UncheckableSubgoal { index });
    }

    Ok(GoalDecomposition {
        goal,
        subgoals: proposed_subgoals,
        requires_semantic_validation: true,
    })
}

fn is_checkable(method: &VerificationMethod) -> bool {
    match method {
        VerificationMethod::ExpectedValue(_) => true,
        VerificationMethod::DeterministicCheck { description } => !description.trim().is_empty(),
        VerificationMethod::IndependentMethods { methods } => {
            let distinct: HashSet<&str> = methods
                .iter()
                .map(|method| method.trim())
                .filter(|method| !method.is_empty())
                .collect();
            methods.len() >= 2 && distinct.len() == methods.len()
        }
        VerificationMethod::HumanReview { criteria } => !criteria.trim().is_empty(),
    }
}

fn evaluate_recovery(label: &str, original: &Value, recovered: &Value) -> Evaluation {
    let surprise = detect_surprise(original, recovered);
    let success = surprise == 0.0;

    Evaluation {
        tier: VerifiabilityTier::Consensus,
        success,
        details: comparison_details(label, original, recovered, success),
        surprise: Some(surprise),
    }
}

fn comparison_details(label: &str, expected: &Value, observed: &Value, success: bool) -> String {
    if success {
        format!("{label} matched: {observed}")
    } else {
        format!("{label} differed: expected {expected}, observed {observed}")
    }
}
