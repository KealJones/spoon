use ekg_core::{Evaluation, Value, VerifiabilityTier};
use ekg_engine::{
    CheckableSubgoal, ConsensusObservation, DecompositionError, TierThreeJudgment,
    VerificationMethod, decompose_goal, detect_surprise, evaluate_consensus,
    evaluate_deterministic, evaluate_inverse, evaluate_round_trip, evaluate_tier_three,
};

#[test]
fn deterministic_equality_is_a_hard_success_without_surprise() {
    let evaluation = evaluate_deterministic(&Value::Int(42), &Value::Int(42));

    assert_evaluation(evaluation, VerifiabilityTier::Hard, true, Some(0.0));
}

#[test]
fn deterministic_mismatch_is_a_hard_failure_with_surprise() {
    let evaluation = evaluate_deterministic(&Value::Int(42), &Value::Int(41));

    assert_evaluation(evaluation, VerifiabilityTier::Hard, false, Some(1.0));
}

#[test]
fn deterministic_comparison_is_type_strict() {
    let evaluation = evaluate_deterministic(&Value::Int(1), &Value::Float(1.0));

    assert!(!evaluation.success);
}

#[test]
fn distinct_independent_methods_can_establish_consensus() {
    let observations = vec![
        ConsensusObservation::new("direct", Value::Text("answer".into())),
        ConsensusObservation::new("independent-cross-check", Value::Text("answer".into())),
    ];

    let evaluation = evaluate_consensus(&observations);

    assert_evaluation(evaluation, VerifiabilityTier::Consensus, true, None);
}

#[test]
fn disagreement_fails_consensus() {
    let observations = vec![
        ConsensusObservation::new("method-a", Value::Int(4)),
        ConsensusObservation::new("method-b", Value::Int(5)),
    ];

    let evaluation = evaluate_consensus(&observations);

    assert_evaluation(evaluation, VerifiabilityTier::Consensus, false, None);
}

#[test]
fn duplicate_or_missing_methods_do_not_pretend_to_be_independent() {
    let observations = vec![
        ConsensusObservation::new("same-method", Value::Int(4)),
        ConsensusObservation::new("same-method", Value::Int(4)),
    ];

    let evaluation = evaluate_consensus(&observations);

    assert_eq!(evaluation.tier, VerifiabilityTier::Deferred);
    assert!(!evaluation.success);
    assert!(evaluation.details.contains("independent"));
}

#[test]
fn inverse_and_round_trip_helpers_check_recovery() {
    let original = Value::List(vec![Value::Int(1), Value::Int(2)]);

    let inverse = evaluate_inverse(&original, &original);
    let round_trip = evaluate_round_trip(&original, &Value::List(vec![Value::Int(2)]));

    assert_evaluation(inverse, VerifiabilityTier::Consensus, true, Some(0.0));
    assert_evaluation(round_trip, VerifiabilityTier::Consensus, false, Some(1.0));
}

#[test]
fn tier_three_distinguishes_pending_from_human_verdicts() {
    let pending = TierThreeJudgment::Deferred {
        reason: "wait for field observation".into(),
    };
    let accepted = TierThreeJudgment::Human {
        accepted: true,
        rationale: "meets the review rubric".into(),
    };

    assert!(evaluate_tier_three(&pending).is_none());

    let evaluation = evaluate_tier_three(&accepted).expect("resolved judgment");
    assert_evaluation(evaluation, VerifiabilityTier::Deferred, true, None);
}

#[test]
fn surprise_is_a_bounded_change_signal() {
    assert_eq!(detect_surprise(&Value::Bool(true), &Value::Bool(true)), 0.0);
    assert_eq!(
        detect_surprise(&Value::Bool(true), &Value::Bool(false)),
        1.0
    );
}

#[test]
fn decomposition_records_explicit_checkable_proposals_for_review() {
    let subgoals = vec![
        CheckableSubgoal::new(
            "calculate candidate total",
            VerificationMethod::ExpectedValue(Value::Int(12)),
        ),
        CheckableSubgoal::new(
            "review whether the tone is appropriate",
            VerificationMethod::HumanReview {
                criteria: "matches the supplied tone rubric".into(),
            },
        ),
    ];

    let decomposition = decompose_goal("produce a correct, useful answer", subgoals.clone())
        .expect("valid decomposition");

    assert_eq!(decomposition.goal, "produce a correct, useful answer");
    assert_eq!(decomposition.subgoals, subgoals);
    assert!(decomposition.requires_semantic_validation);
}

#[test]
fn decomposition_rejects_empty_or_uncheckable_input() {
    assert_eq!(
        decompose_goal("", vec![]),
        Err(DecompositionError::EmptyGoal)
    );
    assert_eq!(
        decompose_goal("goal", vec![]),
        Err(DecompositionError::NoSubgoals)
    );

    let result = decompose_goal(
        "goal",
        vec![CheckableSubgoal::new(
            "  ",
            VerificationMethod::DeterministicCheck {
                description: "run test".into(),
            },
        )],
    );
    assert_eq!(result, Err(DecompositionError::EmptySubgoal { index: 0 }));

    let result = decompose_goal(
        "goal",
        vec![CheckableSubgoal::new(
            "run a deterministic check",
            VerificationMethod::DeterministicCheck {
                description: " ".into(),
            },
        )],
    );
    assert_eq!(
        result,
        Err(DecompositionError::UncheckableSubgoal { index: 0 })
    );

    let result = decompose_goal(
        "goal",
        vec![CheckableSubgoal::new(
            "cross-check independently",
            VerificationMethod::IndependentMethods {
                methods: vec!["same".into(), "same".into()],
            },
        )],
    );
    assert_eq!(
        result,
        Err(DecompositionError::UncheckableSubgoal { index: 0 })
    );
}

fn assert_evaluation(
    evaluation: Evaluation,
    tier: VerifiabilityTier,
    success: bool,
    surprise: Option<f64>,
) {
    assert_eq!(evaluation.tier, tier);
    assert_eq!(evaluation.success, success);
    assert_eq!(evaluation.surprise, surprise);
    assert!(!evaluation.details.is_empty());
}
