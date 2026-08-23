use std::path::PathBuf;

use spoon_adapt::{
    AdaptationPolicy, ApplyOutcome, AttributionStrength, Claim, ContradictionStatus,
    ContradictionStore, CorrectionAction, CorrectionApplier, CorrectionRequest, CorrectionTarget,
    DemonstratedFeature, EvidenceGate, GraphAlternativeSupport, Implication, KnowledgeRef,
    MutationAuthorizer, ReconciliationApplier, ReconciliationOutcome, ReconciliationPlanner,
    ScopeAssignment, StagedReconciliation, Uncertainty,
};
use spoon_core::{
    BinOp, Concept, Condition, ContextRelationship, Episode, EpisodeId, Evaluation, Expr,
    Lifecycle, MutabilityClass, ObservedFact, Param, Procedure, Relationship, Value,
    VerifiabilityTier,
};
use spoon_credit::{
    Attribution, AttributionConfidence, AttributionEvidence, AttributionMechanism,
    AttributionProvenance, ContractSection, CounterfactualMode, ReplayProvenance, Suspect,
};
use spoon_episode::{EpisodeFeedback, EpisodeStore, FeedbackSource};
use spoon_exec::{
    ConditionCheck, ConditionCheckStatus, ContractChecks, ExecStep, ExecStepStatus, ExecTrace,
};
use spoon_graph::KnowledgeStore;

fn evidence(episodes: u32, sources: u32, tier: VerifiabilityTier) -> EvidenceGate {
    EvidenceGate {
        verified_episodes: episodes,
        distinct_sources: sources,
        strongest_tier: Some(tier),
        challenger_beats_incumbent: false,
        corroborated: sources >= 2,
        offline: false,
    }
}

fn procedure(name: &str) -> Procedure {
    Procedure::new(name, vec![Param::named("x")], Expr::Var("x".to_owned()))
}

fn executable_scope(description: &str) -> Condition {
    Condition::described(description).with_check(Expr::Var("has_active_leavening".into()))
}

fn insert_verified_support_episode(
    episodes: &EpisodeStore,
    relationship: &Relationship,
    source: &Concept,
    target: &Concept,
    success: bool,
) -> EpisodeId {
    let mut episode = Episode::new("verified alternative support");
    episode.context.entities.extend([source.id, target.id]);
    episode
        .context
        .relevant_knowledge
        .push(ContextRelationship {
            relationship: relationship.clone(),
            discovered_from: source.id,
            adjacent_concept: target.clone(),
            hops: 1,
            relevance_score: 1.0,
        });
    episode.observed_result = Some(Value::Bool(success));
    episode.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success,
        details: "alternative produced the verified result".into(),
        surprise: (!success).then_some(1.0),
    });
    let id = episode.id;
    episodes.insert(&episode).unwrap();
    id
}

fn authorize_scope(
    decision: &spoon_adapt::CorrectionDecision,
    procedure: spoon_core::ProcedureId,
    version: u32,
    episode_id: EpisodeId,
) -> spoon_adapt::AuthorizedCorrection {
    let episodes = failed_episode_store(procedure, version, episode_id);
    episodes
        .insert(&successful_regression_episode(
            procedure,
            version,
            VerifiabilityTier::Hard,
            true,
        ))
        .unwrap();
    let attribution = contract_attribution(procedure, version, episode_id);
    MutationAuthorizer::authorize(&episodes, decision, &attribution).unwrap()
}

fn failed_episode_store(
    procedure: spoon_core::ProcedureId,
    version: u32,
    episode_id: EpisodeId,
) -> EpisodeStore {
    let episodes = EpisodeStore::in_memory().unwrap();
    let mut episode = Episode::new("verified failure");
    episode.id = episode_id;
    episode.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: false,
        details: "contract failure".into(),
        surprise: Some(1.0),
    });
    episode
        .context
        .environment
        .insert("has_active_leavening".into(), Value::Bool(false));
    episode.execution_trace = Some(
        serde_json::to_value(ExecTrace {
            steps: vec![ExecStep {
                expr_description: "verified call".into(),
                input: Some(Value::List(vec![Value::Bool(false)])),
                output: Value::Null,
                procedure_called: Some(procedure),
                procedure_version: Some(version),
                contract_checks: ContractChecks {
                    requires: vec![ConditionCheck {
                        description: "missing condition".into(),
                        status: ConditionCheckStatus::Violated,
                    }],
                    promises: Vec::new(),
                    fails_when: Vec::new(),
                },
                status: ExecStepStatus::Failed {
                    error: "contract violation".into(),
                },
            }],
        })
        .unwrap(),
    );
    episodes.insert(&episode).unwrap();
    episodes
}

fn successful_regression_episode(
    procedure: spoon_core::ProcedureId,
    version: u32,
    tier: VerifiabilityTier,
    admitted: bool,
) -> Episode {
    let mut episode = Episode::new("canonical successful regression");
    episode.evaluation = Some(Evaluation {
        tier,
        success: true,
        details: "verified regression".into(),
        surprise: None,
    });
    episode
        .context
        .environment
        .insert("has_active_leavening".into(), Value::Bool(admitted));
    episode.execution_trace = Some(
        serde_json::to_value(ExecTrace {
            steps: vec![ExecStep {
                expr_description: "verified call".into(),
                input: Some(Value::List(vec![Value::Bool(admitted)])),
                output: Value::Null,
                procedure_called: Some(procedure),
                procedure_version: Some(version),
                contract_checks: ContractChecks::default(),
                status: ExecStepStatus::Succeeded,
            }],
        })
        .unwrap(),
    );
    episode
}

fn contract_attribution(
    procedure: spoon_core::ProcedureId,
    version: u32,
    episode_id: EpisodeId,
) -> Attribution {
    Attribution {
        suspect: Suspect {
            procedure,
            version,
            trace_step: 0,
        },
        mechanism: AttributionMechanism::ContractViolation,
        confidence: AttributionConfidence::High,
        score: 0.95,
        decisive: false,
        evidence: vec![AttributionEvidence::Contract {
            section: ContractSection::Requires,
            description: "missing condition".into(),
            status: ConditionCheckStatus::Violated,
        }],
        limitations: vec![],
        provenance: AttributionProvenance {
            episode_ids: vec![episode_id],
            details: vec!["persisted contract violation".into()],
        },
        attribution_cost: 1.0,
        total_cost: 2.0,
        attribution_cost_ratio: 0.5,
    }
}

fn deterministic_replay_attribution(
    procedure: spoon_core::ProcedureId,
    version: u32,
    episode_id: EpisodeId,
) -> Attribution {
    Attribution {
        suspect: Suspect {
            procedure,
            version,
            trace_step: 0,
        },
        mechanism: AttributionMechanism::CounterfactualReplay,
        confidence: AttributionConfidence::High,
        score: 0.95,
        decisive: true,
        evidence: vec![AttributionEvidence::Replay {
            mode: CounterfactualMode::Deterministic,
            change_description: "exclude the failing input".into(),
            counterfactual_succeeded: Some(true),
            steps_used: 1,
            details: "replay succeeded".into(),
            provenance: ReplayProvenance::default(),
        }],
        limitations: vec![],
        provenance: AttributionProvenance {
            episode_ids: vec![episode_id],
            details: vec!["version-pinned replay".into()],
        },
        attribution_cost: 1.0,
        total_cost: 2.0,
        attribution_cost_ratio: 0.5,
    }
}

#[test]
fn statistical_suspicion_only_schedules_a_test() {
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::StatisticalSuspicion,
        target: CorrectionTarget::ProcedureScope {
            procedure_id: spoon_core::ProcedureId::new(),
            expected_version: 1,
            condition: executable_scope("batter contains leavening"),
            learned_from: EpisodeId::new(),
        },
        evidence: evidence(20, 4, VerifiabilityTier::Hard),
    });

    assert!(decision.action.is_schedule_test());
    assert!(decision.rationale.contains("statistical"));
}

#[test]
fn credit_attributions_map_without_upgrading_weak_or_statistical_evidence() {
    let procedure = spoon_core::ProcedureId::new();
    let attribution = |mechanism, confidence, decisive, evidence| Attribution {
        suspect: Suspect {
            procedure,
            version: 1,
            trace_step: 0,
        },
        mechanism,
        confidence,
        score: 1.0,
        decisive,
        evidence,
        limitations: vec![],
        provenance: AttributionProvenance::default(),
        attribution_cost: 1.0,
        total_cost: 2.0,
        attribution_cost_ratio: 0.5,
    };
    let statistical = attribution(
        AttributionMechanism::StatisticalSuspicion,
        AttributionConfidence::Certain,
        true,
        vec![],
    );
    let contract = attribution(
        AttributionMechanism::ContractViolation,
        AttributionConfidence::High,
        false,
        vec![],
    );
    let failed_replay = attribution(
        AttributionMechanism::CounterfactualReplay,
        AttributionConfidence::Inconclusive,
        false,
        vec![AttributionEvidence::Replay {
            mode: CounterfactualMode::Deterministic,
            change_description: "change one operation".into(),
            counterfactual_succeeded: Some(false),
            steps_used: 1,
            details: "still failed".into(),
            provenance: ReplayProvenance::default(),
        }],
    );
    let confirmed_replay = attribution(
        AttributionMechanism::CounterfactualReplay,
        AttributionConfidence::High,
        false,
        vec![AttributionEvidence::Replay {
            mode: CounterfactualMode::Simulated,
            change_description: "change one planning choice".into(),
            counterfactual_succeeded: Some(true),
            steps_used: 1,
            details: "succeeded".into(),
            provenance: ReplayProvenance::default(),
        }],
    );

    assert_eq!(
        AttributionStrength::from(&statistical),
        AttributionStrength::StatisticalSuspicion
    );
    assert_eq!(
        AttributionStrength::from(&contract),
        AttributionStrength::ContractViolation
    );
    assert_eq!(
        AttributionStrength::from(&failed_replay),
        AttributionStrength::InsufficientEvidence
    );
    assert_eq!(
        AttributionStrength::from(&confirmed_replay),
        AttributionStrength::SimulatedEvidence
    );
}

#[test]
fn assumption_failure_fixes_only_the_assumption() {
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ContractViolation,
        target: CorrectionTarget::Assumption {
            key: "oven_preheated".into(),
            replacement: Value::Bool(false),
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });

    let correction = decision.action.assumption_fix().expect("assumption fix");
    assert_eq!(correction.0, "oven_preheated");
    assert_eq!(correction.1, &Value::Bool(false));
    assert!(!decision.action.modifies_procedure());
}

#[test]
fn simulated_model_evidence_cannot_rewrite_even_non_graph_assumptions() {
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::SimulatedEvidence,
        target: CorrectionTarget::Assumption {
            key: "oven_preheated".into(),
            replacement: Value::Bool(false),
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });

    assert!(matches!(
        decision.action,
        CorrectionAction::ScheduleTest { .. }
    ));
}

#[test]
fn scope_narrowing_requires_strong_attribution_and_tier_one_or_two_evidence() {
    let procedure_id = spoon_core::ProcedureId::new();
    let episode = EpisodeId::new();
    let request = |tier| CorrectionRequest {
        attribution: AttributionStrength::ReplayConfirmed,
        target: CorrectionTarget::ProcedureScope {
            procedure_id,
            expected_version: 1,
            condition: executable_scope("batter contains active leavening"),
            learned_from: episode,
        },
        evidence: evidence(1, 1, tier),
    };

    assert!(
        AdaptationPolicy::decide(request(VerifiabilityTier::Hard))
            .action
            .is_narrow_scope()
    );
    assert!(
        AdaptationPolicy::decide(request(VerifiabilityTier::Consensus))
            .action
            .is_narrow_scope()
    );
    assert!(
        AdaptationPolicy::decide(request(VerifiabilityTier::Deferred))
            .action
            .is_schedule_test()
    );
}

#[test]
fn descriptive_only_scope_conditions_cannot_be_applied() {
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ContractViolation,
        target: CorrectionTarget::ProcedureScope {
            procedure_id: spoon_core::ProcedureId::new(),
            expected_version: 1,
            condition: Condition::described("batter contains active leavening"),
            learned_from: EpisodeId::new(),
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });

    assert!(decision.action.is_schedule_test());
    assert!(decision.rationale.contains("executable"));
}

#[test]
fn replacement_requires_several_verified_failures_and_a_winning_challenger() {
    let incumbent = procedure("scale pancakes");
    let mut challenger = incumbent.clone();
    challenger.body = Expr::Literal(Value::Text("corrected".into()));
    let request = |episodes, beats| CorrectionRequest {
        attribution: AttributionStrength::ReplayConfirmed,
        target: CorrectionTarget::ProcedureReplacement {
            incumbent_id: incumbent.id,
            incumbent_version: 1,
            challenger: Box::new(challenger.clone()),
        },
        evidence: EvidenceGate {
            challenger_beats_incumbent: beats,
            ..evidence(episodes, 2, VerifiabilityTier::Hard)
        },
    };

    assert!(
        AdaptationPolicy::decide(request(2, true))
            .action
            .is_schedule_test()
    );
    assert!(
        AdaptationPolicy::decide(request(3, false))
            .action
            .is_schedule_test()
    );
    assert!(
        AdaptationPolicy::decide(request(3, true))
            .action
            .is_replace_procedure()
    );
    let mut simulated = request(3, true);
    simulated.attribution = AttributionStrength::SimulatedEvidence;
    assert!(
        AdaptationPolicy::decide(simulated)
            .action
            .is_schedule_test()
    );
}

#[test]
fn concept_revision_is_highly_corroborated_and_offline_only() {
    let concept_id = spoon_core::ConceptId::new();
    let request = |episodes, sources, corroborated, offline| CorrectionRequest {
        attribution: AttributionStrength::ReplayConfirmed,
        target: CorrectionTarget::ConceptRevision {
            concept_id,
            expected_version: 1,
            revised_description: "Leavened batter rises under heat".into(),
        },
        evidence: EvidenceGate {
            corroborated,
            offline,
            ..evidence(episodes, sources, VerifiabilityTier::Consensus)
        },
    };

    assert!(
        AdaptationPolicy::decide(request(4, 2, true, true))
            .action
            .is_schedule_test()
    );
    assert!(
        AdaptationPolicy::decide(request(5, 1, true, true))
            .action
            .is_schedule_test()
    );
    assert!(
        AdaptationPolicy::decide(request(5, 2, true, false))
            .action
            .is_schedule_test()
    );
    assert!(
        AdaptationPolicy::decide(request(5, 2, true, true))
            .action
            .is_revise_concept()
    );
}

#[test]
fn applying_scope_narrowing_preserves_the_incumbent_version() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let incumbent = procedure("scale pancakes");
    graph.insert_procedure(&incumbent).unwrap();
    let learned_from = EpisodeId::new();
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ContractViolation,
        target: CorrectionTarget::ProcedureScope {
            procedure_id: incumbent.id,
            expected_version: 1,
            condition: executable_scope("batter contains active leavening"),
            learned_from,
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });

    let authorization = authorize_scope(&decision, incumbent.id, 1, learned_from);
    let outcome = CorrectionApplier::apply(&graph, &authorization, 200).unwrap();

    assert!(matches!(
        outcome,
        ApplyOutcome::ProcedureUpdated {
            previous_version: 1,
            current_version: 2,
            ..
        }
    ));
    let current = graph.get_procedure(incumbent.id).unwrap().unwrap();
    assert_eq!(current.version, 2);
    assert_eq!(current.contract.requires.len(), 1);
    assert_eq!(
        current.contract.requires[0].description,
        "batter contains active leavening"
    );
    assert!(current.contract.requires[0].check.is_some());
    assert_eq!(
        current.contract.confidence.scope[0].learned_from,
        Some(learned_from)
    );
    assert!(
        graph
            .get_procedure_version(incumbent.id, 1)
            .unwrap()
            .is_some()
    );
}

#[test]
fn stale_correction_decisions_fail_compare_and_swap() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let incumbent = procedure("scale pancakes");
    graph.insert_procedure(&incumbent).unwrap();
    let learned_from = EpisodeId::new();
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ContractViolation,
        target: CorrectionTarget::ProcedureScope {
            procedure_id: incumbent.id,
            expected_version: 1,
            condition: executable_scope("batter contains active leavening"),
            learned_from,
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });
    let mut concurrent_revision = incumbent.clone();
    concurrent_revision.version = 2;
    concurrent_revision
        .contract
        .promises
        .push(Condition::described("a separately reviewed promise"));
    graph.revise_procedure(&concurrent_revision, 1).unwrap();

    let authorization = authorize_scope(&decision, incumbent.id, 1, learned_from);
    let error = CorrectionApplier::apply(&graph, &authorization, 201).unwrap_err();

    assert!(error.to_string().contains("revision conflict"));
    let current = graph.get_procedure(incumbent.id).unwrap().unwrap();
    assert_eq!(current.version, 2);
    assert!(current.contract.requires.is_empty());
}

#[test]
fn forged_planning_confidence_cannot_authorize_mutation() {
    let episodes = EpisodeStore::in_memory().unwrap();
    let procedure_id = spoon_core::ProcedureId::new();
    let learned_from = EpisodeId::new();
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ReplayConfirmed,
        target: CorrectionTarget::ProcedureScope {
            procedure_id,
            expected_version: 1,
            condition: executable_scope("batter contains active leavening"),
            learned_from,
        },
        evidence: evidence(100, 100, VerifiabilityTier::Hard),
    });
    let forged = contract_attribution(procedure_id, 1, learned_from);

    let error = MutationAuthorizer::authorize(&episodes, &decision, &forged).unwrap_err();

    assert!(error.to_string().contains("episode"));
}

#[test]
fn forged_contract_attribution_without_a_violated_check_cannot_authorize_mutation() {
    let procedure_id = spoon_core::ProcedureId::new();
    let learned_from = EpisodeId::new();
    let episodes = failed_episode_store(procedure_id, 1, learned_from);
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ContractViolation,
        target: CorrectionTarget::ProcedureScope {
            procedure_id,
            expected_version: 1,
            condition: executable_scope("batter contains active leavening"),
            learned_from,
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });
    let mut forged = contract_attribution(procedure_id, 1, learned_from);
    forged.evidence = vec![AttributionEvidence::Contract {
        section: ContractSection::Requires,
        description: "check actually passed".into(),
        status: ConditionCheckStatus::Passed,
    }];

    let error = MutationAuthorizer::authorize(&episodes, &decision, &forged).unwrap_err();

    assert!(error.to_string().contains("violated check"));

    forged.evidence = vec![AttributionEvidence::Contract {
        section: ContractSection::Requires,
        description: "invented violation not present in the trace".into(),
        status: ConditionCheckStatus::Violated,
    }];
    let error = MutationAuthorizer::authorize(&episodes, &decision, &forged).unwrap_err();
    assert!(error.to_string().contains("stored episode trace"));
}

#[test]
fn scope_authorization_rejects_a_condition_that_admits_the_failed_trace_input() {
    let procedure_id = spoon_core::ProcedureId::new();
    let learned_from = EpisodeId::new();
    let episodes = failed_episode_store(procedure_id, 1, learned_from);
    episodes
        .insert(&successful_regression_episode(
            procedure_id,
            1,
            VerifiabilityTier::Hard,
            true,
        ))
        .unwrap();
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ContractViolation,
        target: CorrectionTarget::ProcedureScope {
            procedure_id,
            expected_version: 1,
            condition: Condition::described("always admitted")
                .with_check(Expr::Literal(Value::Bool(true))),
            learned_from,
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });

    let error = MutationAuthorizer::authorize(
        &episodes,
        &decision,
        &contract_attribution(procedure_id, 1, learned_from),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("does not exclude the failed trace input")
    );
}

#[test]
fn scope_authorization_requires_an_admitted_canonical_success_for_the_same_version() {
    let procedure_id = spoon_core::ProcedureId::new();
    let learned_from = EpisodeId::new();
    let episodes = failed_episode_store(procedure_id, 1, learned_from);
    episodes
        .insert(&successful_regression_episode(
            procedure_id,
            2,
            VerifiabilityTier::Hard,
            true,
        ))
        .unwrap();
    episodes
        .insert(&successful_regression_episode(
            procedure_id,
            1,
            VerifiabilityTier::Deferred,
            true,
        ))
        .unwrap();
    episodes
        .insert(&successful_regression_episode(
            procedure_id,
            1,
            VerifiabilityTier::Consensus,
            false,
        ))
        .unwrap();
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ContractViolation,
        target: CorrectionTarget::ProcedureScope {
            procedure_id,
            expected_version: 1,
            condition: executable_scope("admit only active leavening"),
            learned_from,
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });

    let error = MutationAuthorizer::authorize(
        &episodes,
        &decision,
        &contract_attribution(procedure_id, 1, learned_from),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("successful Hard or Consensus regression")
    );
}

#[test]
fn replay_evidence_cannot_authorize_scope_mutation_without_a_trusted_receipt() {
    let procedure_id = spoon_core::ProcedureId::new();
    let learned_from = EpisodeId::new();
    let episodes = failed_episode_store(procedure_id, 1, learned_from);
    episodes
        .insert(&successful_regression_episode(
            procedure_id,
            1,
            VerifiabilityTier::Hard,
            true,
        ))
        .unwrap();
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ReplayConfirmed,
        target: CorrectionTarget::ProcedureScope {
            procedure_id,
            expected_version: 1,
            condition: executable_scope("admit only active leavening"),
            learned_from,
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });

    let error = MutationAuthorizer::authorize(
        &episodes,
        &decision,
        &deterministic_replay_attribution(procedure_id, 1, learned_from),
    )
    .unwrap_err();

    assert!(error.to_string().contains("trusted replay receipt"));
}

#[test]
fn same_scope_description_is_idempotent_only_for_the_full_condition() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let mut incumbent = procedure("scale pancakes");
    let learned_from = EpisodeId::new();
    let proposed = executable_scope("batter contains active leavening");
    incumbent.contract.requires.push(proposed.clone());
    graph.insert_procedure(&incumbent).unwrap();
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ContractViolation,
        target: CorrectionTarget::ProcedureScope {
            procedure_id: incumbent.id,
            expected_version: 1,
            condition: proposed,
            learned_from,
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });
    let authorization = authorize_scope(&decision, incumbent.id, 1, learned_from);

    assert_eq!(
        CorrectionApplier::apply(&graph, &authorization, 202).unwrap(),
        ApplyOutcome::NoGraphChange
    );

    let conflicting =
        Condition::described("batter contains active leavening").with_check(Expr::BinOp {
            op: BinOp::Eq,
            left: Box::new(Expr::Var("has_active_leavening".into())),
            right: Box::new(Expr::Literal(Value::Bool(true))),
        });
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ContractViolation,
        target: CorrectionTarget::ProcedureScope {
            procedure_id: incumbent.id,
            expected_version: 1,
            condition: conflicting,
            learned_from,
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });
    let authorization = authorize_scope(&decision, incumbent.id, 1, learned_from);
    let error = CorrectionApplier::apply(&graph, &authorization, 203).unwrap_err();

    assert!(error.to_string().contains("condition description conflict"));
    assert_eq!(graph.current_procedure_version(incumbent.id).unwrap(), 1);
}

#[test]
fn learning_cannot_mutate_definitional_knowledge() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let concept = Concept::new("definition", MutabilityClass::Definitional);
    graph.insert_concept(&concept).unwrap();
    let incumbent = procedure("defined operation").with_concept(concept.id);
    graph.insert_procedure(&incumbent).unwrap();
    let learned_from = EpisodeId::new();
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ContractViolation,
        target: CorrectionTarget::ProcedureScope {
            procedure_id: incumbent.id,
            expected_version: 1,
            condition: executable_scope("new observed boundary"),
            learned_from,
        },
        evidence: evidence(1, 1, VerifiabilityTier::Hard),
    });
    let authorization = authorize_scope(&decision, incumbent.id, 1, learned_from);

    let error = CorrectionApplier::apply(&graph, &authorization, 202).unwrap_err();

    assert!(error.to_string().contains("Definitional"));
    assert_eq!(
        graph.get_procedure(incumbent.id).unwrap().unwrap().version,
        1
    );
}

#[test]
fn a_winning_challenger_still_requires_a_trusted_regression_capability() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let incumbent = procedure("scale pancakes");
    graph.insert_procedure(&incumbent).unwrap();
    let mut challenger = incumbent.clone();
    challenger.body = Expr::Literal(Value::Text("corrected".into()));
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ReplayConfirmed,
        target: CorrectionTarget::ProcedureReplacement {
            incumbent_id: incumbent.id,
            incumbent_version: 1,
            challenger: Box::new(challenger),
        },
        evidence: EvidenceGate {
            challenger_beats_incumbent: true,
            ..evidence(3, 2, VerifiabilityTier::Hard)
        },
    });

    let episodes = EpisodeStore::in_memory().unwrap();
    let attribution = contract_attribution(incumbent.id, 1, EpisodeId::new());
    let error = MutationAuthorizer::authorize(&episodes, &decision, &attribution).unwrap_err();

    assert!(error.to_string().contains("trusted offline capability"));
    assert_eq!(
        graph.get_procedure(incumbent.id).unwrap().unwrap().version,
        1
    );
}

#[test]
fn caller_offline_boolean_cannot_authorize_concept_revision() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let concept = Concept::new("leavening", MutabilityClass::DefeasibleGeneral);
    graph.insert_concept(&concept).unwrap();
    let decision = AdaptationPolicy::decide(CorrectionRequest {
        attribution: AttributionStrength::ReplayConfirmed,
        target: CorrectionTarget::ConceptRevision {
            concept_id: concept.id,
            expected_version: 1,
            revised_description: "Leavened batter rises under heat".into(),
        },
        evidence: EvidenceGate {
            corroborated: true,
            offline: true,
            ..evidence(5, 2, VerifiabilityTier::Consensus)
        },
    });

    let episodes = EpisodeStore::in_memory().unwrap();
    let attribution = contract_attribution(spoon_core::ProcedureId::new(), 1, EpisodeId::new());
    let error = MutationAuthorizer::authorize(&episodes, &decision, &attribution).unwrap_err();

    assert!(error.to_string().contains("quiescence capability"));
    assert_eq!(
        graph.get_concept(concept.id).unwrap().unwrap().description,
        None
    );
}

#[test]
fn reconciliation_preserves_alternatively_supported_dependents_and_stops_that_branch() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let base = Concept::new("base rule", MutabilityClass::DefeasibleGeneral);
    let middle = Concept::new("middle rule", MutabilityClass::DefeasibleGeneral);
    let leaf = Concept::new("leaf rule", MutabilityClass::DefeasibleGeneral);
    let alternative = Concept::new("alternative support", MutabilityClass::DefeasibleGeneral);
    for concept in [&base, &middle, &leaf, &alternative] {
        graph.insert_concept(concept).unwrap();
    }
    let dependency = Relationship::new(middle.id, base.id, "depends-on");
    graph.insert_relationship(&dependency).unwrap();
    graph
        .insert_relationship(&Relationship::new(leaf.id, middle.id, "depends-on"))
        .unwrap();
    let mut proof = Relationship::new(
        middle.id,
        alternative.id,
        format!("alternative-support:{}", base.id),
    );
    proof.lifecycle = Lifecycle::Validated;
    proof.evidence.push(insert_verified_support_episode(
        &episodes,
        &proof,
        &middle,
        &alternative,
        true,
    ));
    graph.insert_relationship(&proof).unwrap();

    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Concept(base.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();

    assert_eq!(plan.entries.len(), 2);
    let middle_entry = plan
        .entries
        .iter()
        .find(|entry| entry.knowledge == KnowledgeRef::Concept(middle.id))
        .unwrap();
    assert_eq!(
        middle_entry.outcome,
        ReconciliationOutcome::PreservedByAlternativeSupport
    );
    let staged = StagedReconciliation::new("preserved-middle-v1", plan, 300).unwrap();
    ReconciliationApplier::apply(&graph, &staged).unwrap();
    assert_eq!(
        graph.get_concept(middle.id).unwrap().unwrap().lifecycle,
        Lifecycle::Active
    );
    assert_eq!(
        graph.get_concept(leaf.id).unwrap().unwrap().lifecycle,
        Lifecycle::Active
    );
    assert_eq!(
        graph
            .get_relationship(dependency.id)
            .unwrap()
            .unwrap()
            .lifecycle,
        Lifecycle::UnderReview
    );
}

#[test]
fn forged_or_irrelevant_episode_ids_cannot_prove_alternative_support() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let base = Concept::new("base", MutabilityClass::DefeasibleGeneral);
    let dependent = Concept::new("dependent", MutabilityClass::DefeasibleGeneral);
    let alternative = Concept::new("alternative", MutabilityClass::DefeasibleGeneral);
    for concept in [&base, &dependent, &alternative] {
        graph.insert_concept(concept).unwrap();
    }
    graph
        .insert_relationship(&Relationship::new(dependent.id, base.id, "depends-on"))
        .unwrap();
    let mut unrelated_episode = Episode::new("unrelated verified success");
    unrelated_episode.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: true,
        details: "verified, but unrelated to either endpoint".into(),
        surprise: None,
    });
    episodes.insert(&unrelated_episode).unwrap();
    let mut deferred_episode = Episode::new("relevant but weak support");
    deferred_episode
        .context
        .entities
        .extend([dependent.id, alternative.id]);
    deferred_episode.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Deferred,
        success: true,
        details: "relevant human impression".into(),
        surprise: None,
    });
    episodes.insert(&deferred_episode).unwrap();
    let mut failed_episode = Episode::new("relevant verified counterexample");
    failed_episode
        .context
        .entities
        .extend([dependent.id, alternative.id]);
    failed_episode.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: false,
        details: "the proposed alternative failed".into(),
        surprise: Some(1.0),
    });
    episodes.insert(&failed_episode).unwrap();
    let mut forged_proof = Relationship::new(
        dependent.id,
        alternative.id,
        format!("alternative-support:{}", base.id),
    );
    forged_proof.lifecycle = Lifecycle::Validated;
    forged_proof.evidence = vec![
        EpisodeId::new(),
        unrelated_episode.id,
        deferred_episode.id,
        failed_episode.id,
    ];
    graph.insert_relationship(&forged_proof).unwrap();

    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Concept(base.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();
    let dependent_entry = plan
        .entries
        .iter()
        .find(|entry| entry.knowledge == KnowledgeRef::Concept(dependent.id))
        .unwrap();

    assert_eq!(
        dependent_entry.outcome,
        ReconciliationOutcome::MarkUnderReview
    );
}

#[test]
fn one_success_cannot_hide_claim_specific_counterevidence() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let base = Concept::new("base", MutabilityClass::DefeasibleGeneral);
    let dependent = Concept::new("dependent", MutabilityClass::DefeasibleGeneral);
    let alternative = Concept::new("alternative", MutabilityClass::DefeasibleGeneral);
    for concept in [&base, &dependent, &alternative] {
        graph.insert_concept(concept).unwrap();
    }
    graph
        .insert_relationship(&Relationship::new(dependent.id, base.id, "depends-on"))
        .unwrap();
    let mut proof = Relationship::new(
        dependent.id,
        alternative.id,
        format!("alternative-support:{}", base.id),
    );
    proof.lifecycle = Lifecycle::Validated;
    proof.evidence = vec![
        insert_verified_support_episode(&episodes, &proof, &dependent, &alternative, true),
        insert_verified_support_episode(&episodes, &proof, &dependent, &alternative, false),
    ];
    graph.insert_relationship(&proof).unwrap();

    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Concept(base.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();

    assert_eq!(
        plan.entries
            .iter()
            .find(|entry| entry.knowledge == KnowledgeRef::Concept(dependent.id))
            .unwrap()
            .outcome,
        ReconciliationOutcome::MarkUnderReview
    );
}

#[test]
fn conflicting_late_feedback_revokes_alternative_proof() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let base = Concept::new("base", MutabilityClass::DefeasibleGeneral);
    let dependent = Concept::new("dependent", MutabilityClass::DefeasibleGeneral);
    let alternative = Concept::new("alternative", MutabilityClass::DefeasibleGeneral);
    for concept in [&base, &dependent, &alternative] {
        graph.insert_concept(concept).unwrap();
    }
    graph
        .insert_relationship(&Relationship::new(dependent.id, base.id, "depends-on"))
        .unwrap();
    let mut proof = Relationship::new(
        dependent.id,
        alternative.id,
        format!("alternative-support:{}", base.id),
    );
    proof.lifecycle = Lifecycle::Validated;
    let evidence_id =
        insert_verified_support_episode(&episodes, &proof, &dependent, &alternative, true);
    episodes
        .append_feedback(&EpisodeFeedback::new(
            evidence_id,
            Value::Bool(false),
            Evaluation {
                tier: VerifiabilityTier::Hard,
                success: false,
                details: "independent counterexample".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("trusted_lab", Some("lab-b".into())),
            "alternative-counterexample",
        ))
        .unwrap();
    proof.evidence.push(evidence_id);
    graph.insert_relationship(&proof).unwrap();

    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Concept(base.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();

    assert_eq!(
        plan.entries
            .iter()
            .find(|entry| entry.knowledge == KnowledgeRef::Concept(dependent.id))
            .unwrap()
            .outcome,
        ReconciliationOutcome::MarkUnderReview
    );
}

#[test]
fn verified_concept_edge_can_explicitly_prove_a_procedure_fallback() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let caller_concept = Concept::new("recipe", MutabilityClass::DefeasibleGeneral);
    let fallback_concept = Concept::new("fallback", MutabilityClass::DefeasibleGeneral);
    graph.insert_concept(&caller_concept).unwrap();
    graph.insert_concept(&fallback_concept).unwrap();
    let changed = procedure("linear leavening");
    let mut caller = Procedure::new(
        "scale recipe",
        vec![],
        Expr::Call {
            procedure: changed.id,
            args: vec![Expr::Literal(Value::Int(1))],
        },
    );
    caller.concept = Some(caller_concept.id);
    let mut fallback = procedure("make two batches");
    fallback.concept = Some(fallback_concept.id);
    for procedure in [&changed, &caller, &fallback] {
        graph.insert_procedure(procedure).unwrap();
    }
    let mut proof = Relationship::new(
        caller_concept.id,
        fallback_concept.id,
        format!("alternative-support:{}", changed.id),
    );
    proof.lifecycle = Lifecycle::Validated;
    proof.evidence.push(insert_verified_support_episode(
        &episodes,
        &proof,
        &caller_concept,
        &fallback_concept,
        true,
    ));
    graph.insert_relationship(&proof).unwrap();

    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Procedure(changed.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();

    assert_eq!(plan.entries.len(), 1);
    assert_eq!(
        plan.entries[0].knowledge,
        KnowledgeRef::Procedure(caller.id)
    );
    assert_eq!(
        plan.entries[0].outcome,
        ReconciliationOutcome::PreservedByAlternativeSupport
    );
}

#[test]
fn invalid_alternative_knowledge_does_not_block_reconciliation() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let base = Concept::new("base", MutabilityClass::DefeasibleGeneral);
    let dependent = Concept::new("dependent", MutabilityClass::DefeasibleGeneral);
    let mut invalid = Concept::new("invalid alternative", MutabilityClass::DefeasibleGeneral);
    invalid.lifecycle = Lifecycle::Invalid;
    for concept in [&base, &dependent, &invalid] {
        graph.insert_concept(concept).unwrap();
    }
    graph
        .insert_relationship(&Relationship::new(dependent.id, base.id, "depends-on"))
        .unwrap();
    let mut invalid_proof = Relationship::new(
        dependent.id,
        invalid.id,
        format!("alternative-support:{}", base.id),
    );
    invalid_proof.lifecycle = Lifecycle::Validated;
    invalid_proof.evidence.push(EpisodeId::new());
    graph.insert_relationship(&invalid_proof).unwrap();

    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Concept(base.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();

    assert_eq!(
        plan.entries[0].outcome,
        ReconciliationOutcome::MarkUnderReview
    );
}

#[test]
fn reconciliation_marks_transitive_dependents_without_deletion_and_preserves_versions() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let callee = procedure("leaven batter");
    let caller = Procedure::new(
        "make pancakes",
        vec![],
        Expr::Call {
            procedure: callee.id,
            args: vec![],
        },
    );
    graph.insert_procedure(&callee).unwrap();
    graph.insert_procedure(&caller).unwrap();
    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Procedure(callee.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();

    assert_eq!(plan.entries.len(), 1);
    assert_eq!(
        plan.entries[0].outcome,
        ReconciliationOutcome::MarkUnderReview
    );
    let staged = StagedReconciliation::new("review-caller-v1", plan, 400).unwrap();
    let result = ReconciliationApplier::apply(&graph, &staged).unwrap();

    assert_eq!(result.updated, vec![KnowledgeRef::Procedure(caller.id)]);
    let current = graph.get_procedure(caller.id).unwrap().unwrap();
    assert_eq!(current.lifecycle, Lifecycle::UnderReview);
    assert_eq!(current.version, 2);
    let historical = graph.get_procedure_version(caller.id, 1).unwrap().unwrap();
    assert_eq!(historical.lifecycle, Lifecycle::Active);
    assert!(graph.get_procedure(callee.id).unwrap().is_some());
}

#[test]
fn stale_reconciliation_plans_fail_compare_and_swap() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let callee = procedure("leaven batter");
    let caller = Procedure::new(
        "make pancakes",
        vec![],
        Expr::Call {
            procedure: callee.id,
            args: vec![],
        },
    );
    graph.insert_procedure(&callee).unwrap();
    graph.insert_procedure(&caller).unwrap();
    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Procedure(callee.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();
    let mut concurrent_revision = caller.clone();
    concurrent_revision.version = 2;
    concurrent_revision
        .contract
        .promises
        .push(Condition::described("independently reviewed"));
    graph.revise_procedure(&concurrent_revision, 1).unwrap();

    let staged = StagedReconciliation::new("stale-caller-v1", plan, 401).unwrap();
    let error = ReconciliationApplier::apply(&graph, &staged).unwrap_err();

    assert!(error.to_string().contains("revision conflict"));
    let current = graph.get_procedure(caller.id).unwrap().unwrap();
    assert_eq!(current.version, 2);
    assert_eq!(current.lifecycle, Lifecycle::Active);
}

#[test]
fn reconciliation_versions_incident_relationship_claims_without_deleting_history() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let base = Concept::new("base", MutabilityClass::DefeasibleGeneral);
    let dependent = Concept::new("dependent", MutabilityClass::DefeasibleGeneral);
    graph.insert_concept(&base).unwrap();
    graph.insert_concept(&dependent).unwrap();
    let dependency = Relationship::new(dependent.id, base.id, "depends-on");
    graph.insert_relationship(&dependency).unwrap();
    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Concept(base.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();
    assert!(plan.entries.iter().any(|entry| {
        entry.knowledge == KnowledgeRef::Relationship(dependency.id)
            && entry.expected_version == 1
            && entry.outcome == ReconciliationOutcome::MarkUnderReview
    }));
    let staged = StagedReconciliation::new("relationship-reconcile-v1", plan, 425).unwrap();

    let result = ReconciliationApplier::apply(&graph, &staged).unwrap();

    assert!(
        result
            .updated
            .contains(&KnowledgeRef::Relationship(dependency.id))
    );
    assert_eq!(
        graph.current_relationship_version(dependency.id).unwrap(),
        2
    );
    assert_eq!(
        graph
            .get_relationship(dependency.id)
            .unwrap()
            .unwrap()
            .lifecycle,
        Lifecycle::UnderReview
    );
    assert_eq!(
        graph
            .get_relationship_version(dependency.id, 1)
            .unwrap()
            .unwrap()
            .lifecycle,
        Lifecycle::Active
    );
}

#[test]
fn multi_entity_reconciliation_is_atomic_and_idempotently_resumable() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let base = Concept::new("base", MutabilityClass::DefeasibleGeneral);
    let direct = Concept::new("direct", MutabilityClass::DefeasibleGeneral);
    let transitive = Concept::new("transitive", MutabilityClass::DefeasibleGeneral);
    for concept in [&base, &direct, &transitive] {
        graph.insert_concept(concept).unwrap();
    }
    let direct_edge = Relationship::new(direct.id, base.id, "depends-on");
    let transitive_edge = Relationship::new(transitive.id, direct.id, "depends-on");
    graph.insert_relationship(&direct_edge).unwrap();
    graph.insert_relationship(&transitive_edge).unwrap();

    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Concept(base.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();

    assert_eq!(plan.entries.len(), 4);
    assert!(
        plan.entries
            .iter()
            .all(|entry| entry.outcome == ReconciliationOutcome::MarkUnderReview)
    );
    let stage = StagedReconciliation::new("reconcile-base-v1", plan, 450).unwrap();
    assert_eq!(stage.remaining(&graph).unwrap().len(), 4);
    let first = ReconciliationApplier::apply(&graph, &stage).unwrap();
    let retried = ReconciliationApplier::apply(&graph, &stage).unwrap();

    assert_eq!(first, retried);
    assert_eq!(first.updated.len(), 4);
    assert!(first.receipt.is_some());
    assert!(stage.remaining(&graph).unwrap().is_empty());
    assert_eq!(
        graph.get_concept(direct.id).unwrap().unwrap().lifecycle,
        Lifecycle::UnderReview
    );
    assert_eq!(
        graph.get_concept(transitive.id).unwrap().unwrap().lifecycle,
        Lifecycle::UnderReview
    );
    assert_eq!(graph.current_concept_version(direct.id).unwrap(), 2);
    assert_eq!(graph.current_concept_version(transitive.id).unwrap(), 2);
    assert_eq!(
        graph.current_relationship_version(direct_edge.id).unwrap(),
        2
    );
    assert_eq!(
        graph
            .current_relationship_version(transitive_edge.id)
            .unwrap(),
        2
    );

    let recovered: StagedReconciliation =
        serde_json::from_str(&serde_json::to_string(&stage).unwrap()).unwrap();
    assert!(recovered.is_applied(&graph).unwrap());
    assert_eq!(recovered.receipt(&graph).unwrap(), first.receipt);
    let conflicting =
        StagedReconciliation::new("reconcile-base-v1", recovered.plan().clone(), 999).unwrap();
    assert!(conflicting.is_applied(&graph).is_err());
}

#[test]
fn all_preserved_reconciliation_still_binds_its_idempotency_key_durably() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let changed = Concept::new("standalone", MutabilityClass::DefeasibleGeneral);
    graph.insert_concept(&changed).unwrap();
    let stage = StagedReconciliation::new(
        "no-op-reconciliation-1",
        spoon_adapt::ReconciliationPlan {
            changed: KnowledgeRef::Concept(changed.id),
            entries: Vec::new(),
        },
        500,
    )
    .unwrap();

    let first = ReconciliationApplier::apply(&graph, &stage).unwrap();
    let retry = ReconciliationApplier::apply(&graph, &stage).unwrap();

    assert_eq!(first, retry);
    assert!(first.receipt.is_some());
    assert!(stage.is_applied(&graph).unwrap());
    assert!(stage.receipt(&graph).unwrap().is_some());

    let conflicting =
        StagedReconciliation::new("no-op-reconciliation-1", stage.plan().clone(), 501).unwrap();
    assert!(ReconciliationApplier::apply(&graph, &conflicting).is_err());
}

#[test]
fn stale_multi_entity_reconciliation_rolls_back_every_planned_change() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let base = Concept::new("base", MutabilityClass::DefeasibleGeneral);
    let direct = Concept::new("direct", MutabilityClass::DefeasibleGeneral);
    let transitive = Concept::new("transitive", MutabilityClass::DefeasibleGeneral);
    for concept in [&base, &direct, &transitive] {
        graph.insert_concept(concept).unwrap();
    }
    let direct_edge = Relationship::new(direct.id, base.id, "depends-on");
    let transitive_edge = Relationship::new(transitive.id, direct.id, "depends-on");
    graph.insert_relationship(&direct_edge).unwrap();
    graph.insert_relationship(&transitive_edge).unwrap();
    let plan = ReconciliationPlanner::plan(
        &graph,
        KnowledgeRef::Concept(base.id),
        &GraphAlternativeSupport::new(&episodes),
    )
    .unwrap();
    let staged = StagedReconciliation::new("stale-atomic-base-v1", plan, 451).unwrap();
    let mut concurrent = direct.clone();
    concurrent.description = Some("reviewed concurrently".into());
    concurrent.updated_at = 450;
    graph.revise_concept(&concurrent, 1).unwrap();

    let error = ReconciliationApplier::apply(&graph, &staged).unwrap_err();

    assert!(error.to_string().contains("revision conflict"));
    assert_eq!(graph.current_concept_version(direct.id).unwrap(), 2);
    assert_eq!(graph.current_concept_version(transitive.id).unwrap(), 1);
    assert_eq!(
        graph.current_relationship_version(direct_edge.id).unwrap(),
        1
    );
    assert_eq!(
        graph
            .current_relationship_version(transitive_edge.id)
            .unwrap(),
        1
    );
    assert_eq!(
        graph.get_concept(transitive.id).unwrap().unwrap().lifecycle,
        Lifecycle::Active
    );
    assert!(
        graph
            .get_change_receipt("stale-atomic-base-v1")
            .unwrap()
            .is_none()
    );
}

#[test]
fn unresolved_contradictions_persist_and_propagate_uncertainty() {
    let path = temporary_database("held");
    let left_episode = EpisodeId::new();
    let right_episode = EpisodeId::new();
    let episodes = claim_evidence_episodes(
        "pancakes-rise",
        left_episode,
        Value::Bool(true),
        right_episode,
        Value::Bool(false),
    );
    let id = {
        let store = ContradictionStore::open(path.to_str().unwrap()).unwrap();
        store
            .record(
                Claim::new(
                    "claim-a",
                    "Pancakes rise",
                    Implication::new("pancakes-rise", Value::Bool(true)),
                    vec![left_episode],
                ),
                Claim::new(
                    "claim-b",
                    "Pancakes do not rise",
                    Implication::new("pancakes-rise", Value::Bool(false)),
                    vec![right_episode],
                ),
                &episodes,
                500,
            )
            .unwrap()
            .id
    };
    let reopened = ContradictionStore::open(path.to_str().unwrap()).unwrap();
    reopened
        .add_claim_dependency("recipe-plan", "claim-a")
        .unwrap();

    assert_eq!(
        reopened.get(id).unwrap().unwrap().status,
        ContradictionStatus::Held
    );
    assert_eq!(reopened.list_held().unwrap().len(), 1);
    assert_eq!(
        reopened.uncertainty_for_claim("claim-a").unwrap(),
        Uncertainty::HeldContradictions(vec![id])
    );
    assert_eq!(
        reopened.uncertainty_for_claim("recipe-plan").unwrap(),
        Uncertainty::HeldContradictions(vec![id])
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn demonstrated_discriminator_splits_both_claims_without_destroying_history() {
    let store = ContradictionStore::in_memory().unwrap();
    let left_episode = EpisodeId::new();
    let right_episode = EpisodeId::new();
    let episodes = discriminator_episodes(
        "active-leavening",
        left_episode,
        Value::Bool(true),
        Value::Bool(true),
        right_episode,
        Value::Bool(false),
        Value::Bool(false),
    );
    let contradiction = store
        .record(
            Claim::new(
                "claim-a",
                "Pancakes rise",
                Implication::new("pancakes-rise", Value::Bool(true)),
                vec![left_episode],
            ),
            Claim::new(
                "claim-b",
                "Pancakes do not rise",
                Implication::new("pancakes-rise", Value::Bool(false)),
                vec![right_episode],
            ),
            &episodes,
            600,
        )
        .unwrap();
    let feature = DemonstratedFeature::new(
        "active-leavening",
        Value::Bool(true),
        left_episode,
        Value::Bool(false),
        right_episode,
    )
    .unwrap();
    let refinement = store
        .refine(contradiction.id, feature, &episodes, 601)
        .unwrap();

    assert_eq!(refinement.left.scope[0].feature, "active-leavening");
    assert_eq!(refinement.left.scope[0].value, Value::Bool(true));
    assert_eq!(refinement.right.scope[0].value, Value::Bool(false));
    assert_eq!(
        store.get(contradiction.id).unwrap().unwrap().status,
        ContradictionStatus::Refined
    );
    assert!(matches!(
        store.uncertainty_for_claim("claim-a").unwrap(),
        Uncertainty::Certain
    ));
}

#[test]
fn refinement_rechecks_that_discriminator_episodes_demonstrate_each_claim() {
    let store = ContradictionStore::in_memory().unwrap();
    let left_episode = EpisodeId::new();
    let right_episode = EpisodeId::new();
    let canonical = claim_evidence_episodes(
        "pancakes-rise",
        left_episode,
        Value::Bool(true),
        right_episode,
        Value::Bool(false),
    );
    let contradiction = store
        .record(
            Claim::new(
                "claim-a",
                "Pancakes rise",
                Implication::new("pancakes-rise", Value::Bool(true)),
                vec![left_episode],
            ),
            Claim::new(
                "claim-b",
                "Pancakes do not rise",
                Implication::new("pancakes-rise", Value::Bool(false)),
                vec![right_episode],
            ),
            &canonical,
            650,
        )
        .unwrap();
    let substituted = discriminator_episodes(
        "active-leavening",
        left_episode,
        Value::Bool(true),
        Value::Bool(false),
        right_episode,
        Value::Bool(false),
        Value::Bool(true),
    );

    let error = store
        .refine(
            contradiction.id,
            DemonstratedFeature::new(
                "active-leavening",
                Value::Bool(true),
                left_episode,
                Value::Bool(false),
                right_episode,
            )
            .unwrap(),
            &substituted,
            651,
        )
        .unwrap_err();

    assert!(error.to_string().contains("observed predicate"));
    assert_eq!(
        store.get(contradiction.id).unwrap().unwrap().status,
        ContradictionStatus::Held
    );
}

#[test]
fn non_conflicts_and_unsupported_discriminators_are_rejected() {
    let store = ContradictionStore::in_memory().unwrap();
    let episode = EpisodeId::new();
    let left = Claim::new(
        "left",
        "Pancakes rise",
        Implication::new("pancakes-rise", Value::Bool(true)),
        vec![episode],
    );
    let compatible = Claim::new(
        "same",
        "Pancakes also rise",
        Implication::new("pancakes-rise", Value::Bool(true)),
        vec![EpisodeId::new()],
    );
    let compatible_episodes = claim_evidence_episodes(
        "pancakes-rise",
        episode,
        Value::Bool(true),
        compatible.supporting_episodes[0],
        Value::Bool(true),
    );
    assert!(
        store
            .record(left.clone(), compatible, &compatible_episodes, 700)
            .is_err()
    );

    let right_episode = EpisodeId::new();
    let supporting_episodes = claim_evidence_episodes(
        "pancakes-rise",
        episode,
        Value::Bool(true),
        right_episode,
        Value::Bool(false),
    );
    let contradiction = store
        .record(
            left,
            Claim::new(
                "right",
                "Pancakes do not rise",
                Implication::new("pancakes-rise", Value::Bool(false)),
                vec![right_episode],
            ),
            &supporting_episodes,
            701,
        )
        .unwrap();
    let unrelated_episode = EpisodeId::new();
    let feature = DemonstratedFeature::new(
        "active-leavening",
        Value::Bool(true),
        unrelated_episode,
        Value::Bool(false),
        right_episode,
    )
    .unwrap();
    let episodes = discriminator_episodes(
        "active-leavening",
        unrelated_episode,
        Value::Bool(true),
        Value::Bool(true),
        right_episode,
        Value::Bool(false),
        Value::Bool(false),
    );
    assert!(
        store
            .refine(contradiction.id, feature, &episodes, 702)
            .is_err()
    );
    assert!(
        DemonstratedFeature::new(
            "same-value",
            Value::Bool(true),
            episode,
            Value::Bool(true),
            right_episode,
        )
        .is_err()
    );
}

#[test]
fn contradiction_recording_is_canonically_idempotent() {
    let store = ContradictionStore::in_memory().unwrap();
    let left = Claim::new(
        "claim-a",
        "Pancakes rise",
        Implication::new("pancakes-rise", Value::Bool(true)),
        vec![EpisodeId::new()],
    );
    let right = Claim::new(
        "claim-b",
        "Pancakes do not rise",
        Implication::new("pancakes-rise", Value::Bool(false)),
        vec![EpisodeId::new()],
    );
    let episodes = claim_evidence_episodes(
        "pancakes-rise",
        left.supporting_episodes[0],
        Value::Bool(true),
        right.supporting_episodes[0],
        Value::Bool(false),
    );

    let first = store
        .record(left.clone(), right.clone(), &episodes, 900)
        .unwrap();
    let duplicate = store.record(right, left, &episodes, 901).unwrap();

    assert_eq!(first.id, duplicate.id);
    assert_eq!(store.list_held().unwrap().len(), 1);
}

#[test]
fn contradiction_recording_requires_canonical_verified_observations() {
    let left_episode = EpisodeId::new();
    let right_episode = EpisodeId::new();
    let left = Claim::new(
        "verified-left",
        "Pancakes rise",
        Implication::new("pancakes-rise", Value::Bool(true)),
        vec![left_episode],
    );
    let right = Claim::new(
        "verified-right",
        "Pancakes do not rise",
        Implication::new("pancakes-rise", Value::Bool(false)),
        vec![right_episode],
    );

    let missing = EpisodeStore::in_memory().unwrap();
    let error = ContradictionStore::in_memory()
        .unwrap()
        .record(left.clone(), right.clone(), &missing, 910)
        .unwrap_err();
    assert!(error.to_string().contains("episode"));

    let failed = EpisodeStore::in_memory().unwrap();
    insert_claim_episode(
        &failed,
        left_episode,
        "pancakes-rise",
        Value::Bool(true),
        VerifiabilityTier::Hard,
        false,
    );
    insert_claim_episode(
        &failed,
        right_episode,
        "pancakes-rise",
        Value::Bool(false),
        VerifiabilityTier::Consensus,
        true,
    );
    let error = ContradictionStore::in_memory()
        .unwrap()
        .record(left.clone(), right.clone(), &failed, 911)
        .unwrap_err();
    assert!(error.to_string().contains("successful Hard or Consensus"));

    let deferred = EpisodeStore::in_memory().unwrap();
    insert_claim_episode(
        &deferred,
        left_episode,
        "pancakes-rise",
        Value::Bool(true),
        VerifiabilityTier::Deferred,
        true,
    );
    insert_claim_episode(
        &deferred,
        right_episode,
        "pancakes-rise",
        Value::Bool(false),
        VerifiabilityTier::Hard,
        true,
    );
    let error = ContradictionStore::in_memory()
        .unwrap()
        .record(left.clone(), right.clone(), &deferred, 912)
        .unwrap_err();
    assert!(error.to_string().contains("successful Hard or Consensus"));

    let unrelated = claim_evidence_episodes(
        "different-predicate",
        left_episode,
        Value::Bool(false),
        right_episode,
        Value::Bool(true),
    );
    let error = ContradictionStore::in_memory()
        .unwrap()
        .record(left, right, &unrelated, 913)
        .unwrap_err();
    assert!(error.to_string().contains("observed predicate"));
}

#[test]
fn scoped_refinements_remain_queryable_after_reopen() {
    let path = temporary_database("refinements");
    let left_episode = EpisodeId::new();
    let right_episode = EpisodeId::new();
    let episodes = discriminator_episodes(
        "active-leavening",
        left_episode,
        Value::Bool(true),
        Value::Bool(true),
        right_episode,
        Value::Bool(false),
        Value::Bool(false),
    );
    let expected = {
        let store = ContradictionStore::open(path.to_str().unwrap()).unwrap();
        store
            .add_claim_dependency("recipe-plan", "claim-a")
            .unwrap();
        let contradiction = store
            .record(
                Claim::new(
                    "claim-a",
                    "Pancakes rise",
                    Implication::new("pancakes-rise", Value::Bool(true)),
                    vec![left_episode],
                ),
                Claim::new(
                    "claim-b",
                    "Pancakes do not rise",
                    Implication::new("pancakes-rise", Value::Bool(false)),
                    vec![right_episode],
                ),
                &episodes,
                920,
            )
            .unwrap();
        let refinement = store
            .refine(
                contradiction.id,
                DemonstratedFeature::new(
                    "active-leavening",
                    Value::Bool(true),
                    left_episode,
                    Value::Bool(false),
                    right_episode,
                )
                .unwrap(),
                &episodes,
                921,
            )
            .unwrap();
        let duplicate = store
            .record(
                Claim::new(
                    "claim-b",
                    "Pancakes do not rise",
                    Implication::new("pancakes-rise", Value::Bool(false)),
                    vec![right_episode],
                ),
                Claim::new(
                    "claim-a",
                    "Pancakes rise",
                    Implication::new("pancakes-rise", Value::Bool(true)),
                    vec![left_episode],
                ),
                &episodes,
                922,
            )
            .unwrap();
        assert_eq!(duplicate.status, ContradictionStatus::Refined);
        assert_eq!(duplicate.created_at, 920);
        assert_eq!(duplicate.updated_at, 921);
        assert_eq!(duplicate.refinement, Some(refinement.clone()));
        assert!(duplicate.left.scope.is_empty());
        assert!(duplicate.right.scope.is_empty());
        refinement
    };
    let reopened = ContradictionStore::open(path.to_str().unwrap()).unwrap();

    assert_eq!(
        reopened.refinements_for_claim("claim-a").unwrap(),
        vec![expected.clone()]
    );
    assert_eq!(
        reopened.refinements_for_claim("claim-b").unwrap(),
        vec![expected]
    );
    assert_eq!(
        reopened.refinements_for_claim("recipe-plan").unwrap(),
        reopened.refinements_for_claim("claim-a").unwrap()
    );
    assert!(
        reopened
            .refinements_for_claim("unrelated")
            .unwrap()
            .is_empty()
    );
    std::fs::remove_file(path).unwrap();
}

fn claim_evidence_episodes(
    predicate: &str,
    left_id: EpisodeId,
    left_value: Value,
    right_id: EpisodeId,
    right_value: Value,
) -> EpisodeStore {
    let store = EpisodeStore::in_memory().unwrap();
    insert_claim_episode(
        &store,
        left_id,
        predicate,
        left_value,
        VerifiabilityTier::Hard,
        true,
    );
    insert_claim_episode(
        &store,
        right_id,
        predicate,
        right_value,
        VerifiabilityTier::Consensus,
        true,
    );
    store
}

fn insert_claim_episode(
    store: &EpisodeStore,
    id: EpisodeId,
    predicate: &str,
    observed_result: Value,
    tier: VerifiabilityTier,
    success: bool,
) {
    let mut episode = Episode::new("claim evidence");
    episode.id = id;
    episode.observed_result = Some(observed_result.clone());
    episode.observed_facts.push(ObservedFact::new(
        predicate,
        observed_result,
        Default::default(),
    ));
    episode.evaluation = Some(Evaluation {
        tier,
        success,
        details: "claim observed".into(),
        surprise: None,
    });
    store.insert(&episode).unwrap();
}

fn discriminator_episodes(
    feature: &str,
    left_id: EpisodeId,
    left_feature_value: Value,
    left_observed_value: Value,
    right_id: EpisodeId,
    right_feature_value: Value,
    right_observed_value: Value,
) -> EpisodeStore {
    let store = EpisodeStore::in_memory().unwrap();
    for (id, feature_value, observed_value) in [
        (left_id, left_feature_value, left_observed_value),
        (right_id, right_feature_value, right_observed_value),
    ] {
        let mut episode = Episode::new("discriminator evidence");
        episode.id = id;
        episode
            .context
            .environment
            .insert(feature.into(), feature_value);
        episode.observed_result = Some(observed_value.clone());
        episode.observed_facts.push(ObservedFact::new(
            "pancakes-rise",
            observed_value,
            episode.context.environment.clone(),
        ));
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "feature observed".into(),
            surprise: None,
        });
        store.insert(&episode).unwrap();
    }
    store
}

#[test]
fn conflicting_implications_in_already_disjoint_scopes_are_not_a_contradiction() {
    let store = ContradictionStore::in_memory().unwrap();
    let left_episode = EpisodeId::new();
    let right_episode = EpisodeId::new();
    let mut left = Claim::new(
        "with-leavening",
        "Pancakes rise",
        Implication::new("pancakes-rise", Value::Bool(true)),
        vec![left_episode],
    );
    left.scope.push(ScopeAssignment {
        feature: "active-leavening".into(),
        value: Value::Bool(true),
        learned_from: left_episode,
    });
    let mut right = Claim::new(
        "without-leavening",
        "Pancakes do not rise",
        Implication::new("pancakes-rise", Value::Bool(false)),
        vec![right_episode],
    );
    right.scope.push(ScopeAssignment {
        feature: "active-leavening".into(),
        value: Value::Bool(false),
        learned_from: right_episode,
    });
    let episodes = claim_evidence_episodes(
        "pancakes-rise",
        left_episode,
        Value::Bool(true),
        right_episode,
        Value::Bool(false),
    );

    assert!(store.record(left, right, &episodes, 800).is_err());
    assert!(store.list_held().unwrap().is_empty());
}

fn temporary_database(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "spoon-adapt-{label}-{}-{unique}.db",
        std::process::id()
    ))
}
