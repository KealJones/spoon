use std::collections::BTreeMap;

use ekg_core::{
    BinOp, Condition, Contract, Episode, Evaluation, Expr, Lifecycle, Param, Procedure, Value,
    VerifiabilityTier,
};
use ekg_credit::{
    AttributionConfidence, AttributionEvidence, BudgetStopReason, CounterfactualCandidate,
    CounterfactualChange, CounterfactualMode, CounterfactualReplayer, ReplayOutcome, ReplayRequest,
    Suspect,
};
use ekg_engine::{
    CounterfactualMutation, Engine, EngineError, FailureAnalysisBudget, FailureAnalysisRequest,
    FailureEvidenceSource, PHASE2_MAX_ATTRIBUTION_COST_RATIO, ProcedureVersionRef,
    ReplayVerification, SimulatedReplayModel, SimulatedReplayObservation, SimulatedReplayRequest,
};
use ekg_episode::{EpisodeFeedback, FeedbackSource};
use ekg_exec::{ExecStepStatus, ExecTrace};
use serde_json::json;

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

fn failed_double(engine: &Engine, procedure: &Procedure) -> Episode {
    engine
        .execute_procedure(procedure.id, inputs(7), Some(Value::Int(15)))
        .unwrap()
        .episode
}

fn mutation_candidate(
    procedure: &Procedure,
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
            trace_step: 0,
        },
        prior_score: 0.8,
        change: CounterfactualChange {
            description: "replace multiplier".into(),
            replacement: serde_json::to_value(mutation).unwrap(),
        },
        mode,
    }
}

fn triple_body() -> Expr {
    Expr::BinOp {
        op: BinOp::Mul,
        left: Box::new(Expr::Var("x".into())),
        right: Box::new(Expr::Literal(Value::Int(3))),
    }
}

#[test]
fn current_execution_rejects_inactive_lifecycles_but_historical_replay_still_works() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let original = engine
        .execute_procedure(procedure.id, inputs(7), Some(Value::Int(14)))
        .unwrap();

    for lifecycle in [
        Lifecycle::Stale,
        Lifecycle::UnderReview,
        Lifecycle::Superseded,
        Lifecycle::Retired,
        Lifecycle::Invalid,
    ] {
        procedure.version += 1;
        procedure.lifecycle = lifecycle;
        engine.admin_update_procedure(&procedure).unwrap();

        let error = engine
            .execute_procedure(procedure.id, inputs(8), None)
            .unwrap_err();
        assert!(error.to_string().contains("not executable"));
    }

    let replayed = engine
        .replay_episode(original.episode.id, inputs(9))
        .unwrap();
    assert_eq!(replayed.value, Value::Int(18));
    assert_eq!(replayed.trace.steps[0].procedure_version, Some(1));
}

#[test]
fn deterministic_analysis_composes_credit_mechanisms_and_reports_raw_cost() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = engine
        .execute_procedure(procedure.id, inputs(7), Some(Value::Int(21)))
        .unwrap()
        .episode;
    let candidate = mutation_candidate(
        &procedure,
        triple_body(),
        ReplayVerification::DeterministicExpected {
            expected: Value::Int(21),
        },
        CounterfactualMode::Deterministic,
    );

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![candidate],
            budget: FailureAnalysisBudget {
                top_k: 1,
                max_replays: 1,
                max_replay_steps: 100,
            },
        })
        .unwrap();

    assert_eq!(analysis.counterfactual.replays_run, 1);
    assert_eq!(analysis.counterfactual.attributions.len(), 1);
    assert_eq!(
        analysis.counterfactual.attributions[0].confidence,
        AttributionConfidence::Certain
    );
    assert!(analysis.counterfactual.attributions[0].decisive);
    let AttributionEvidence::Replay { provenance, .. } =
        &analysis.counterfactual.attributions[0].evidence[0]
    else {
        panic!("expected replay evidence");
    };
    assert!(provenance.source_trace_hash.is_some());
    assert!(provenance.mutation_hash.is_some());
    assert!(provenance.verification.is_some());
    let AttributionEvidence::Replay { details, .. } =
        &analysis.counterfactual.attributions[0].evidence[0]
    else {
        unreachable!()
    };
    assert!(details.contains("caller expected 21 was ignored"));
    assert!(details.contains("canonical precommitted oracle 21"));
    assert_eq!(analysis.cost.statistical_episodes_scanned, 0);
    assert_eq!(analysis.cost.analysis_cache_lookups, 1);
    assert_eq!(analysis.cost.evidence_digest_source_episode_reads, 1);
    assert_eq!(analysis.cost.evidence_digest_history_episodes_scanned, 0);
    assert_eq!(analysis.cost.evidence_digest_feedback_rows_scanned, 0);
    assert_eq!(analysis.cost.evidence_digest_trace_steps_scanned, 1);
    assert_eq!(analysis.cost.evidence_digest_procedure_snapshots_read, 1);
    assert_eq!(analysis.cost.evidence_digest_element_aggregate_rows_read, 1);
    assert_eq!(analysis.cost.evidence_digest_pair_aggregate_rows_read, 0);
    assert_eq!(
        analysis.cost.evidence_digest_work_units,
        analysis.cost.evidence_digest_source_episode_reads
            + analysis.cost.evidence_digest_history_episodes_scanned
            + analysis.cost.evidence_digest_feedback_rows_scanned
            + analysis.cost.evidence_digest_trace_steps_scanned
            + analysis.cost.evidence_digest_procedure_snapshots_read
            + analysis.cost.evidence_digest_element_aggregate_rows_read
            + analysis.cost.evidence_digest_pair_aggregate_rows_read
    );
    assert!(analysis.cost.contract_steps > 0);
    assert!(analysis.cost.replay_steps > 0);
    assert_eq!(
        analysis.cost.total_cost,
        analysis.cost.original_execution_cost + analysis.cost.attribution_cost
    );
    assert!(analysis.cost.attribution_cost_ratio > 0.0);
    assert!(analysis.cost.attribution_cost_ratio < 1.0);
    assert_eq!(
        analysis.cost.attribution_cost,
        analysis.cost.contract_steps as f64
            + analysis.cost.statistical_work_units
            + analysis.cost.replay_steps as f64
            + analysis.cost.evidence_digest_work_units as f64
            + analysis.cost.analysis_cache_lookups as f64
    );
    assert_eq!(engine.episodes().count().unwrap(), 1);
    assert_eq!(
        engine
            .graph()
            .list_procedure_versions(procedure.id)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        engine
            .graph()
            .get_procedure(procedure.id)
            .unwrap()
            .unwrap()
            .body,
        procedure.body
    );
}

#[test]
fn explicit_retry_key_counts_lookup_and_first_miss_digest_without_recomputing_report() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = failed_double(&engine, &procedure);
    let request = FailureAnalysisRequest {
        episode_id: failed.id,
        selected_feedback_id: None,
        candidates: Vec::new(),
        budget: FailureAnalysisBudget::default(),
    };

    let first = engine
        .analyze_failure_idempotent("typed-cost-explicit-key", request.clone())
        .unwrap();
    let retry = engine
        .analyze_failure_idempotent("typed-cost-explicit-key", request)
        .unwrap();

    assert_eq!(first.cost.analysis_cache_lookups, 1);
    assert!(first.cost.evidence_digest_work_units > 0);
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(retry).unwrap()
    );
}

#[test]
fn phase2_indexed_credit_cost_declines_and_crosses_threshold_as_traces_lengthen() {
    #[derive(Debug)]
    struct CurvePoint {
        trace_steps: usize,
        history_size: usize,
        ratio: f64,
    }

    assert_eq!(PHASE2_MAX_ATTRIBUTION_COST_RATIO, 0.5);
    let mut curve = Vec::new();
    for call_count in [1_usize, 2, 4] {
        for history_size in [1_usize, 4, 8] {
            let engine = Engine::in_memory_with_admin("test-admin").unwrap();
            let leaf = double();
            engine.admin_insert_procedure(&leaf).unwrap();
            let parent = Procedure::new(
                format!("COST_CHAIN_{call_count}_{history_size}"),
                vec![Param::named("x")],
                Expr::Block(
                    (0..call_count)
                        .map(|_| Expr::Call {
                            procedure: leaf.id,
                            args: vec![Expr::Var("x".into())],
                        })
                        .collect(),
                ),
            );
            engine.admin_insert_procedure(&parent).unwrap();
            for value in 1..history_size {
                engine
                    .execute_procedure(
                        parent.id,
                        inputs(value as i64),
                        Some(Value::Int(value as i64 * 2)),
                    )
                    .unwrap();
            }
            let failed = engine
                .execute_procedure(parent.id, inputs(99), Some(Value::Int(999)))
                .unwrap()
                .episode;
            let analysis = engine
                .analyze_failure(FailureAnalysisRequest {
                    episode_id: failed.id,
                    selected_feedback_id: None,
                    candidates: Vec::new(),
                    budget: FailureAnalysisBudget::default(),
                })
                .unwrap();
            let trace_steps = call_count + 1;
            assert_eq!(analysis.cost.evidence_digest_history_episodes_scanned, 0);
            assert_eq!(
                analysis.cost.evidence_digest_trace_steps_scanned,
                trace_steps as u64
            );
            assert_eq!(analysis.cost.statistical_trace_steps_scanned, 0);
            assert_eq!(analysis.cost.evidence_digest_element_aggregate_rows_read, 2);
            assert_eq!(analysis.cost.evidence_digest_pair_aggregate_rows_read, 1);
            curve.push(CurvePoint {
                trace_steps,
                history_size,
                ratio: analysis.cost.attribution_cost_ratio,
            });
        }
    }

    // This is an acceptance measurement, not a tuned denominator. Index
    // ingestion occurs transactionally with episode writes; first analysis
    // still counts cache lookup, source/trace/snapshot digest work, aggregate
    // row reads, contract inspection, and aggregate processing.
    let failures = curve
        .iter()
        .filter(|point| point.trace_steps == 5 && point.ratio >= PHASE2_MAX_ATTRIBUTION_COST_RATIO)
        .collect::<Vec<_>>();
    assert!(failures.is_empty(), "{curve:#?}");
    assert!(
        curve
            .iter()
            .any(|point| point.ratio >= PHASE2_MAX_ATTRIBUTION_COST_RATIO),
        "fixed startup costs on tiny traces must remain visible: {curve:#?}"
    );
    for history_size in [1, 4, 8] {
        let points = curve
            .iter()
            .filter(|point| point.history_size == history_size)
            .collect::<Vec<_>>();
        assert!(
            points.windows(2).all(|pair| pair[1].ratio < pair[0].ratio),
            "{curve:#?}"
        );
    }
    assert!(curve.iter().any(|point| point.trace_steps == 5));
    assert!(curve.iter().any(|point| point.history_size == 8));
}

#[derive(Default)]
struct FixedSimulator;

impl SimulatedReplayModel for FixedSimulator {
    type Error = std::convert::Infallible;

    fn model_id(&self) -> &str {
        "rise-model"
    }

    fn model_version(&self) -> &str {
        "3"
    }

    fn simulate(
        &mut self,
        request: SimulatedReplayRequest,
    ) -> Result<SimulatedReplayObservation, Self::Error> {
        assert_eq!(request.model_id, "rise-model");
        assert_eq!(request.model_version, "3");
        assert_eq!(request.step_budget, 20);
        Ok(SimulatedReplayObservation {
            result: Value::Int(15),
            steps_used: 4,
            details: "bounded kitchen model predicts recovery".into(),
        })
    }
}

#[test]
fn engine_minted_simulator_receipt_is_exact_stored_and_non_decisive() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = failed_double(&engine, &procedure);
    let candidate = mutation_candidate(
        &procedure,
        triple_body(),
        ReplayVerification::SimulatedExpected {
            expected: Value::Int(15),
            model_id: "rise-model".into(),
            model_version: "3".into(),
            assumptions: vec!["oven temperature held fixed".into()],
        },
        CounterfactualMode::Simulated,
    );
    let mut simulator = FixedSimulator;
    let issued = engine
        .issue_simulated_replay_receipt(failed.id, candidate, 20, &mut simulator)
        .unwrap();

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![issued.clone()],
            budget: FailureAnalysisBudget {
                top_k: 1,
                max_replays: 1,
                max_replay_steps: 20,
            },
        })
        .unwrap();
    let attribution = &analysis.counterfactual.attributions[0];
    assert_eq!(attribution.confidence, AttributionConfidence::Medium);
    assert!(!attribution.decisive);
    let AttributionEvidence::Replay {
        counterfactual_succeeded,
        provenance,
        steps_used,
        ..
    } = &attribution.evidence[0]
    else {
        panic!("expected simulated replay evidence");
    };
    assert_eq!(*counterfactual_succeeded, Some(true));
    assert_eq!(*steps_used, 4);
    assert!(matches!(
        &provenance.verification,
        Some(ekg_credit::ReplayVerificationProvenance::Simulated {
            receipt_id: Some(receipt_id),
            model_id,
            model_version,
            assumptions,
        }) if receipt_id.starts_with("sha256:")
            && model_id == "rise-model"
            && model_version == "3"
            && assumptions == &["oven temperature held fixed"]
    ));

    let mut tampered = issued;
    tampered.change.description = "same receipt, changed payload".into();
    tampered.change.replacement["body"] =
        serde_json::to_value(Expr::Literal(Value::Int(99))).unwrap();
    let rejected = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![tampered],
            budget: FailureAnalysisBudget {
                top_k: 1,
                max_replays: 1,
                max_replay_steps: 20,
            },
        })
        .unwrap();
    assert_eq!(
        rejected.counterfactual.attributions[0].confidence,
        AttributionConfidence::Inconclusive
    );
    assert!(!rejected.counterfactual.attributions[0].decisive);
}

#[test]
fn arbitrary_embedding_simulator_cannot_mint_a_durable_receipt() {
    let engine = Engine::in_memory().unwrap();
    let procedure = double();
    let candidate = mutation_candidate(
        &procedure,
        triple_body(),
        ReplayVerification::SimulatedExpected {
            expected: Value::Int(15),
            model_id: "caller-model".into(),
            model_version: "1".into(),
            assumptions: Vec::new(),
        },
        CounterfactualMode::Simulated,
    );
    let mut simulator = FixedSimulator;

    let error = engine
        .issue_simulated_replay_receipt(ekg_core::EpisodeId::new(), candidate, 20, &mut simulator)
        .unwrap_err();
    assert!(error.to_string().contains("admin authority"));
}

#[test]
fn simulated_replay_without_engine_receipt_is_rejected_as_inconclusive() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = failed_double(&engine, &procedure);
    let candidate = mutation_candidate(
        &procedure,
        triple_body(),
        ReplayVerification::SimulatedExpected {
            expected: Value::Int(21),
            model_id: "rise-model".into(),
            model_version: "3".into(),
            assumptions: vec!["oven temperature held fixed".into()],
        },
        CounterfactualMode::Simulated,
    );

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![candidate],
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap();
    let attribution = &analysis.counterfactual.attributions[0];

    assert_eq!(attribution.confidence, AttributionConfidence::Inconclusive);
    assert!(!attribution.decisive);
    let AttributionEvidence::Replay {
        counterfactual_succeeded,
        details,
        ..
    } = &attribution.evidence[0]
    else {
        panic!("expected replay evidence");
    };
    assert_eq!(*counterfactual_succeeded, None);
    assert!(details.contains("rejected before execution"));
}

#[test]
fn replay_budget_zero_is_inconclusive_and_spends_nothing() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = failed_double(&engine, &procedure);
    let candidate = mutation_candidate(
        &procedure,
        triple_body(),
        ReplayVerification::DeterministicExpected {
            expected: Value::Int(21),
        },
        CounterfactualMode::Deterministic,
    );

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![candidate],
            budget: FailureAnalysisBudget {
                top_k: 1,
                max_replays: 1,
                max_replay_steps: 0,
            },
        })
        .unwrap();

    assert!(analysis.counterfactual.attributions.is_empty());
    assert_eq!(
        analysis.counterfactual.stop_reason,
        Some(BudgetStopReason::StepLimit)
    );
    assert_eq!(analysis.cost.replay_steps, 0);
}

#[test]
fn replay_boundary_rejects_target_injection_and_effectful_payloads() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = failed_double(&engine, &procedure);
    let suspect = Suspect {
        procedure: procedure.id,
        version: 1,
        trace_step: 0,
    };
    let mut replayer = engine.version_pinned_replayer();

    let absent_target = replayer
        .replay(ReplayRequest {
            source_episode: failed.id,
            suspect: Suspect {
                trace_step: 99,
                ..suspect
            },
            change: mutation_candidate(
                &procedure,
                triple_body(),
                ReplayVerification::DeterministicExpected {
                    expected: Value::Int(21),
                },
                CounterfactualMode::Deterministic,
            )
            .change,
            mode: CounterfactualMode::Deterministic,
            step_budget: 100,
        })
        .unwrap();
    let ReplayOutcome::NotReplayable { reason } = absent_target.outcome else {
        panic!("absent target must be rejected");
    };
    assert!(reason.contains("suspect trace step is absent"));

    let injected = CounterfactualMutation::ReplaceContract {
        target: ProcedureVersionRef {
            id: ekg_core::ProcedureId::new(),
            version: 99,
        },
        contract: Contract::default(),
        verification: ReplayVerification::DeterministicExpected {
            expected: Value::Int(14),
        },
    };
    let rejected = replayer
        .replay(ReplayRequest {
            source_episode: failed.id,
            suspect,
            change: CounterfactualChange {
                description: "inject another target".into(),
                replacement: serde_json::to_value(injected).unwrap(),
            },
            mode: CounterfactualMode::Deterministic,
            step_budget: 100,
        })
        .unwrap();
    assert!(matches!(
        rejected.outcome,
        ReplayOutcome::NotReplayable { .. }
    ));

    let effectful = replayer
        .replay(ReplayRequest {
            source_episode: failed.id,
            suspect,
            change: CounterfactualChange {
                description: "send a network request".into(),
                replacement: json!({ "kind": "effectful", "url": "https://example.test" }),
            },
            mode: CounterfactualMode::Deterministic,
            step_budget: 100,
        })
        .unwrap();
    assert!(matches!(
        effectful.outcome,
        ReplayOutcome::NotReplayable { .. }
    ));
}

#[test]
fn replay_boundary_rejects_missing_and_mixed_source_versions() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let source = failed_double(&engine, &procedure);
    let source_procedure = procedure.clone();
    procedure.version = 2;
    engine.admin_update_procedure(&procedure).unwrap();

    let mut missing = source.clone();
    missing.id = ekg_core::EpisodeId::new();
    let mut missing_trace: ExecTrace =
        serde_json::from_value(missing.execution_trace.clone().unwrap()).unwrap();
    missing_trace.steps[0].procedure_version = None;
    missing.execution_trace = Some(serde_json::to_value(missing_trace).unwrap());
    engine.admin_insert_episode(&missing).unwrap();

    let mutation = mutation_candidate(
        &source_procedure,
        triple_body(),
        ReplayVerification::DeterministicExpected {
            expected: Value::Int(21),
        },
        CounterfactualMode::Deterministic,
    );
    let mut replayer = engine.version_pinned_replayer();
    let missing_result = replayer
        .replay(ReplayRequest {
            source_episode: missing.id,
            suspect: Suspect {
                procedure: procedure.id,
                version: 1,
                trace_step: 0,
            },
            change: mutation.change.clone(),
            mode: CounterfactualMode::Deterministic,
            step_budget: 100,
        })
        .unwrap();
    let ReplayOutcome::NotReplayable { reason } = missing_result.outcome else {
        panic!("missing version must be rejected");
    };
    assert!(reason.contains("no procedure version"));

    let mut mixed = source;
    mixed.id = ekg_core::EpisodeId::new();
    let mut mixed_trace: ExecTrace =
        serde_json::from_value(mixed.execution_trace.clone().unwrap()).unwrap();
    let mut second = mixed_trace.steps[0].clone();
    second.procedure_version = Some(2);
    second.status = ExecStepStatus::Succeeded;
    mixed_trace.steps.push(second);
    mixed.execution_trace = Some(serde_json::to_value(mixed_trace).unwrap());
    mixed.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: false,
        details: "injected mixed trace".into(),
        surprise: Some(1.0),
    });
    engine.admin_insert_episode(&mixed).unwrap();

    let mixed_result = replayer
        .replay(ReplayRequest {
            source_episode: mixed.id,
            suspect: Suspect {
                procedure: procedure.id,
                version: 1,
                trace_step: 0,
            },
            change: mutation.change,
            mode: CounterfactualMode::Deterministic,
            step_budget: 100,
        })
        .unwrap();
    let ReplayOutcome::NotReplayable { reason } = mixed_result.outcome else {
        panic!("mixed versions must be rejected");
    };
    assert!(reason.contains("mixes versions"));
}

#[test]
fn inactive_nested_procedure_is_excluded_from_current_evaluator() {
    for lifecycle in [
        Lifecycle::Stale,
        Lifecycle::UnderReview,
        Lifecycle::Superseded,
        Lifecycle::Retired,
        Lifecycle::Invalid,
    ] {
        let engine = Engine::in_memory_with_admin("test-admin").unwrap();
        let mut child = double();
        child.lifecycle = lifecycle;
        engine.admin_insert_procedure(&child).unwrap();
        let parent = Procedure::new(
            "PARENT",
            vec![Param::named("x")],
            Expr::Call {
                procedure: child.id,
                args: vec![Expr::Var("x".into())],
            },
        );
        engine.admin_insert_procedure(&parent).unwrap();

        let error = engine
            .execute_procedure(parent.id, inputs(7), None)
            .unwrap_err();

        assert!(error.to_string().contains("undefined procedure"));
    }
}

#[test]
fn typed_contract_patch_replays_without_persisting_the_candidate() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut procedure = double();
    procedure.contract.requires.push(
        Condition::described("injected invalid precondition")
            .with_check(Expr::Literal(Value::Bool(false))),
    );
    engine.admin_insert_procedure(&procedure).unwrap();
    let error = engine
        .execute_procedure(procedure.id, inputs(7), Some(Value::Int(14)))
        .unwrap_err();
    let EngineError::ExecutionFailed { episode_id, .. } = error else {
        panic!("the injected contract must fail execution");
    };
    let failed = engine.episodes().get(episode_id).unwrap();
    let mutation = CounterfactualMutation::ReplaceContract {
        target: ProcedureVersionRef {
            id: procedure.id,
            version: 1,
        },
        contract: Contract::default(),
        verification: ReplayVerification::DeterministicExpected {
            expected: Value::Int(14),
        },
    };
    let mut replayer = engine.version_pinned_replayer();

    let observation = replayer
        .replay(ReplayRequest {
            source_episode: failed.id,
            suspect: Suspect {
                procedure: procedure.id,
                version: 1,
                trace_step: 0,
            },
            change: CounterfactualChange {
                description: "replace contract".into(),
                replacement: serde_json::to_value(mutation).unwrap(),
            },
            mode: CounterfactualMode::Deterministic,
            step_budget: 100,
        })
        .unwrap();

    assert_eq!(observation.outcome, ReplayOutcome::Succeeded);
    assert_eq!(engine.episodes().count().unwrap(), 1);
    assert_eq!(
        engine
            .graph()
            .list_procedure_versions(procedure.id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn contract_patch_without_causal_contract_effect_is_inconclusive() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = failed_double(&engine, &procedure);
    let mut no_effect_contract = Contract::default();
    no_effect_contract.costs.operations = 1;
    let mutation = CounterfactualMutation::ReplaceContract {
        target: ProcedureVersionRef {
            id: procedure.id,
            version: 1,
        },
        contract: no_effect_contract,
        verification: ReplayVerification::DeterministicExpected {
            expected: Value::Int(14),
        },
    };

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![CounterfactualCandidate {
                suspect: Suspect {
                    procedure: procedure.id,
                    version: 1,
                    trace_step: 0,
                },
                prior_score: 0.8,
                change: CounterfactualChange {
                    description: "change contract cost metadata only".into(),
                    replacement: serde_json::to_value(mutation).unwrap(),
                },
                mode: CounterfactualMode::Deterministic,
            }],
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap();

    assert_eq!(
        analysis.counterfactual.attributions[0].confidence,
        AttributionConfidence::Inconclusive
    );
    assert!(!analysis.counterfactual.attributions[0].decisive);
    let AttributionEvidence::Replay {
        counterfactual_succeeded,
        ..
    } = &analysis.counterfactual.attributions[0].evidence[0]
    else {
        panic!("expected replay evidence");
    };
    assert_eq!(*counterfactual_succeeded, Some(false));
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn selected_late_feedback_drives_failure_analysis_without_rewriting_the_episode() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let original = engine
        .execute_procedure(procedure.id, inputs(7), Some(Value::Int(14)))
        .unwrap()
        .episode;
    let feedback = engine
        .admin_append_feedback(&EpisodeFeedback::new(
            original.id,
            Value::Text("flat pancake".into()),
            Evaluation {
                tier: VerifiabilityTier::Deferred,
                success: false,
                details: "injected kitchen result had no rise".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("human", Some("kitchen-tester".into())),
            "flat-pancake-feedback",
        ))
        .unwrap();
    let candidate = mutation_candidate(
        &procedure,
        triple_body(),
        ReplayVerification::DeterministicExpected {
            expected: Value::Int(21),
        },
        CounterfactualMode::Deterministic,
    );

    let mut replayer = engine.version_pinned_replayer();
    let successful_source = replayer
        .replay(ReplayRequest {
            source_episode: original.id,
            suspect: candidate.suspect,
            change: candidate.change.clone(),
            mode: candidate.mode,
            step_budget: 100,
        })
        .unwrap();
    let ReplayOutcome::NotReplayable { reason } = successful_source.outcome else {
        panic!("replay requires selected evidence that the source failed");
    };
    assert!(reason.contains("does not identify a failed episode"));

    let without_feedback = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: original.id,
            selected_feedback_id: None,
            candidates: vec![candidate.clone()],
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap_err();
    assert!(without_feedback.to_string().contains("not a failed"));

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: original.id,
            selected_feedback_id: Some(feedback.id),
            candidates: vec![candidate],
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap();

    assert_eq!(
        analysis.failure_evidence.source,
        FailureEvidenceSource::LateFeedback {
            feedback_id: feedback.id
        }
    );
    assert_eq!(
        analysis.failure_evidence.evaluation.tier,
        VerifiabilityTier::Deferred
    );
    assert!(analysis.ranked.iter().all(|attribution| {
        attribution
            .provenance
            .details
            .iter()
            .any(|detail| detail.contains(&feedback.id.to_string()))
    }));
    assert!(engine.episodes().get(original.id).unwrap().succeeded());
}

#[test]
fn feedback_must_belong_to_the_analyzed_episode() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let first = engine
        .execute_procedure(procedure.id, inputs(7), Some(Value::Int(14)))
        .unwrap()
        .episode;
    let second = engine
        .execute_procedure(procedure.id, inputs(8), Some(Value::Int(16)))
        .unwrap()
        .episode;
    let feedback = engine
        .admin_append_feedback(&EpisodeFeedback::new(
            second.id,
            Value::Text("flat".into()),
            Evaluation {
                tier: VerifiabilityTier::Deferred,
                success: false,
                details: "unrelated failure".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("human", None),
            "unrelated-feedback",
        ))
        .unwrap();

    let error = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: first.id,
            selected_feedback_id: Some(feedback.id),
            candidates: Vec::new(),
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap_err();

    assert!(error.to_string().contains("does not belong"));
}

#[test]
fn caller_expected_matching_mutant_cannot_override_canonical_prediction() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = failed_double(&engine, &procedure);
    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![mutation_candidate(
                &procedure,
                triple_body(),
                ReplayVerification::DeterministicExpected {
                    expected: Value::Int(21),
                },
                CounterfactualMode::Deterministic,
            )],
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap();

    let attribution = &analysis.counterfactual.attributions[0];
    assert_eq!(attribution.confidence, AttributionConfidence::Inconclusive);
    assert!(!attribution.decisive);
    let AttributionEvidence::Replay {
        counterfactual_succeeded,
        details,
        provenance,
        ..
    } = &attribution.evidence[0]
    else {
        panic!("expected replay evidence");
    };
    assert_eq!(*counterfactual_succeeded, Some(false));
    assert!(details.contains("caller expected 21 was ignored"));
    assert!(details.contains("canonical precommitted oracle 15"));
    let serialized = serde_json::to_value(provenance).unwrap();
    assert!(
        serialized["verification"]["verifier"]
            .as_str()
            .unwrap()
            .contains("oracle=fnv1a64:")
    );
}

#[test]
fn replay_rejects_tampered_source_when_exact_baseline_does_not_match() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let mut tampered = failed_double(&engine, &procedure);
    tampered.id = ekg_core::EpisodeId::new();
    let mut trace: ExecTrace =
        serde_json::from_value(tampered.execution_trace.clone().unwrap()).unwrap();
    trace.steps[0].output = Value::Int(999);
    tampered.execution_trace = Some(serde_json::to_value(trace).unwrap());
    engine.admin_insert_episode(&tampered).unwrap();

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: tampered.id,
            selected_feedback_id: None,
            candidates: vec![mutation_candidate(
                &procedure,
                triple_body(),
                ReplayVerification::DeterministicExpected {
                    expected: Value::Int(15),
                },
                CounterfactualMode::Deterministic,
            )],
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap();

    let AttributionEvidence::Replay {
        counterfactual_succeeded,
        details,
        steps_used,
        ..
    } = &analysis.counterfactual.attributions[0].evidence[0]
    else {
        panic!("expected replay evidence");
    };
    assert_eq!(*counterfactual_succeeded, None);
    assert!(*steps_used > 0);
    assert!(details.contains("baseline identity failed"));
}

#[test]
fn version_wide_mutation_of_repeated_call_is_not_step_decisive() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let child = double();
    engine.admin_insert_procedure(&child).unwrap();
    let parent = Procedure::new(
        "DOUBLE_TWICE",
        vec![Param::named("x")],
        Expr::Block(vec![
            Expr::Call {
                procedure: child.id,
                args: vec![Expr::Var("x".into())],
            },
            Expr::Call {
                procedure: child.id,
                args: vec![Expr::Var("x".into())],
            },
        ]),
    );
    engine.admin_insert_procedure(&parent).unwrap();
    let failed = engine
        .execute_procedure(parent.id, inputs(7), Some(Value::Int(21)))
        .unwrap()
        .episode;
    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![mutation_candidate(
                &child,
                triple_body(),
                ReplayVerification::DeterministicExpected {
                    expected: Value::Int(21),
                },
                CounterfactualMode::Deterministic,
            )],
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap();

    let attribution = &analysis.counterfactual.attributions[0];
    assert_eq!(attribution.confidence, AttributionConfidence::Inconclusive);
    assert!(!attribution.decisive);
    let AttributionEvidence::Replay {
        counterfactual_succeeded,
        ..
    } = attribution.evidence[0]
    else {
        panic!("expected replay evidence");
    };
    assert_eq!(counterfactual_succeeded, None);
    assert!(attribution.limitations.iter().any(|limitation| {
        matches!(
            limitation,
            ekg_credit::AttributionLimitation::NotReplayable { reason }
                if reason.contains("occurs 2 times")
        )
    }));
}

#[test]
fn decoy_prior_cannot_make_an_unknown_element_a_replay_suspect() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = engine
        .execute_procedure(procedure.id, inputs(7), Some(Value::Int(21)))
        .unwrap()
        .episode;
    let mut valid = mutation_candidate(
        &procedure,
        triple_body(),
        ReplayVerification::DeterministicExpected {
            expected: Value::Int(-999),
        },
        CounterfactualMode::Deterministic,
    );
    valid.prior_score = -1_000_000.0;
    let mut decoy = valid.clone();
    decoy.suspect.procedure = ekg_core::ProcedureId::new();
    decoy.prior_score = 1_000_000.0;

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![decoy.clone(), valid],
            budget: FailureAnalysisBudget {
                top_k: 1,
                max_replays: 1,
                max_replay_steps: 100,
            },
        })
        .unwrap();

    assert_eq!(analysis.counterfactual.replays_run, 1);
    assert_eq!(analysis.counterfactual.attributions.len(), 1);
    assert_eq!(
        analysis.counterfactual.attributions[0].suspect.procedure,
        procedure.id
    );
    assert!(analysis.counterfactual.attributions[0].decisive);

    let omitted = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![decoy],
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap();
    assert_eq!(omitted.counterfactual.replays_run, 0);
    assert!(omitted.counterfactual.attributions.is_empty());
    assert!(
        omitted
            .ranked
            .iter()
            .any(|attribution| attribution.suspect.procedure == procedure.id)
    );
}

#[test]
fn deterministic_replay_without_precommitted_strong_oracle_is_inconclusive() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let mut weak = failed_double(&engine, &procedure);
    weak.id = ekg_core::EpisodeId::new();
    weak.evaluation.as_mut().unwrap().tier = VerifiabilityTier::Deferred;
    engine.admin_insert_episode(&weak).unwrap();

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: weak.id,
            selected_feedback_id: None,
            candidates: vec![mutation_candidate(
                &procedure,
                triple_body(),
                ReplayVerification::DeterministicExpected {
                    expected: Value::Int(21),
                },
                CounterfactualMode::Deterministic,
            )],
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap();
    let attribution = &analysis.counterfactual.attributions[0];
    assert_eq!(attribution.confidence, AttributionConfidence::Inconclusive);
    assert!(!attribution.decisive);
    assert_eq!(analysis.cost.replay_steps, 0);
    assert!(attribution.limitations.iter().any(|limitation| matches!(
        limitation,
        ekg_credit::AttributionLimitation::NotReplayable { reason }
            if reason.contains("Hard or Consensus oracle")
    )));
}

#[test]
fn late_feedback_statistics_join_once_and_exclude_conflicting_episode() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = failed_double(&engine, &procedure);
    let late_failure = engine
        .execute_procedure(procedure.id, inputs(8), Some(Value::Int(16)))
        .unwrap()
        .episode;
    engine
        .admin_append_feedback(&EpisodeFeedback::new(
            late_failure.id,
            Value::Text("late failure".into()),
            Evaluation {
                tier: VerifiabilityTier::Hard,
                success: false,
                details: "independent check failed".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("test", Some("oracle-a".into())),
            "late-failure",
        ))
        .unwrap();
    let conflict = engine
        .execute_procedure(procedure.id, inputs(9), Some(Value::Int(18)))
        .unwrap()
        .episode;
    for (key, success) in [("conflict-fail", false), ("conflict-pass", true)] {
        engine
            .admin_append_feedback(&EpisodeFeedback::new(
                conflict.id,
                Value::Bool(success),
                Evaluation {
                    tier: VerifiabilityTier::Hard,
                    success,
                    details: "conflicting late result".into(),
                    surprise: None,
                },
                FeedbackSource::new("test", Some(key.into())),
                key,
            ))
            .unwrap();
    }

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: Vec::new(),
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap();
    let AttributionEvidence::Statistics {
        exposures,
        failures,
        ..
    } = analysis.statistical[0].evidence[0]
    else {
        panic!("expected statistical evidence");
    };
    assert_eq!((exposures, failures), (2, 2));
    assert_eq!(analysis.cost.statistical_feedback_rows_scanned, 0);
    assert_eq!(analysis.cost.statistical_conflicts_excluded, 1);
}

#[test]
fn identical_analysis_retry_has_stable_content_addressed_identity() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = engine
        .execute_procedure(procedure.id, inputs(7), Some(Value::Int(21)))
        .unwrap()
        .episode;
    let request = FailureAnalysisRequest {
        episode_id: failed.id,
        selected_feedback_id: None,
        candidates: vec![mutation_candidate(
            &procedure,
            triple_body(),
            ReplayVerification::DeterministicExpected {
                expected: Value::Int(123_456),
            },
            CounterfactualMode::Deterministic,
        )],
        budget: FailureAnalysisBudget::default(),
    };

    let first = engine.analyze_failure(request.clone()).unwrap();
    let second = engine.analyze_failure(request).unwrap();
    assert_eq!(first.analysis_id, second.analysis_id);
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(second).unwrap()
    );
}

#[test]
fn completed_analysis_is_durable_idempotent_and_conflict_safe() {
    let path = std::env::temp_dir().join(format!(
        "ekg-credit-analysis-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().to_string();
    let request;
    let first;
    let automatic_before_feedback;
    {
        let engine = Engine::open_with_admin(&path_text, "test-admin").unwrap();
        let procedure = double();
        engine.admin_insert_procedure(&procedure).unwrap();
        let failed = engine
            .execute_procedure(procedure.id, inputs(7), Some(Value::Int(21)))
            .unwrap()
            .episode;
        request = FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![mutation_candidate(
                &procedure,
                triple_body(),
                ReplayVerification::DeterministicExpected {
                    expected: Value::Int(-123),
                },
                CounterfactualMode::Deterministic,
            )],
            budget: FailureAnalysisBudget::default(),
        };
        first = engine
            .analyze_failure_idempotent("durable-credit-analysis", request.clone())
            .unwrap();
        automatic_before_feedback = engine.analyze_failure(request.clone()).unwrap();
        assert_eq!(automatic_before_feedback.analysis_id, first.analysis_id);
        assert_eq!(
            serde_json::to_value(
                engine
                    .get_failure_analysis(&first.analysis_id)
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(&first).unwrap()
        );
    }

    let reopened = Engine::open_with_admin(&path_text, "test-admin").unwrap();
    let stored = reopened
        .get_failure_analysis(&first.analysis_id)
        .unwrap()
        .unwrap();
    reopened
        .admin_append_feedback(&EpisodeFeedback::new(
            request.episode_id,
            Value::Text("new evidence after completion".into()),
            Evaluation {
                tier: VerifiabilityTier::Deferred,
                success: false,
                details: "arrived after the canonical analysis".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("test", Some("late-auditor".into())),
            "post-analysis-feedback",
        ))
        .unwrap();
    let retry = reopened
        .analyze_failure_idempotent("durable-credit-analysis", request.clone())
        .unwrap();
    let refreshed = reopened.analyze_failure(request.clone()).unwrap();
    let refreshed_retry = reopened.analyze_failure(request.clone()).unwrap();
    let by_key = reopened
        .get_failure_analysis_by_key("durable-credit-analysis")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&stored).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&retry).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert_ne!(refreshed.analysis_id, automatic_before_feedback.analysis_id);
    assert_eq!(
        serde_json::to_value(&refreshed_retry).unwrap(),
        serde_json::to_value(&refreshed).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&by_key).unwrap(),
        serde_json::to_value(&first).unwrap()
    );

    let mut conflicting = request;
    conflicting.budget.max_replays += 1;
    let error = reopened
        .analyze_failure_idempotent("durable-credit-analysis", conflicting)
        .unwrap_err();
    assert!(error.to_string().contains("different request"));
    assert_eq!(
        serde_json::to_value(
            reopened
                .get_failure_analysis_by_key("durable-credit-analysis")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(first).unwrap()
    );
    let raw = rusqlite::Connection::open(&path).unwrap();
    let immutable = raw
        .execute(
            "UPDATE engine_credit_analyses SET analysis_json = '{}' WHERE analysis_id = ?1",
            rusqlite::params![stored.analysis_id],
        )
        .unwrap_err();
    assert!(immutable.to_string().contains("immutable"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn failed_analysis_does_not_reserve_its_idempotency_key() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let missing = ekg_core::EpisodeId::new();
    let request = FailureAnalysisRequest {
        episode_id: missing,
        selected_feedback_id: None,
        candidates: Vec::new(),
        budget: FailureAnalysisBudget::default(),
    };

    assert!(
        engine
            .analyze_failure_idempotent("incomplete-analysis", request)
            .is_err()
    );
    assert!(
        engine
            .get_failure_analysis_by_key("incomplete-analysis")
            .unwrap()
            .is_none()
    );
}

#[test]
fn missing_or_mismatched_replay_provenance_cannot_be_decisive() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let failed = failed_double(&engine, &procedure);
    let mismatched = mutation_candidate(
        &procedure,
        triple_body(),
        ReplayVerification::SimulatedExpected {
            expected: Value::Int(21),
            model_id: "simulator".into(),
            model_version: "1".into(),
            assumptions: Vec::new(),
        },
        CounterfactualMode::Deterministic,
    );
    let missing_model = mutation_candidate(
        &procedure,
        triple_body(),
        ReplayVerification::SimulatedExpected {
            expected: Value::Int(21),
            model_id: String::new(),
            model_version: String::new(),
            assumptions: Vec::new(),
        },
        CounterfactualMode::Simulated,
    );
    let mut missing_provenance = mutation_candidate(
        &procedure,
        triple_body(),
        ReplayVerification::DeterministicExpected {
            expected: Value::Int(21),
        },
        CounterfactualMode::Deterministic,
    );
    missing_provenance
        .change
        .replacement
        .as_object_mut()
        .unwrap()
        .remove("verification");

    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![mismatched, missing_model, missing_provenance],
            budget: FailureAnalysisBudget {
                top_k: 3,
                max_replays: 3,
                max_replay_steps: 100,
            },
        })
        .unwrap();

    assert_eq!(analysis.counterfactual.attributions.len(), 3);
    assert!(
        analysis
            .counterfactual
            .attributions
            .iter()
            .all(|attribution| {
                attribution.confidence == AttributionConfidence::Inconclusive
                    && !attribution.decisive
            })
    );
}
