use std::collections::BTreeMap;

use ekg_adapt::{DemonstratedFeature, Implication, Uncertainty};
use ekg_core::{
    BinOp, Concept, Condition, Evaluation, Expr, Lifecycle, MutabilityClass, Param, Procedure,
    Value, VerifiabilityTier,
};
use ekg_credit::{
    AttributionMechanism, CounterfactualCandidate, CounterfactualChange, CounterfactualMode,
    Suspect,
};
use ekg_engine::{
    AdaptationEvidenceRef, AdaptationPlanRequest, AdaptationTarget, ApplyAdaptationRequest,
    AttributionSelector, CounterfactualMutation, CycleBudget, CycleDisposition, CycleInput,
    CycleProgress, Engine, EngineError, FailureAnalysisBudget, FailureAnalysisRequest,
    MutationScope, ProcedureVersionRef, ReplayVerification,
};
use ekg_episode::{EpisodeFeedback, FeedbackSource};

fn double() -> Procedure {
    Procedure::new(
        "DOUBLE",
        vec![Param::named("x")],
        Expr::BinOp {
            op: BinOp::Mul,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(2))),
        },
    )
}

fn inputs(value: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([("x".into(), Value::Int(value))])
}

fn contract_failure(engine: &Engine, procedure: &Procedure, value: i64) -> ekg_core::EpisodeId {
    let error = engine
        .execute_procedure(procedure.id, inputs(value), None)
        .unwrap_err();
    let EngineError::ExecutionFailed { episode_id, .. } = error else {
        panic!("injected contract should fail");
    };
    episode_id
}

fn analysis_request(episode_id: ekg_core::EpisodeId) -> FailureAnalysisRequest {
    FailureAnalysisRequest {
        episode_id,
        selected_feedback_id: None,
        candidates: Vec::new(),
        budget: FailureAnalysisBudget::default(),
    }
}

fn scope_plan_request(
    episode_id: ekg_core::EpisodeId,
    procedure: &Procedure,
    key: &str,
) -> AdaptationPlanRequest {
    AdaptationPlanRequest {
        idempotency_key: key.into(),
        analysis: analysis_request(episode_id),
        attribution: AttributionSelector {
            suspect: Suspect {
                procedure: procedure.id,
                version: procedure.version,
                trace_step: 0,
            },
            mechanism: AttributionMechanism::ContractViolation,
        },
        evidence: vec![AdaptationEvidenceRef {
            episode_id,
            selected_feedback_id: None,
        }],
        target: AdaptationTarget::ProcedureScope {
            procedure_id: procedure.id,
            expected_version: procedure.version,
            condition: Condition::described("exclude the demonstrated bad input").with_check(
                Expr::BinOp {
                    op: BinOp::Ne,
                    left: Box::new(Expr::Var("x".into())),
                    right: Box::new(Expr::Literal(Value::Int(7))),
                },
            ),
            learned_from: episode_id,
        },
        created_at: 100,
    }
}

#[test]
fn trusted_narrow_plan_is_persisted_idempotent_and_reconciles_dependents() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut procedure = double();
    procedure
        .contract
        .requires
        .push(
            Condition::described("injected failure").with_check(Expr::BinOp {
                op: BinOp::Ne,
                left: Box::new(Expr::Var("x".into())),
                right: Box::new(Expr::Literal(Value::Int(7))),
            }),
        );
    engine.admin_insert_procedure(&procedure).unwrap();
    let dependent = Procedure::new(
        "DEPENDENT",
        vec![Param::named("x")],
        Expr::Call {
            procedure: procedure.id,
            args: vec![Expr::Var("x".into())],
        },
    );
    engine.admin_insert_procedure(&dependent).unwrap();
    engine
        .execute_procedure(procedure.id, inputs(3), Some(Value::Int(6)))
        .unwrap();
    let episode_id = contract_failure(&engine, &procedure, 7);
    let episode_before = serde_json::to_value(engine.episodes().get(episode_id).unwrap()).unwrap();
    let request = scope_plan_request(episode_id, &procedure, "scope-plan-1");

    let plan = engine.plan_adaptation(request.clone()).unwrap();
    let retried = engine.plan_adaptation(request.clone()).unwrap();

    assert_eq!(plan.id, retried.id);
    assert_eq!(plan.mutation_scope, MutationScope::OnlineNarrow);
    assert_eq!(plan.evidence_gate.verified_episodes, 1);
    assert_eq!(plan.reconciliation.as_ref().unwrap().entries.len(), 1);
    let mut conflict = request;
    conflict.created_at += 1;
    assert!(
        engine
            .plan_adaptation(conflict)
            .unwrap_err()
            .to_string()
            .contains("idempotency conflict")
    );

    let apply = ApplyAdaptationRequest {
        plan_id: plan.id,
        idempotency_key: "scope-apply-1".into(),
        applied_at: 200,
    };
    let receipt = engine.apply_adaptation(apply.clone()).unwrap();
    let retried_receipt = engine.apply_adaptation(apply).unwrap();

    assert_eq!(
        serde_json::to_value(&receipt).unwrap(),
        serde_json::to_value(&retried_receipt).unwrap()
    );
    assert!(
        receipt
            .reconciliation
            .as_ref()
            .unwrap()
            .updated
            .contains(&ekg_engine::AdaptationKnowledgeRef::Procedure { id: dependent.id })
    );
    let revised = engine.graph().get_procedure(procedure.id).unwrap().unwrap();
    assert_eq!(revised.version, 2);
    assert_eq!(
        engine
            .graph()
            .get_procedure(dependent.id)
            .unwrap()
            .unwrap()
            .lifecycle,
        Lifecycle::UnderReview
    );
    assert_eq!(
        engine
            .graph()
            .list_procedure_versions(procedure.id)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        serde_json::to_value(engine.episodes().get(episode_id).unwrap()).unwrap(),
        episode_before
    );
    assert!(
        engine
            .get_adaptation(plan.id)
            .unwrap()
            .unwrap()
            .receipt
            .is_some()
    );
}

#[test]
fn forged_or_disconnected_evidence_cannot_create_a_mutating_plan() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut procedure = double();
    procedure.contract.requires.push(
        Condition::described("injected failure").with_check(Expr::Literal(Value::Bool(false))),
    );
    engine.admin_insert_procedure(&procedure).unwrap();
    let episode_id = contract_failure(&engine, &procedure, 7);
    let mut request = scope_plan_request(episode_id, &procedure, "forged-evidence");
    request.evidence.clear();
    assert!(
        engine
            .plan_adaptation(request)
            .unwrap_err()
            .to_string()
            .contains("canonical episode evidence")
    );

    let mut request = scope_plan_request(episode_id, &procedure, "forged-selector");
    request.attribution.suspect.version = 99;
    assert!(
        engine
            .plan_adaptation(request)
            .unwrap_err()
            .to_string()
            .contains("absent from the trusted")
    );
}

#[test]
fn broad_concept_revision_requires_one_shot_engine_issued_offline_capability() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("pancake rise", MutabilityClass::DefeasibleGeneral);
    engine.admin_insert_concept(&concept).unwrap();
    let mut procedure = double().with_concept(concept.id);
    procedure
        .contract
        .requires
        .push(
            Condition::described("injected kitchen fault").with_check(Expr::BinOp {
                op: BinOp::Ge,
                left: Box::new(Expr::Var("x".into())),
                right: Box::new(Expr::Literal(Value::Int(0))),
            }),
        );
    engine.admin_insert_procedure(&procedure).unwrap();
    // A broad structural change must protect at least one durable verified
    // behavior, not only the failure reports that motivated it.
    engine
        .execute_procedure(procedure.id, inputs(3), Some(Value::Int(6)))
        .unwrap();
    let evidence = (0..5)
        .map(|index| {
            let episode_id = contract_failure(&engine, &procedure, -1);
            let feedback = engine
                .record_authenticated_verifier_feedback(
                    &EpisodeFeedback::new(
                        episode_id,
                        Value::Text("flat pancake".into()),
                        feedback_evaluation(),
                        FeedbackSource::new(
                            "human",
                            Some(if index % 2 == 0 { "cook-a" } else { "cook-b" }.into()),
                        ),
                        format!("kitchen-feedback-{index}"),
                    ),
                    if index % 2 == 0 { "cook-a" } else { "cook-b" },
                )
                .unwrap();
            AdaptationEvidenceRef {
                episode_id,
                selected_feedback_id: Some(feedback.id),
            }
        })
        .collect::<Vec<_>>();
    let analyzed = evidence[0];
    let plan = engine
        .plan_adaptation(AdaptationPlanRequest {
            idempotency_key: "broad-plan".into(),
            analysis: FailureAnalysisRequest {
                selected_feedback_id: analyzed.selected_feedback_id,
                ..analysis_request(analyzed.episode_id)
            },
            attribution: AttributionSelector {
                suspect: Suspect {
                    procedure: procedure.id,
                    version: 1,
                    trace_step: 0,
                },
                mechanism: AttributionMechanism::ContractViolation,
            },
            evidence,
            target: AdaptationTarget::ConceptRevision {
                concept_id: concept.id,
                expected_version: 1,
                revised_description: "rise depends on active leavening and oven conditions".into(),
            },
            created_at: 300,
        })
        .unwrap();
    assert_eq!(plan.mutation_scope, MutationScope::OfflineBroad);

    let no_capability = engine
        .apply_adaptation(ApplyAdaptationRequest {
            plan_id: plan.id,
            idempotency_key: "broad-apply".into(),
            applied_at: 400,
        })
        .unwrap_err();
    assert!(no_capability.to_string().contains("offline capability"));

    let apply_request = ApplyAdaptationRequest {
        plan_id: plan.id,
        idempotency_key: "broad-apply".into(),
        applied_at: 400,
    };
    let capability = engine.issue_offline_capability(&apply_request).unwrap();
    let receipt = engine
        .apply_adaptation_offline(apply_request, &capability)
        .unwrap();
    assert!(matches!(
        receipt.outcome,
        ekg_engine::AdaptationOutcome::ConceptUpdated { concept_id } if concept_id == concept.id
    ));
    assert_eq!(
        engine.graph().current_concept_version(concept.id).unwrap(),
        2
    );
    assert_eq!(
        engine
            .graph()
            .get_concept_version(concept.id, 1)
            .unwrap()
            .unwrap()
            .description,
        None
    );
}

#[test]
fn broad_mutation_without_durable_regression_coverage_is_rejected() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new(
        "unverified structural claim",
        MutabilityClass::DefeasibleGeneral,
    );
    engine.admin_insert_concept(&concept).unwrap();
    let mut procedure = double().with_concept(concept.id);
    procedure.contract.requires.push(
        Condition::described("all demonstrations fail before a baseline exists")
            .with_check(Expr::Literal(Value::Bool(false))),
    );
    engine.admin_insert_procedure(&procedure).unwrap();

    let evidence = (0..5)
        .map(|index| {
            let episode_id = contract_failure(&engine, &procedure, index);
            let source = if index % 2 == 0 {
                "oracle-a"
            } else {
                "oracle-b"
            };
            let feedback = engine
                .record_authenticated_verifier_feedback(
                    &EpisodeFeedback::new(
                        episode_id,
                        Value::Text("does not establish a passing baseline".into()),
                        feedback_evaluation(),
                        FeedbackSource::new("deterministic-oracle", Some(source.into())),
                        format!("no-baseline-{index}"),
                    ),
                    source,
                )
                .unwrap();
            AdaptationEvidenceRef {
                episode_id,
                selected_feedback_id: Some(feedback.id),
            }
        })
        .collect::<Vec<_>>();
    let analyzed = evidence[0];
    let plan = engine
        .plan_adaptation(AdaptationPlanRequest {
            idempotency_key: "no-durable-regression-coverage".into(),
            analysis: FailureAnalysisRequest {
                selected_feedback_id: analyzed.selected_feedback_id,
                ..analysis_request(analyzed.episode_id)
            },
            attribution: AttributionSelector {
                suspect: Suspect {
                    procedure: procedure.id,
                    version: 1,
                    trace_step: 0,
                },
                mechanism: AttributionMechanism::ContractViolation,
            },
            evidence,
            target: AdaptationTarget::ConceptRevision {
                concept_id: concept.id,
                expected_version: 1,
                revised_description: "must not promote without tested behavior".into(),
            },
            created_at: 401,
        })
        .unwrap();
    assert_eq!(plan.mutation_scope, MutationScope::OfflineBroad);

    let request = ApplyAdaptationRequest {
        plan_id: plan.id,
        idempotency_key: "no-durable-regression-coverage-apply".into(),
        applied_at: 402,
    };
    let capability = engine.issue_offline_capability(&request).unwrap();
    let error = engine
        .apply_adaptation_offline(request, &capability)
        .expect_err("broad mutation without a locally verified baseline must be rejected");
    assert!(error.to_string().contains("0 applicable cases (minimum 1)"));
    let suite = engine
        .adaptation_regression_suite(plan.id)
        .unwrap()
        .unwrap();
    assert!(!suite.accepted);
    assert_eq!(suite.applicable, 0);
    assert_eq!(suite.failed, 0);
    assert_eq!(
        engine.graph().current_concept_version(concept.id).unwrap(),
        1
    );
}

#[test]
fn many_feedback_rows_for_one_execution_cannot_inflate_broad_evidence_thresholds() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("pancake rise", MutabilityClass::DefeasibleGeneral);
    engine.admin_insert_concept(&concept).unwrap();
    let mut procedure = double().with_concept(concept.id);
    procedure.contract.requires.push(
        Condition::described("injected kitchen fault")
            .with_check(Expr::Literal(Value::Bool(false))),
    );
    engine.admin_insert_procedure(&procedure).unwrap();
    let episode_id = contract_failure(&engine, &procedure, 7);
    let evidence = (0..5)
        .map(|index| {
            let feedback = engine
                .record_authenticated_verifier_feedback(
                    &EpisodeFeedback::new(
                        episode_id,
                        Value::Text("flat pancake".into()),
                        feedback_evaluation(),
                        FeedbackSource::new("human", Some(format!("cook-{index}"))),
                        format!("same-execution-feedback-{index}"),
                    ),
                    &format!("cook-{index}"),
                )
                .unwrap();
            AdaptationEvidenceRef {
                episode_id,
                selected_feedback_id: Some(feedback.id),
            }
        })
        .collect::<Vec<_>>();
    let plan = engine
        .plan_adaptation(AdaptationPlanRequest {
            idempotency_key: "one-execution-many-feedback".into(),
            analysis: FailureAnalysisRequest {
                selected_feedback_id: evidence[0].selected_feedback_id,
                ..analysis_request(episode_id)
            },
            attribution: AttributionSelector {
                suspect: Suspect {
                    procedure: procedure.id,
                    version: 1,
                    trace_step: 0,
                },
                mechanism: AttributionMechanism::ContractViolation,
            },
            evidence,
            target: AdaptationTarget::ConceptRevision {
                concept_id: concept.id,
                expected_version: 1,
                revised_description: "should not be authorized".into(),
            },
            created_at: 325,
        })
        .unwrap();

    assert_eq!(plan.evidence_gate.verified_episodes, 1);
    assert_eq!(plan.mutation_scope, MutationScope::NoGraphChange);
    assert!(matches!(
        plan.action,
        ekg_engine::AdaptationAction::ScheduleTest { .. }
    ));
}

#[test]
fn replay_confirmed_replacement_does_not_treat_a_failed_prediction_as_reality() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let incumbent = double();
    engine.admin_insert_procedure(&incumbent).unwrap();
    let failed = (0..3)
        .map(|_| {
            engine
                .execute_procedure(incumbent.id, inputs(7), Some(Value::Int(21)))
                .unwrap()
                .episode
        })
        .collect::<Vec<_>>();
    let mut challenger = incumbent.clone();
    challenger.version = 2;
    challenger.body = Expr::BinOp {
        op: BinOp::Mul,
        left: Box::new(Expr::Var("x".into())),
        right: Box::new(Expr::Literal(Value::Int(3))),
    };
    let mutation = CounterfactualMutation::ReplaceBody {
        target: ProcedureVersionRef {
            id: incumbent.id,
            version: 1,
        },
        body: challenger.body.clone(),
        verification: ReplayVerification::DeterministicExpected {
            expected: Value::Int(21),
        },
    };
    let plan = engine
        .plan_adaptation(AdaptationPlanRequest {
            idempotency_key: "replacement-plan".into(),
            analysis: FailureAnalysisRequest {
                episode_id: failed[0].id,
                selected_feedback_id: None,
                candidates: vec![CounterfactualCandidate {
                    suspect: Suspect {
                        procedure: incumbent.id,
                        version: 1,
                        trace_step: 0,
                    },
                    prior_score: 1.0,
                    change: CounterfactualChange {
                        description: "replace double with triple".into(),
                        replacement: serde_json::to_value(mutation).unwrap(),
                    },
                    mode: CounterfactualMode::Deterministic,
                }],
                budget: FailureAnalysisBudget::default(),
            },
            attribution: AttributionSelector {
                suspect: Suspect {
                    procedure: incumbent.id,
                    version: 1,
                    trace_step: 0,
                },
                mechanism: AttributionMechanism::CounterfactualReplay,
            },
            evidence: failed
                .iter()
                .map(|episode| AdaptationEvidenceRef {
                    episode_id: episode.id,
                    selected_feedback_id: None,
                })
                .collect(),
            target: AdaptationTarget::ProcedureReplacement {
                incumbent_id: incumbent.id,
                incumbent_version: 1,
                challenger: Box::new(challenger),
            },
            created_at: 350,
        })
        .unwrap();
    assert_eq!(plan.mutation_scope, MutationScope::NoGraphChange);
    assert!(!plan.evidence_gate.challenger_beats_incumbent);
    assert!(matches!(
        plan.action,
        ekg_engine::AdaptationAction::ScheduleTest { .. }
    ));
}

fn verified_replacement_plan(
    engine: &Engine,
    incumbent: &Procedure,
    challenger: Procedure,
    key: &str,
) -> ekg_engine::AdaptationPlan {
    verified_replacement_plan_with_evidence(engine, incumbent, challenger, key, true)
}

fn verified_replacement_plan_with_evidence(
    engine: &Engine,
    incumbent: &Procedure,
    mut challenger: Procedure,
    key: &str,
    include_existing_regression_in_evidence: bool,
) -> ekg_engine::AdaptationPlan {
    challenger.version = 2;
    let failed = (0..3)
        .map(|index| {
            let episode = engine
                .execute_procedure(incumbent.id, inputs(7), Some(Value::Int(21)))
                .unwrap()
                .episode;
            let feedback = engine
                .record_authenticated_verifier_feedback(
                    &EpisodeFeedback::new(
                        episode.id,
                        Value::Int(21),
                        Evaluation {
                            tier: VerifiabilityTier::Hard,
                            success: false,
                            details: "independent deterministic oracle rejected the incumbent"
                                .into(),
                            surprise: Some(1.0),
                        },
                        FeedbackSource::new(
                            "deterministic-oracle",
                            Some("replacement-suite".into()),
                        ),
                        format!("verified-replacement-{key}-{index}"),
                    ),
                    "replacement-suite",
                )
                .unwrap();
            (episode, feedback)
        })
        .collect::<Vec<_>>();
    let regression = engine
        .execute_procedure(incumbent.id, inputs(3), Some(Value::Int(6)))
        .unwrap()
        .episode;
    let mutation = CounterfactualMutation::ReplaceBody {
        target: ProcedureVersionRef {
            id: incumbent.id,
            version: 1,
        },
        body: challenger.body.clone(),
        verification: ReplayVerification::DeterministicExpected {
            expected: Value::Int(21),
        },
    };
    let mut evidence = failed
        .iter()
        .map(|(episode, feedback)| AdaptationEvidenceRef {
            episode_id: episode.id,
            selected_feedback_id: Some(feedback.id),
        })
        .collect::<Vec<_>>();
    if include_existing_regression_in_evidence {
        evidence.push(AdaptationEvidenceRef {
            episode_id: regression.id,
            selected_feedback_id: None,
        });
    }
    // The analyzed execution is also retained without substituting the later
    // oracle observation, so causal replay must improve on what happened then.
    evidence.push(AdaptationEvidenceRef {
        episode_id: failed[0].0.id,
        selected_feedback_id: None,
    });
    engine
        .plan_adaptation(AdaptationPlanRequest {
            idempotency_key: key.into(),
            analysis: FailureAnalysisRequest {
                episode_id: failed[0].0.id,
                selected_feedback_id: None,
                candidates: vec![CounterfactualCandidate {
                    suspect: Suspect {
                        procedure: incumbent.id,
                        version: 1,
                        trace_step: 0,
                    },
                    prior_score: 0.0,
                    change: CounterfactualChange {
                        description: "replace only the failing branch".into(),
                        replacement: serde_json::to_value(mutation).unwrap(),
                    },
                    mode: CounterfactualMode::Deterministic,
                }],
                budget: FailureAnalysisBudget::default(),
            },
            attribution: AttributionSelector {
                suspect: Suspect {
                    procedure: incumbent.id,
                    version: 1,
                    trace_step: 0,
                },
                mechanism: AttributionMechanism::CounterfactualReplay,
            },
            evidence,
            target: AdaptationTarget::ProcedureReplacement {
                incumbent_id: incumbent.id,
                incumbent_version: 1,
                challenger: Box::new(challenger),
            },
            created_at: 355,
        })
        .unwrap()
}

fn broad_apply_request(engine: &Engine, key: &str) -> ApplyAdaptationRequest {
    let incumbent = double();
    engine.admin_insert_procedure(&incumbent).unwrap();
    let mut challenger = incumbent.clone();
    challenger.body = Expr::If {
        cond: Box::new(Expr::BinOp {
            op: BinOp::Eq,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(7))),
        }),
        then: Box::new(Expr::Literal(Value::Int(21))),
        else_: Box::new(incumbent.body.clone()),
    };
    let plan = verified_replacement_plan(engine, &incumbent, challenger, &format!("{key}-plan"));
    assert_eq!(plan.mutation_scope, MutationScope::OfflineBroad);
    ApplyAdaptationRequest {
        plan_id: plan.id,
        idempotency_key: format!("{key}-apply"),
        applied_at: 1,
    }
}

#[test]
fn replacement_requires_verified_reality_and_preserves_successful_regressions() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let incumbent = double();
    engine.admin_insert_procedure(&incumbent).unwrap();
    let mut challenger = incumbent.clone();
    challenger.body = Expr::If {
        cond: Box::new(Expr::BinOp {
            op: BinOp::Eq,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(7))),
        }),
        then: Box::new(Expr::Literal(Value::Int(21))),
        else_: Box::new(incumbent.body.clone()),
    };
    let plan =
        verified_replacement_plan(&engine, &incumbent, challenger, "verified-replacement-plan");
    assert!(plan.evidence_gate.challenger_beats_incumbent);
    assert_eq!(plan.mutation_scope, MutationScope::OfflineBroad);

    let apply_request = ApplyAdaptationRequest {
        plan_id: plan.id,
        idempotency_key: "verified-replacement-apply".into(),
        applied_at: 360,
    };
    let capability = engine.issue_offline_capability(&apply_request).unwrap();
    engine
        .apply_adaptation_offline(apply_request, &capability)
        .unwrap();
    let suite = engine
        .adaptation_regression_suite(plan.id)
        .unwrap()
        .unwrap();
    assert!(suite.accepted);
    assert_eq!(suite.failed, 0);
    assert!(suite.passed >= 1);
    assert_eq!(
        engine
            .execute_procedure(incumbent.id, inputs(7), None)
            .unwrap()
            .value,
        Value::Int(21)
    );
    assert_eq!(
        engine
            .execute_procedure(incumbent.id, inputs(3), None)
            .unwrap()
            .value,
        Value::Int(6)
    );
    // The successful replacement advances the procedure version but leaves
    // the v1 evidence that authorized it intact and queryable.
    assert_eq!(
        engine
            .episodes()
            .list_verified_regression_cases(incumbent.id, 1)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn full_regression_suite_rejects_a_broad_change_that_selected_evidence_misses() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let incumbent = double();
    engine.admin_insert_procedure(&incumbent).unwrap();

    // The supporting evidence used for the replacement is all about x = 7:
    // independent feedback says the incumbent's 14 should have been 21.
    // The candidate fixes that case but incorrectly changes every other input.
    let mut challenger = incumbent.clone();
    challenger.body = Expr::If {
        cond: Box::new(Expr::BinOp {
            op: BinOp::Eq,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(7))),
        }),
        then: Box::new(Expr::Literal(Value::Int(21))),
        else_: Box::new(Expr::Literal(Value::Int(0))),
    };
    let plan = verified_replacement_plan_with_evidence(
        &engine,
        &incumbent,
        challenger,
        "full-suite-catches-unselected-regression",
        false,
    );
    assert_eq!(plan.mutation_scope, MutationScope::OfflineBroad);
    assert!(plan.evidence_gate.challenger_beats_incumbent);

    let apply_request = ApplyAdaptationRequest {
        plan_id: plan.id,
        idempotency_key: "full-suite-catches-unselected-regression-apply".into(),
        applied_at: 361,
    };
    let capability = engine.issue_offline_capability(&apply_request).unwrap();
    let error = engine
        .apply_adaptation_offline(apply_request, &capability)
        .expect_err("the full durable suite must block the regression");
    assert!(error.to_string().contains("regression suite rejected"));

    let suite = engine
        .adaptation_regression_suite(plan.id)
        .unwrap()
        .unwrap();
    assert!(!suite.accepted);
    assert_eq!(suite.passed, 0);
    assert_eq!(suite.failed, 1);
    assert_eq!(suite.cases.len(), 1);
    assert_eq!(suite.cases[0].expected_output, Value::Int(6));
    assert_eq!(suite.cases[0].actual_output, Some(Value::Int(0)));
    assert!(engine.recover_pending_adaptations().unwrap().is_empty());

    // The rejected broad mutation never changes the incumbent or its
    // append-only verified history.
    assert_eq!(
        engine
            .execute_procedure(incumbent.id, inputs(3), None)
            .unwrap()
            .value,
        Value::Int(6)
    );
    assert!(
        engine
            .get_adaptation(plan.id)
            .unwrap()
            .unwrap()
            .receipt
            .is_none()
    );
}

#[test]
fn replacement_rejects_unrelated_procedure_changes() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let incumbent = double();
    engine.admin_insert_procedure(&incumbent).unwrap();
    let mut challenger = incumbent.clone();
    challenger.name = "UNRELATED_RENAME".into();
    challenger.body = Expr::If {
        cond: Box::new(Expr::BinOp {
            op: BinOp::Eq,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(7))),
        }),
        then: Box::new(Expr::Literal(Value::Int(21))),
        else_: Box::new(incumbent.body.clone()),
    };
    let plan = verified_replacement_plan(&engine, &incumbent, challenger, "wide-replacement-plan");
    assert!(!plan.evidence_gate.challenger_beats_incumbent);
    assert_eq!(plan.mutation_scope, MutationScope::NoGraphChange);
}

#[test]
fn offline_capability_is_denied_while_a_teacher_cycle_is_pending() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let request = broad_apply_request(&engine, "blocked-by-active-cycle");
    let progress = engine
        .begin_cycle(CycleInput {
            situation: "unknown thing".into(),
            environment: BTreeMap::new(),
            assumptions: Vec::new(),
            teacher_allowed: true,
            budget: CycleBudget {
                max_exec_steps: 100,
                max_context_items: 10,
                max_teacher_turns: 1,
            },
        })
        .unwrap();
    assert!(matches!(progress, CycleProgress::NeedTeacher { .. }));
    assert!(
        engine
            .issue_offline_capability(&request)
            .unwrap_err()
            .to_string()
            .contains("active or pending cycles")
    );
}

#[test]
fn maintenance_lease_and_active_cycles_exclude_each_other_across_engines() {
    let path = std::env::temp_dir().join(format!(
        "ekg-runtime-exclusion-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().into_owned();
    let request = {
        let setup = Engine::open_with_admin(&path_text, "test-admin").unwrap();
        broad_apply_request(&setup, "cross-process-maintenance")
    };
    let mut reasoning = Engine::open(&path_text).unwrap();
    let CycleProgress::NeedTeacher { .. } = reasoning
        .begin_cycle(CycleInput {
            situation: "cross-process unknown".into(),
            environment: BTreeMap::new(),
            assumptions: Vec::new(),
            teacher_allowed: true,
            budget: CycleBudget {
                max_exec_steps: 100,
                max_context_items: 10,
                max_teacher_turns: 1,
            },
        })
        .unwrap()
    else {
        panic!("cycle should remain active while awaiting a teacher");
    };
    let mut maintenance = Engine::open_with_admin(&path_text, "test-admin").unwrap();
    assert!(
        maintenance
            .issue_offline_capability(&request)
            .unwrap_err()
            .to_string()
            .contains("active or pending cycles")
    );
    drop(maintenance);
    drop(reasoning);
    std::fs::remove_file(path).unwrap();

    let path = std::env::temp_dir().join(format!(
        "ekg-runtime-exclusion-reverse-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().into_owned();
    let mut maintenance = Engine::open_with_admin(&path_text, "test-admin").unwrap();
    let request = broad_apply_request(&maintenance, "reverse-process-maintenance");
    let _lease = maintenance.issue_offline_capability(&request).unwrap();
    let mut reasoning = Engine::open(&path_text).unwrap();
    assert!(
        reasoning
            .begin_cycle(CycleInput {
                situation: "blocked by maintenance".into(),
                environment: BTreeMap::new(),
                assumptions: Vec::new(),
                teacher_allowed: false,
                budget: CycleBudget {
                    max_exec_steps: 100,
                    max_context_items: 10,
                    max_teacher_turns: 0,
                },
            })
            .unwrap_err()
            .to_string()
            .contains("maintenance operation is active")
    );
    drop(reasoning);
    drop(maintenance);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn plans_and_receipts_survive_engine_reopen() {
    let path = std::env::temp_dir().join(format!("ekg-adaptation-{}.sqlite", uuid::Uuid::new_v4()));
    let path_text = path.to_string_lossy().to_string();
    let plan_id;
    {
        let mut engine = Engine::open_with_admin(&path_text, "test-admin").unwrap();
        let mut procedure = double();
        procedure
            .contract
            .requires
            .push(
                Condition::described("injected failure").with_check(Expr::BinOp {
                    op: BinOp::Ne,
                    left: Box::new(Expr::Var("x".into())),
                    right: Box::new(Expr::Literal(Value::Int(7))),
                }),
            );
        engine.admin_insert_procedure(&procedure).unwrap();
        engine
            .execute_procedure(procedure.id, inputs(3), Some(Value::Int(6)))
            .unwrap();
        let episode_id = contract_failure(&engine, &procedure, 7);
        let plan = engine
            .plan_adaptation(scope_plan_request(
                episode_id,
                &procedure,
                "persistent-plan",
            ))
            .unwrap();
        plan_id = plan.id;
        engine
            .apply_adaptation(ApplyAdaptationRequest {
                plan_id,
                idempotency_key: "persistent-apply".into(),
                applied_at: 500,
            })
            .unwrap();
    }
    let reopened = Engine::open(&path_text).unwrap();
    let record = reopened.get_adaptation(plan_id).unwrap().unwrap();
    assert_eq!(record.plan.id, plan_id);
    assert!(record.receipt.is_some());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn engine_open_resumes_an_exact_interrupted_adaptation_stage_once() {
    let path = std::env::temp_dir().join(format!(
        "ekg-interrupted-adaptation-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().to_string();
    let request;
    let plan_id;
    let procedure_id;
    let dependent_id;
    {
        let engine = Engine::open_with_admin(&path_text, "test-admin").unwrap();
        let mut procedure = double();
        procedure.contract.requires.push(
            Condition::described("reject the demonstrated input").with_check(Expr::BinOp {
                op: BinOp::Ne,
                left: Box::new(Expr::Var("x".into())),
                right: Box::new(Expr::Literal(Value::Int(7))),
            }),
        );
        engine.admin_insert_procedure(&procedure).unwrap();
        let dependent = Procedure::new(
            "INTERRUPTED_DEPENDENT",
            vec![Param::named("x")],
            Expr::Call {
                procedure: procedure.id,
                args: vec![Expr::Var("x".into())],
            },
        );
        engine.admin_insert_procedure(&dependent).unwrap();
        engine
            .execute_procedure(procedure.id, inputs(3), Some(Value::Int(6)))
            .unwrap();
        let failure_id = contract_failure(&engine, &procedure, 7);
        let plan = engine
            .plan_adaptation(scope_plan_request(
                failure_id,
                &procedure,
                "interrupted-stage-plan",
            ))
            .unwrap();
        plan_id = plan.id;
        procedure_id = procedure.id;
        dependent_id = dependent.id;
        request = ApplyAdaptationRequest {
            plan_id,
            idempotency_key: "interrupted-stage-apply".into(),
            applied_at: 550,
        };
    }
    {
        let connection = rusqlite::Connection::open(&path_text).unwrap();
        let stage = serde_json::json!({
            "request": &request,
            "outcome": null,
            "reconciliationComplete": false,
            "reconciliation": null
        });
        connection
            .execute(
                "INSERT INTO engine_adaptation_apply_stages
                    (plan_id, idempotency_key, request_json, stage_json, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    plan_id.0.to_string(),
                    request.idempotency_key,
                    serde_json::to_string(&request).unwrap(),
                    serde_json::to_string(&stage).unwrap(),
                    request.applied_at,
                ],
            )
            .unwrap();
    }

    let mut reopened = Engine::open(&path_text).unwrap();
    let record = reopened.get_adaptation(plan_id).unwrap().unwrap();
    let startup_receipt = record.receipt.unwrap();
    assert_eq!(startup_receipt.plan_id, plan_id);
    assert_eq!(startup_receipt.idempotency_key, request.idempotency_key);
    assert_eq!(startup_receipt.applied_at, request.applied_at);
    assert!(matches!(
        startup_receipt.outcome,
        ekg_engine::AdaptationOutcome::ProcedureUpdated {
            procedure_id: id,
            previous_version: 1,
            current_version: 2,
        } if id == procedure_id
    ));
    assert_eq!(
        reopened
            .graph()
            .get_procedure(dependent_id)
            .unwrap()
            .unwrap()
            .lifecycle,
        Lifecycle::UnderReview
    );
    let retry = reopened.apply_adaptation(request.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&retry).unwrap(),
        serde_json::to_value(&startup_receipt).unwrap()
    );
    assert_eq!(
        reopened
            .graph()
            .list_procedure_versions(procedure_id)
            .unwrap()
            .len(),
        2
    );
    drop(reopened);

    let mut reopened_again = Engine::open(&path_text).unwrap();
    let second_retry = reopened_again.apply_adaptation(request.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(second_retry).unwrap(),
        serde_json::to_value(startup_receipt).unwrap()
    );
    let mut conflicting = request;
    conflicting.idempotency_key = "different-apply-key".into();
    assert!(
        reopened_again
            .apply_adaptation(conflicting)
            .unwrap_err()
            .to_string()
            .contains("idempotency conflict")
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn expired_broad_stage_reacquires_exclusion_and_receipted_retry_releases_stale_lease() {
    let path = std::env::temp_dir().join(format!(
        "ekg-expired-broad-stage-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().into_owned();
    let request;
    let plan_id;
    let request_digest;
    {
        let mut engine = Engine::open_with_admin(&path_text, "test-admin").unwrap();
        let incumbent = double();
        engine.admin_insert_procedure(&incumbent).unwrap();
        let mut challenger = incumbent.clone();
        challenger.body = Expr::If {
            cond: Box::new(Expr::BinOp {
                op: BinOp::Eq,
                left: Box::new(Expr::Var("x".into())),
                right: Box::new(Expr::Literal(Value::Int(7))),
            }),
            then: Box::new(Expr::Literal(Value::Int(21))),
            else_: Box::new(incumbent.body.clone()),
        };
        let plan =
            verified_replacement_plan(&engine, &incumbent, challenger, "expired-broad-stage-plan");
        assert_eq!(plan.mutation_scope, MutationScope::OfflineBroad);
        plan_id = plan.id;
        request = ApplyAdaptationRequest {
            plan_id,
            idempotency_key: "expired-broad-stage-apply".into(),
            applied_at: 777,
        };
        let _capability = engine.issue_offline_capability(&request).unwrap();
        let connection = rusqlite::Connection::open(&path_text).unwrap();
        request_digest = connection
            .query_row(
                "SELECT request_digest FROM engine_maintenance WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        let stage = serde_json::json!({
            "request": &request,
            "outcome": null,
            "reconciliationComplete": false,
            "reconciliation": null
        });
        connection
            .execute(
                "INSERT INTO engine_adaptation_apply_stages
                    (plan_id, idempotency_key, request_json, stage_json, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    plan_id.0.to_string(),
                    request.idempotency_key,
                    serde_json::to_string(&request).unwrap(),
                    serde_json::to_string(&stage).unwrap(),
                    request.applied_at,
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE engine_maintenance SET expires_at = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();
    }

    let mut reopened = Engine::open(&path_text).unwrap();
    assert!(
        reopened
            .get_adaptation(plan_id)
            .unwrap()
            .unwrap()
            .receipt
            .is_some()
    );
    let connection = rusqlite::Connection::open(&path_text).unwrap();
    let owner_after_recovery: Option<String> = connection
        .query_row(
            "SELECT owner_id FROM engine_maintenance WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(owner_after_recovery.is_none());

    // Recreate the receipt-before-release crash window with a lease now held
    // by another instance. Idempotent retry may return the durable receipt,
    // but it must never clear a potentially newer owner's live lease.
    let foreign_owner = uuid::Uuid::new_v4().to_string();
    connection
        .execute(
            "UPDATE engine_maintenance
             SET owner_id = ?1, request_digest = ?2, expires_at = 4102444800
             WHERE singleton = 1",
            rusqlite::params![&foreign_owner, &request_digest],
        )
        .unwrap();
    reopened.apply_adaptation(request).unwrap();
    let owner_after_retry: Option<String> = connection
        .query_row(
            "SELECT owner_id FROM engine_maintenance WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(owner_after_retry.as_deref(), Some(foreign_owner.as_str()));
    drop(connection);
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn assumption_corrections_are_durable_for_future_context_assembly() {
    let path = std::env::temp_dir().join(format!(
        "ekg-assumption-adaptation-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().to_string();
    {
        let mut engine = Engine::open_with_admin(&path_text, "test-admin").unwrap();
        let mut procedure = double();
        procedure.contract.requires.push(
            Condition::described("injected failure").with_check(Expr::Literal(Value::Bool(false))),
        );
        engine.admin_insert_procedure(&procedure).unwrap();
        let episode_id = contract_failure(&engine, &procedure, 7);
        let plan = engine
            .plan_adaptation(AdaptationPlanRequest {
                idempotency_key: "durable-assumption-plan".into(),
                analysis: analysis_request(episode_id),
                attribution: AttributionSelector {
                    suspect: Suspect {
                        procedure: procedure.id,
                        version: procedure.version,
                        trace_step: 0,
                    },
                    mechanism: AttributionMechanism::ContractViolation,
                },
                evidence: vec![AdaptationEvidenceRef {
                    episode_id,
                    selected_feedback_id: None,
                }],
                target: AdaptationTarget::Assumption {
                    key: "oven-temperature-c".into(),
                    replacement: Value::Int(205),
                },
                created_at: 600,
            })
            .unwrap();
        assert!(matches!(
            plan.action,
            ekg_engine::AdaptationAction::FixAssumption { .. }
        ));
        engine
            .apply_adaptation(ApplyAdaptationRequest {
                plan_id: plan.id,
                idempotency_key: "durable-assumption-apply".into(),
                applied_at: 601,
            })
            .unwrap();
        assert_eq!(
            engine.assumption_override("oven-temperature-c").unwrap(),
            Some(Value::Int(205))
        );
    }
    let mut reopened = Engine::open(&path_text).unwrap();
    assert_eq!(
        reopened.assumption_overrides().unwrap(),
        std::collections::HashMap::from([("oven-temperature-c".into(), Value::Int(205))])
    );
    let progress = reopened
        .begin_cycle(CycleInput {
            situation: "future corrected context".into(),
            environment: BTreeMap::from([("oven-temperature-c".into(), Value::Int(180))]),
            assumptions: vec![
                ekg_core::Assumption {
                    description: "oven-temperature-c".into(),
                    basis: "teacher".into(),
                    concept: None,
                },
                ekg_core::Assumption {
                    description: "oven-temperature-c".into(),
                    basis: "inferred".into(),
                    concept: None,
                },
                ekg_core::Assumption {
                    description: "pan-is-greased".into(),
                    basis: "assumed".into(),
                    concept: None,
                },
            ],
            teacher_allowed: false,
            budget: CycleBudget {
                max_exec_steps: 100,
                max_context_items: 10,
                max_teacher_turns: 0,
            },
        })
        .unwrap();
    let CycleProgress::Completed(outcome) = progress else {
        panic!("teacher-disabled cycle should complete");
    };
    assert_eq!(
        outcome
            .episode
            .context
            .environment
            .get("oven-temperature-c"),
        Some(&Value::Int(180)),
        "caller environment remains separate from assumption correction"
    );
    assert_eq!(outcome.episode.context.assumptions.len(), 2);
    assert!(
        outcome
            .episode
            .context
            .assumptions
            .iter()
            .any(|assumption| {
                assumption.description == "oven-temperature-c = 205"
                    && assumption.basis == "corrected"
            })
    );
    assert!(
        outcome
            .episode
            .context
            .assumptions
            .iter()
            .all(|assumption| assumption.description != "oven-temperature-c")
    );

    let observed = reopened
        .begin_cycle(CycleInput {
            situation: "fresh observed context".into(),
            environment: BTreeMap::new(),
            assumptions: vec![ekg_core::Assumption {
                description: "oven-temperature-c".into(),
                basis: "observed".into(),
                concept: None,
            }],
            teacher_allowed: false,
            budget: CycleBudget {
                max_exec_steps: 100,
                max_context_items: 10,
                max_teacher_turns: 0,
            },
        })
        .unwrap();
    let CycleProgress::Completed(observed) = observed else {
        panic!("teacher-disabled cycle should complete");
    };
    assert_eq!(observed.episode.context.assumptions.len(), 1);
    assert_eq!(
        observed.episode.context.assumptions[0].description,
        "oven-temperature-c"
    );
    assert_eq!(observed.episode.context.assumptions[0].basis, "observed");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn contradiction_reads_are_engine_owned_and_episode_backed() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("pancake rise", MutabilityClass::DefeasibleGeneral);
    engine.admin_insert_concept(&concept).unwrap();
    let left_procedure = Procedure::new("OBSERVE_RISE", vec![], Expr::Literal(Value::Bool(true)))
        .with_concept(concept.id);
    let right_procedure =
        Procedure::new("OBSERVE_NO_RISE", vec![], Expr::Literal(Value::Bool(false)))
            .with_concept(concept.id);
    engine.admin_insert_procedure(&left_procedure).unwrap();
    engine.admin_insert_procedure(&right_procedure).unwrap();
    let left_episode = engine
        .execute_procedure(left_procedure.id, BTreeMap::new(), Some(Value::Bool(true)))
        .unwrap()
        .episode;
    let right_episode = engine
        .execute_procedure(
            right_procedure.id,
            BTreeMap::new(),
            Some(Value::Bool(false)),
        )
        .unwrap()
        .episode;
    let contradiction = engine
        .list_held_contradictions()
        .unwrap()
        .into_iter()
        .next()
        .expect("conflicting semantic observations must be detected automatically");
    assert_eq!(
        contradiction.left.implication,
        Implication::for_concept(concept.id, Value::Bool(true))
    );
    assert_eq!(
        contradiction.left.supporting_episodes,
        vec![left_episode.id]
    );
    assert_eq!(
        contradiction.right.supporting_episodes,
        vec![right_episode.id]
    );

    assert_eq!(
        engine.get_contradiction(contradiction.id).unwrap(),
        Some(contradiction.clone())
    );
    assert_eq!(
        engine.list_held_contradictions().unwrap(),
        vec![contradiction.clone()]
    );

    let CycleProgress::Completed(outcome) = engine
        .begin_cycle(CycleInput {
            situation: "pancake rise".into(),
            environment: BTreeMap::new(),
            assumptions: Vec::new(),
            teacher_allowed: false,
            budget: CycleBudget {
                max_exec_steps: 100,
                max_context_items: 10,
                max_teacher_turns: 0,
            },
        })
        .unwrap()
    else {
        panic!("teacher-disabled local reasoning must complete")
    };
    assert_eq!(outcome.disposition, CycleDisposition::Provisional);
    assert_eq!(
        outcome.episode.context.held_contradictions,
        vec![contradiction.id.0]
    );
}

#[test]
fn engine_contradiction_refinement_persists_and_clears_inherited_uncertainty() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("pancake rise", MutabilityClass::DefeasibleGeneral);
    engine.admin_insert_concept(&concept).unwrap();
    let left_procedure = Procedure::new(
        "OBSERVE_SINGLE_BATCH",
        vec![Param::named("scale-factor"), Param::named("oven-profile")],
        Expr::Literal(Value::Bool(true)),
    )
    .with_concept(concept.id);
    let right_procedure = Procedure::new(
        "OBSERVE_DOUBLE_BATCH",
        vec![Param::named("scale-factor"), Param::named("oven-profile")],
        Expr::Literal(Value::Bool(false)),
    )
    .with_concept(concept.id);
    engine.admin_insert_procedure(&left_procedure).unwrap();
    engine.admin_insert_procedure(&right_procedure).unwrap();
    let left_episode = engine
        .execute_procedure(
            left_procedure.id,
            BTreeMap::from([
                ("scale-factor".into(), Value::Int(1)),
                ("oven-profile".into(), Value::Text("low".into())),
            ]),
            Some(Value::Bool(true)),
        )
        .unwrap()
        .episode;
    let right_episode = engine
        .execute_procedure(
            right_procedure.id,
            BTreeMap::from([
                ("scale-factor".into(), Value::Int(2)),
                ("oven-profile".into(), Value::Text("high".into())),
            ]),
            Some(Value::Bool(false)),
        )
        .unwrap()
        .episode;
    let contradiction = engine
        .list_held_contradictions()
        .unwrap()
        .into_iter()
        .next()
        .expect("conflicting semantic observations must be detected automatically");
    let dependent = Concept::new("recipe plan", MutabilityClass::DefeasibleGeneral);
    engine.admin_insert_concept(&dependent).unwrap();
    let dependent_claim = format!("concept:{}", dependent.id);
    engine
        .admin_add_claim_dependency(&dependent_claim, &contradiction.left.id)
        .unwrap();
    assert_eq!(
        engine.uncertainty_for_claim(&dependent_claim).unwrap(),
        Uncertainty::HeldContradictions(vec![contradiction.id])
    );
    let CycleProgress::Completed(inherited) = engine
        .begin_cycle(CycleInput {
            situation: "recipe plan".into(),
            environment: BTreeMap::new(),
            assumptions: Vec::new(),
            teacher_allowed: false,
            budget: CycleBudget {
                max_exec_steps: 100,
                max_context_items: 10,
                max_teacher_turns: 0,
            },
        })
        .unwrap()
    else {
        panic!("teacher-disabled dependent reasoning must terminate")
    };
    assert_eq!(
        inherited.episode.context.held_contradictions,
        vec![contradiction.id.0]
    );

    let refinement = engine
        .admin_refine_contradiction(
            contradiction.id,
            DemonstratedFeature::new(
                "scale-factor",
                Value::Int(1),
                left_episode.id,
                Value::Int(2),
                right_episode.id,
            )
            .unwrap(),
            611,
        )
        .unwrap();
    assert_eq!(
        engine.uncertainty_for_claim(&dependent_claim).unwrap(),
        Uncertainty::Certain
    );
    assert_eq!(
        engine.refinements_for_claim(&dependent_claim).unwrap(),
        vec![refinement]
    );

    let CycleProgress::Completed(matched) = engine
        .begin_cycle(CycleInput {
            situation: "pancake rise at scale two".into(),
            environment: BTreeMap::from([
                ("scale-factor".into(), Value::Int(2)),
                ("oven-profile".into(), Value::Text("high".into())),
            ]),
            assumptions: Vec::new(),
            teacher_allowed: false,
            budget: CycleBudget {
                max_exec_steps: 100,
                max_context_items: 10,
                max_teacher_turns: 0,
            },
        })
        .unwrap()
    else {
        panic!("a demonstrated scope should resolve locally")
    };
    assert_eq!(matched.disposition, CycleDisposition::Verified);
    assert_eq!(matched.answer, Some(Value::Bool(false)));
    assert_eq!(matched.episode.context.applied_refinements.len(), 1);
    assert_eq!(
        matched.episode.context.applied_refinements[0].claim_id,
        contradiction.right.id
    );

    let CycleProgress::Completed(unseen_scope) = engine
        .begin_cycle(CycleInput {
            situation: "pancake rise at scale three".into(),
            environment: BTreeMap::from([
                ("scale-factor".into(), Value::Int(3)),
                ("oven-profile".into(), Value::Text("high".into())),
            ]),
            assumptions: Vec::new(),
            teacher_allowed: false,
            budget: CycleBudget {
                max_exec_steps: 100,
                max_context_items: 10,
                max_teacher_turns: 0,
            },
        })
        .unwrap()
    else {
        panic!("teacher-disabled unseen scope should terminate safely")
    };
    assert_eq!(unseen_scope.disposition, CycleDisposition::Abstained);
    assert_eq!(
        unseen_scope.episode.context.unresolved_refinements,
        vec![contradiction.id.0]
    );
}

#[test]
fn adaptation_target_wire_is_tagged_and_camel_case() {
    let procedure = double();
    let value = serde_json::to_value(AdaptationTarget::ProcedureScope {
        procedure_id: procedure.id,
        expected_version: 1,
        condition: Condition::described("bounded").with_check(Expr::Literal(Value::Bool(true))),
        learned_from: ekg_core::EpisodeId::new(),
    })
    .unwrap();

    assert_eq!(value["kind"], "procedure_scope");
    assert!(value.get("procedureId").is_some());
    assert!(value.get("expectedVersion").is_some());
    assert!(value.get("learnedFrom").is_some());
    assert!(value.get("procedure_id").is_none());
    assert!(
        serde_json::from_value::<ApplyAdaptationRequest>(serde_json::json!({
            "planId": uuid::Uuid::new_v4(),
            "idempotencyKey": "remote-apply",
            "appliedAt": 1,
            "offlineCapability": "forged"
        }))
        .is_err()
    );
}

fn feedback_evaluation() -> Evaluation {
    Evaluation {
        tier: VerifiabilityTier::Hard,
        success: false,
        details: "failure".into(),
        surprise: Some(1.0),
    }
}
