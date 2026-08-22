use std::collections::BTreeMap;

use ekg_adapt::ContradictionStatus;
use ekg_core::{
    BinOp, Concept, Condition, Evaluation, Expr, Lifecycle, MutabilityClass, Param, Procedure,
    Value, VerifiabilityTier,
};
use ekg_credit::{
    AttributionConfidence, AttributionEvidence, CounterfactualCandidate, CounterfactualChange,
    CounterfactualMode, Suspect,
};
use ekg_engine::{
    AdaptationEvidenceRef, AdaptationPlanRequest, AdaptationTarget, ApplyAdaptationRequest,
    AttributionSelector, CounterfactualMutation, Engine, EngineError, FailureAnalysisBudget,
    FailureAnalysisRequest, FailureEvidenceSource, MutationScope, ProcedureVersionRef,
    ReplayVerification,
};
use ekg_episode::{EpisodeFeedback, FeedbackSource};
use ekg_exec::{ConditionCheckStatus, ExecStepStatus, ExecTrace};

fn factor_inputs(factor: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([("factor".into(), Value::Int(factor))])
}

fn replace_body_candidate(
    procedure: &Procedure,
    trace_step: usize,
    prior_score: f64,
    description: &str,
    body: Expr,
    verification: ReplayVerification,
    mode: CounterfactualMode,
) -> CounterfactualCandidate {
    let mutation = CounterfactualMutation::ReplaceBody {
        target: ProcedureVersionRef {
            id: procedure.id,
            version: procedure.version,
        },
        body,
        verification,
    };
    CounterfactualCandidate {
        suspect: Suspect {
            procedure: procedure.id,
            version: procedure.version,
            trace_step,
        },
        prior_score,
        change: CounterfactualChange {
            description: description.into(),
            replacement: serde_json::to_value(mutation).unwrap(),
        },
        mode,
    }
}

#[test]
fn phase2_pancake_replay_confirms_body_fault_while_contract_evidence_authorizes_narrowing() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();

    let substitution = Procedure::new(
        "SUBSTITUTE_BUTTERMILK",
        vec![Param::named("factor")],
        Expr::Var("factor".into()),
    );
    let mut leavening = Procedure::new(
        "SCALE_LEAVENING_LINEARLY",
        vec![Param::named("factor")],
        Expr::BinOp {
            op: BinOp::Mul,
            left: Box::new(Expr::Var("factor".into())),
            right: Box::new(Expr::Literal(Value::Float(5.0))),
        },
    );
    let injected_fault = "scaled leavening must stay below nine grams";
    leavening
        .contract
        .promises
        .push(
            Condition::described(injected_fault).with_check(Expr::BinOp {
                op: BinOp::Lt,
                left: Box::new(Expr::Var("result".into())),
                right: Box::new(Expr::Literal(Value::Float(9.0))),
            }),
        );
    let scale_recipe = Procedure::new(
        "SCALE_RECIPE",
        vec![Param::named("factor")],
        Expr::Block(vec![
            Expr::Call {
                procedure: substitution.id,
                args: vec![Expr::Var("factor".into())],
            },
            Expr::Call {
                procedure: leavening.id,
                args: vec![Expr::Var("factor".into())],
            },
        ]),
    );
    let make_two_batches = Procedure::new(
        "MAKE_TWO_BATCHES",
        vec![Param::named("factor")],
        Expr::Literal(Value::Float(8.5)),
    );
    for procedure in [&substitution, &leavening, &scale_recipe, &make_two_batches] {
        engine.admin_insert_procedure(procedure).unwrap();
    }

    let verified_v1 = engine
        .execute_procedure(scale_recipe.id, factor_inputs(1), Some(Value::Float(5.0)))
        .unwrap()
        .episode;
    let failure_id = match engine.execute_procedure(
        scale_recipe.id,
        factor_inputs(2),
        Some(Value::Float(8.5)),
    ) {
        Err(EngineError::ExecutionFailed { episode_id, .. }) => episode_id,
        other => panic!("injected leavening fault must fail: {other:?}"),
    };
    let failed_episode = engine.episodes().get(failure_id).unwrap();
    let trace: ExecTrace =
        serde_json::from_value(failed_episode.execution_trace.clone().unwrap()).unwrap();
    assert_eq!(trace.steps.len(), 3);
    assert_eq!(
        trace
            .steps
            .iter()
            .map(|step| (step.procedure_called.unwrap(), step.procedure_version))
            .collect::<Vec<_>>(),
        vec![
            (substitution.id, Some(1)),
            (leavening.id, Some(1)),
            (scale_recipe.id, Some(1)),
        ]
    );
    assert_eq!(trace.steps[0].status, ExecStepStatus::Succeeded);
    assert!(matches!(
        trace.steps[1].status,
        ExecStepStatus::Failed { .. }
    ));
    assert!(matches!(
        trace.steps[2].status,
        ExecStepStatus::Failed { .. }
    ));
    assert_eq!(
        trace.steps[1].contract_checks.promises[0].status,
        ConditionCheckStatus::Violated
    );
    assert_eq!(trace.steps[1].output, Value::Float(10.0));

    let immutable_episode_bytes = serde_json::to_vec(&failed_episode).unwrap();
    let feedback = engine
        .admin_append_feedback(&EpisodeFeedback::new(
            failure_id,
            Value::Text("flat pancakes".into()),
            Evaluation {
                tier: VerifiabilityTier::Deferred,
                success: false,
                details: "pancakes did not rise after cooking".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("human", Some("kitchen-tester".into())),
            "phase2-flat-pancake-feedback",
        ))
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&engine.episodes().get(failure_id).unwrap()).unwrap(),
        immutable_episode_bytes
    );
    assert_eq!(
        engine.episodes().list_feedback(failure_id).unwrap(),
        vec![feedback.clone()]
    );

    // The injected hidden kitchen oracle says 2x requires 8.5g, not the
    // linear v1 rule's 10g. This candidate is the known ground-truth fault.
    let correct_candidate = replace_body_candidate(
        &leavening,
        1,
        0.9,
        "hidden oracle: scale leavening to 8.5g",
        Expr::BinOp {
            op: BinOp::Mul,
            left: Box::new(Expr::Var("factor".into())),
            right: Box::new(Expr::Literal(Value::Float(4.25))),
        },
        ReplayVerification::DeterministicExpected {
            expected: Value::Float(8.5),
        },
        CounterfactualMode::Deterministic,
    );
    let noncausal_candidate = replace_body_candidate(
        &substitution,
        0,
        0.8,
        "change the buttermilk substitution",
        Expr::Literal(Value::Int(999)),
        ReplayVerification::DeterministicExpected {
            expected: Value::Float(8.5),
        },
        CounterfactualMode::Deterministic,
    );
    let planning_candidate = replace_body_candidate(
        &scale_recipe,
        2,
        0.7,
        "interpret SCALE_RECIPE as MAKE_TWO_BATCHES",
        make_two_batches.body.clone(),
        ReplayVerification::SimulatedExpected {
            expected: Value::Float(8.5),
            model_id: "kitchen-rise-model".into(),
            model_version: "1".into(),
            assumptions: vec!["two independent batches preserve rise".into()],
        },
        CounterfactualMode::Simulated,
    );
    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failure_id,
            selected_feedback_id: Some(feedback.id),
            candidates: vec![planning_candidate, noncausal_candidate, correct_candidate],
            budget: FailureAnalysisBudget {
                top_k: 3,
                max_replays: 3,
                max_replay_steps: 200,
            },
        })
        .unwrap();

    assert_eq!(
        analysis.failure_evidence.source,
        FailureEvidenceSource::LateFeedback {
            feedback_id: feedback.id
        }
    );
    let decisive = analysis
        .counterfactual
        .attributions
        .iter()
        .filter(|attribution| attribution.decisive)
        .collect::<Vec<_>>();
    assert_eq!(decisive.len(), 1);
    assert_eq!(decisive[0].suspect.procedure, leavening.id);
    assert_eq!(decisive[0].suspect.trace_step, 1);
    assert_eq!(decisive[0].confidence, AttributionConfidence::Certain);
    let noncausal = analysis
        .counterfactual
        .attributions
        .iter()
        .find(|attribution| attribution.suspect.procedure == substitution.id)
        .unwrap();
    assert_eq!(noncausal.confidence, AttributionConfidence::Inconclusive);
    assert!(!noncausal.decisive);
    let planning = analysis
        .counterfactual
        .attributions
        .iter()
        .find(|attribution| attribution.suspect.procedure == scale_recipe.id)
        .unwrap();
    assert_eq!(planning.confidence, AttributionConfidence::Inconclusive);
    assert!(!planning.decisive);
    assert!(matches!(
        planning.evidence[0],
        AttributionEvidence::Replay {
            mode: CounterfactualMode::Simulated,
            counterfactual_succeeded: None,
            ..
        }
    ));

    // Metric 7: the known injected culprit is top-1, rank 1, MRR 1.0.
    let ground_truth_rank = analysis
        .ranked
        .iter()
        .position(|attribution| {
            attribution.suspect.procedure == leavening.id && attribution.decisive
        })
        .map(|index| index + 1)
        .unwrap();
    let top_1_accuracy = usize::from(ground_truth_rank == 1);
    let reciprocal_rank = 1.0 / ground_truth_rank as f64;
    assert_eq!(top_1_accuracy, 1);
    assert_eq!(ground_truth_rank, 1);
    assert_eq!(reciprocal_rank, 1.0);
    assert_eq!(analysis.counterfactual.replays_run, 3);
    assert!(analysis.cost.replay_steps <= 200);
    assert_eq!(
        analysis.cost.total_cost,
        analysis.cost.original_execution_cost + analysis.cost.attribution_cost
    );
    assert!(analysis.cost.attribution_cost_ratio.is_finite());
    assert!(analysis.cost.attribution_cost_ratio > 0.0);
    let measured_metric_8 = analysis.cost.attribution_cost / analysis.cost.total_cost;
    assert!((analysis.cost.attribution_cost_ratio - measured_metric_8).abs() < f64::EPSILON);
    assert!(analysis.cost.attribution_cost_ratio < 0.9);

    let contract_attribution = analysis
        .contract
        .attributions
        .iter()
        .find(|attribution| attribution.suspect.procedure == leavening.id)
        .unwrap();
    let scope_condition = Condition::described("linear leavening applies only below 2x")
        .with_check(Expr::BinOp {
            op: BinOp::Lt,
            left: Box::new(Expr::Var("factor".into())),
            right: Box::new(Expr::Literal(Value::Int(2))),
        });
    let plan = engine
        .plan_adaptation(AdaptationPlanRequest {
            idempotency_key: "phase2-pancake-scope-plan".into(),
            analysis: FailureAnalysisRequest {
                episode_id: failure_id,
                selected_feedback_id: Some(feedback.id),
                candidates: Vec::new(),
                budget: FailureAnalysisBudget::default(),
            },
            attribution: AttributionSelector {
                suspect: contract_attribution.suspect,
                mechanism: contract_attribution.mechanism,
            },
            evidence: vec![
                AdaptationEvidenceRef {
                    episode_id: failure_id,
                    selected_feedback_id: Some(feedback.id),
                },
                AdaptationEvidenceRef {
                    episode_id: failure_id,
                    selected_feedback_id: None,
                },
                AdaptationEvidenceRef {
                    episode_id: verified_v1.id,
                    selected_feedback_id: None,
                },
            ],
            target: AdaptationTarget::ProcedureScope {
                procedure_id: leavening.id,
                expected_version: 1,
                condition: scope_condition,
                learned_from: failure_id,
            },
            created_at: 999,
        })
        .unwrap();
    assert_eq!(plan.mutation_scope, MutationScope::OnlineNarrow);
    let receipt = engine
        .apply_adaptation(ApplyAdaptationRequest {
            plan_id: plan.id,
            idempotency_key: "phase2-pancake-scope-apply".into(),
            applied_at: 1_000,
        })
        .unwrap();
    assert!(matches!(
        receipt.outcome,
        ekg_engine::AdaptationOutcome::ProcedureUpdated {
            procedure_id,
            previous_version: 1,
            current_version: 2,
        } if procedure_id == leavening.id
    ));
    let current = engine.graph().get_procedure(leavening.id).unwrap().unwrap();
    assert_eq!(current.version, 2);
    assert_eq!(current.lifecycle, Lifecycle::Active);
    assert!(
        current
            .contract
            .confidence
            .scope
            .iter()
            .any(|scope| scope.learned_from == Some(failure_id))
    );
    assert!(
        current
            .contract
            .requires
            .iter()
            .any(|condition| condition.description == "linear leavening applies only below 2x")
    );
    let narrowed_failure =
        match engine.execute_procedure(leavening.id, factor_inputs(2), Some(Value::Float(8.5))) {
            Err(EngineError::ExecutionFailed { episode_id, .. }) => episode_id,
            other => panic!("the learned v2 scope must exclude 2x: {other:?}"),
        };
    let narrowed_trace: ExecTrace = serde_json::from_value(
        engine
            .episodes()
            .get(narrowed_failure)
            .unwrap()
            .execution_trace
            .unwrap(),
    )
    .unwrap();
    assert_eq!(narrowed_trace.steps[0].procedure_called, Some(leavening.id));
    assert_eq!(narrowed_trace.steps[0].procedure_version, Some(2));
    assert_eq!(
        narrowed_trace.steps[0].contract_checks.requires[0].status,
        ConditionCheckStatus::Violated
    );
    let historical_v1 = engine
        .graph()
        .get_procedure_version(leavening.id, 1)
        .unwrap()
        .unwrap();
    assert_eq!(historical_v1.body, leavening.body);
    assert_eq!(historical_v1.contract.requires.len(), 0);

    let replayed_v1 = engine
        .replay_episode(verified_v1.id, factor_inputs(1))
        .unwrap();
    assert_eq!(replayed_v1.value, Value::Float(5.0));
    assert_eq!(
        replayed_v1
            .trace
            .steps
            .iter()
            .map(|step| (step.procedure_called.unwrap(), step.procedure_version))
            .collect::<Vec<_>>(),
        vec![
            (substitution.id, Some(1)),
            (leavening.id, Some(1)),
            (scale_recipe.id, Some(1)),
        ]
    );
    let failed_v1_replay = engine
        .replay_episode(failure_id, factor_inputs(2))
        .unwrap_err();
    assert!(failed_v1_replay.to_string().contains(injected_fault));
    assert_eq!(
        serde_json::to_vec(&engine.episodes().get(failure_id).unwrap()).unwrap(),
        immutable_episode_bytes
    );

    // Predicate-bound observations are produced through Engine execution.
    // Their exact signed facts trigger contradiction detection automatically.
    let rise_concept = Concept::new("pancake rise", MutabilityClass::DefeasibleGeneral);
    engine.admin_insert_concept(&rise_concept).unwrap();
    let rises = Procedure::new(
        "OBSERVE_SINGLE_BATCH_RISE",
        vec![Param::named("scale-factor")],
        Expr::Literal(Value::Bool(true)),
    )
    .with_concept(rise_concept.id);
    let stays_flat = Procedure::new(
        "OBSERVE_DOUBLE_BATCH_FLAT",
        vec![Param::named("scale-factor")],
        Expr::Literal(Value::Bool(false)),
    )
    .with_concept(rise_concept.id);
    engine.admin_insert_procedure(&rises).unwrap();
    engine.admin_insert_procedure(&stays_flat).unwrap();
    let left_evidence = engine
        .execute_procedure(
            rises.id,
            BTreeMap::from([("scale-factor".into(), Value::Int(1))]),
            Some(Value::Bool(true)),
        )
        .unwrap()
        .episode;
    let right_evidence = engine
        .execute_procedure(
            stays_flat.id,
            BTreeMap::from([("scale-factor".into(), Value::Int(2))]),
            Some(Value::Bool(false)),
        )
        .unwrap()
        .episode;
    assert!(engine.list_held_contradictions().unwrap().is_empty());
    let predicate = format!("concept:{}", rise_concept.id);
    let refinement_context = engine
        .refinement_context_for_predicate(
            &predicate,
            &BTreeMap::from([("scale-factor".into(), Value::Int(1))]),
        )
        .unwrap();
    let contradiction_id = refinement_context
        .applied
        .into_iter()
        .next()
        .expect("the unique recorded scope feature must refine automatically")
        .contradiction_id;
    let contradiction = engine
        .get_contradiction(contradiction_id)
        .unwrap()
        .expect("automatic refinement must remain durable");
    assert_eq!(contradiction.status, ContradictionStatus::Refined);
    let refinement = contradiction.refinement.unwrap();
    assert_eq!(refinement.left.supporting_episodes, vec![left_evidence.id]);
    assert_eq!(
        refinement.right.supporting_episodes,
        vec![right_evidence.id]
    );
    assert_eq!(refinement.left.scope[0].value, Value::Int(1));
    assert_eq!(refinement.right.scope[0].value, Value::Int(2));
    assert_eq!(
        engine
            .get_contradiction(contradiction_id)
            .unwrap()
            .unwrap()
            .status,
        ContradictionStatus::Refined
    );
}
