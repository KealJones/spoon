use std::collections::HashSet;

use spoon_core::{Episode, Evaluation, ProcedureId, Value, VerifiabilityTier};
use spoon_credit::{
    AttributionConfidence, AttributionEvidence, AttributionLimitation, AttributionMechanism,
    BudgetStopReason, ContractSection, CounterfactualCandidate, CounterfactualChange,
    CounterfactualMode, CounterfactualReplayer, ReplayBudget, ReplayObservation, ReplayOutcome,
    ReplayProvenance, ReplayRequest, ReplayVerificationProvenance, Suspect,
    attribute_contract_violations, rank_statistical_suspects,
    rank_statistical_suspects_from_aggregates, rank_statistical_suspects_with_cost,
    run_counterfactual_replays,
};
use spoon_episode::{CreditElementRef, EpisodeStore};
use spoon_exec::{
    ConditionCheck, ConditionCheckStatus, ContractChecks, ExecStep, ExecStepStatus, ExecTrace,
};

fn step(
    procedure: ProcedureId,
    version: u32,
    status: ExecStepStatus,
    contract_checks: ContractChecks,
) -> ExecStep {
    ExecStep {
        expr_description: "injected call".into(),
        input: Some(Value::List(vec![Value::Int(1)])),
        output: Value::Null,
        procedure_called: Some(procedure),
        procedure_version: Some(version),
        contract_checks,
        status,
    }
}

fn check(description: &str, status: ConditionCheckStatus) -> ConditionCheck {
    ConditionCheck {
        description: description.into(),
        status,
    }
}

fn episode_with_trace(trace: ExecTrace, failed: bool, created_at: i64) -> Episode {
    let mut episode = Episode::new("injected fault");
    episode.execution_trace = Some(serde_json::to_value(trace).unwrap());
    episode.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: !failed,
        details: if failed { "failed" } else { "passed" }.into(),
        surprise: None,
    });
    episode.cost.steps_taken = 10;
    episode.cost.budget_spent = 20.0;
    episode.created_at = created_at;
    episode
}

fn failed_source() -> Episode {
    let mut source = Episode::new("failed source");
    source.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: false,
        details: "verified failure".into(),
        surprise: Some(1.0),
    });
    source
}

#[test]
fn contract_scanner_distinguishes_all_violation_sections_and_exact_steps() {
    let requires_proc = ProcedureId::new();
    let promises_proc = ProcedureId::new();
    let fails_proc = ProcedureId::new();
    let trace = ExecTrace {
        steps: vec![
            step(
                requires_proc,
                2,
                ExecStepStatus::Failed {
                    error: "requirement".into(),
                },
                ContractChecks {
                    requires: vec![check("x is positive", ConditionCheckStatus::Violated)],
                    promises: vec![check("ignored", ConditionCheckStatus::NotExecutable)],
                    fails_when: Vec::new(),
                },
            ),
            step(
                promises_proc,
                4,
                ExecStepStatus::Failed {
                    error: "promise".into(),
                },
                ContractChecks {
                    requires: vec![check("passed", ConditionCheckStatus::Passed)],
                    promises: vec![check("result is even", ConditionCheckStatus::Violated)],
                    fails_when: Vec::new(),
                },
            ),
            step(
                fails_proc,
                7,
                ExecStepStatus::Failed {
                    error: "declared failure".into(),
                },
                ContractChecks {
                    requires: Vec::new(),
                    promises: Vec::new(),
                    fails_when: vec![check("input is zero", ConditionCheckStatus::Violated)],
                },
            ),
        ],
    };
    let episode = episode_with_trace(trace, true, 1);

    let report = attribute_contract_violations(&episode).unwrap();

    assert_eq!(report.steps_inspected, 3);
    assert_eq!(report.attributions.len(), 3);
    assert_eq!(
        report.attributions[0].mechanism,
        AttributionMechanism::ContractViolation
    );
    assert_eq!(
        report.attributions[0].confidence,
        AttributionConfidence::High
    );
    assert_eq!(report.attributions[0].suspect.procedure, requires_proc);
    assert_eq!(report.attributions[0].suspect.version, 2);
    assert_eq!(report.attributions[0].suspect.trace_step, 0);
    assert_eq!(
        report.attributions[0].contract_section(),
        Some(ContractSection::Requires)
    );
    assert_eq!(
        report.attributions[1].contract_section(),
        Some(ContractSection::Promises)
    );
    assert_eq!(
        report.attributions[2].contract_section(),
        Some(ContractSection::FailsWhen)
    );
    assert_eq!(report.attributions[2].suspect.procedure, fails_proc);
    assert!(
        report
            .attributions
            .iter()
            .all(|item| !item.evidence.is_empty())
    );
    assert!(report.attributions.iter().all(|item| {
        item.limitations
            .contains(&AttributionLimitation::ContractViolationNotSoleCause)
    }));
    assert_eq!(report.attribution_cost, 3.0);
    assert_eq!(report.total_cost, 23.0);
    assert!((report.attribution_cost_ratio - (3.0 / 23.0)).abs() < f64::EPSILON);
}

#[test]
fn statistical_attribution_ranks_suspicion_and_marks_cooccurrence_uncertainty() {
    let likely_fault = ProcedureId::new();
    let correlated_a = ProcedureId::new();
    let correlated_b = ProcedureId::new();
    let mut history = Vec::new();

    for index in 0..10 {
        history.push(episode_with_trace(
            ExecTrace {
                steps: vec![step(
                    likely_fault,
                    1,
                    ExecStepStatus::Succeeded,
                    ContractChecks::default(),
                )],
            },
            index < 8,
            index,
        ));
    }
    for index in 10..20 {
        history.push(episode_with_trace(
            ExecTrace {
                steps: vec![
                    step(
                        correlated_a,
                        3,
                        ExecStepStatus::Succeeded,
                        ContractChecks::default(),
                    ),
                    step(
                        correlated_b,
                        5,
                        ExecStepStatus::Succeeded,
                        ContractChecks::default(),
                    ),
                ],
            },
            index < 15,
            index,
        ));
    }

    let failed = episode_with_trace(
        ExecTrace {
            steps: vec![
                step(
                    correlated_a,
                    3,
                    ExecStepStatus::Succeeded,
                    ContractChecks::default(),
                ),
                step(
                    likely_fault,
                    1,
                    ExecStepStatus::Failed {
                        error: "injected".into(),
                    },
                    ContractChecks::default(),
                ),
                step(
                    correlated_b,
                    5,
                    ExecStepStatus::Succeeded,
                    ContractChecks::default(),
                ),
            ],
        },
        true,
        30,
    );

    let ranked = rank_statistical_suspects(&failed, &history).unwrap();

    assert_eq!(ranked.len(), 3);
    assert_eq!(ranked[0].suspect.procedure, likely_fault);
    assert_eq!(
        ranked[0].mechanism,
        AttributionMechanism::StatisticalSuspicion
    );
    assert_eq!(ranked[0].confidence, AttributionConfidence::Low);
    assert!(!ranked[0].decisive);
    assert_eq!(ranked[0].statistical_counts(), Some((10, 8)));

    let correlated = ranked
        .iter()
        .filter(|item| [correlated_a, correlated_b].contains(&item.suspect.procedure))
        .collect::<Vec<_>>();
    assert_eq!(correlated.len(), 2);
    assert!(
        correlated
            .iter()
            .all(|item| item.cooccurrence() == Some(1.0))
    );
    assert!(correlated.iter().all(|item| item.uncertainty() >= 0.5));
    assert!(correlated.iter().all(|item| item.score < ranked[0].score));
    assert!(
        ranked
            .iter()
            .all(|item| item.provenance.episode_ids.len() == 10)
    );
}

#[test]
fn statistical_raw_cost_curve_tracks_history_and_trace_scale() {
    let procedure = ProcedureId::new();
    let trace = ExecTrace {
        steps: vec![step(
            procedure,
            1,
            ExecStepStatus::Succeeded,
            ContractChecks::default(),
        )],
    };
    let failed = episode_with_trace(trace.clone(), true, 100);
    let small = vec![episode_with_trace(trace.clone(), true, 1)];
    let large = (0..10)
        .map(|index| episode_with_trace(trace.clone(), index % 2 == 0, index))
        .collect::<Vec<_>>();

    let small_report = rank_statistical_suspects_with_cost(&failed, &small).unwrap();
    let large_report = rank_statistical_suspects_with_cost(&failed, &large).unwrap();

    assert_eq!(small_report.cost.failed_trace_steps_scanned, 1);
    assert_eq!(small_report.cost.history_episodes_considered, 1);
    assert_eq!(small_report.cost.history_trace_steps_scanned, 1);
    assert_eq!(small_report.cost.element_exposures_counted, 1);
    assert_eq!(small_report.cost.work_units, 4);
    assert_eq!(large_report.cost.history_episodes_considered, 10);
    assert_eq!(large_report.cost.history_trace_steps_scanned, 10);
    assert_eq!(large_report.cost.element_exposures_counted, 10);
    assert_eq!(large_report.cost.work_units, 31);
    assert!(large_report.cost.work_units > small_report.cost.work_units);
}

#[derive(Default)]
struct FakeReplayer {
    observations: Vec<ReplayObservation>,
    requests: Vec<ReplayRequest>,
}

impl CounterfactualReplayer for FakeReplayer {
    type Error = String;

    fn replay(&mut self, request: ReplayRequest) -> Result<ReplayObservation, Self::Error> {
        self.requests.push(request);
        if self.observations.is_empty() {
            return Err("no observation".into());
        }
        Ok(self.observations.remove(0))
    }
}

fn candidate(
    procedure: ProcedureId,
    step: usize,
    mode: CounterfactualMode,
) -> CounterfactualCandidate {
    CounterfactualCandidate {
        suspect: Suspect {
            procedure,
            version: 1,
            trace_step: step,
        },
        prior_score: 0.8,
        change: CounterfactualChange {
            description: format!("change {step}"),
            replacement: serde_json::json!({ "variant": step }),
        },
        mode,
    }
}

#[test]
fn counterfactual_replay_is_one_change_top_k_and_budget_bounded() {
    let source = failed_source();
    let candidates = vec![
        candidate(ProcedureId::new(), 0, CounterfactualMode::Deterministic),
        candidate(ProcedureId::new(), 1, CounterfactualMode::Deterministic),
        candidate(ProcedureId::new(), 2, CounterfactualMode::Deterministic),
    ];
    let mut replayer = FakeReplayer {
        observations: vec![
            ReplayObservation {
                outcome: ReplayOutcome::Failed,
                steps_used: 3,
                details: "still fails".into(),
                provenance: ReplayProvenance::default(),
            },
            ReplayObservation {
                outcome: ReplayOutcome::Succeeded,
                steps_used: 2,
                details: "fault disappears".into(),
                provenance: ReplayProvenance {
                    source_trace_hash: Some("trace-hash".into()),
                    mutation_hash: Some("mutation-hash".into()),
                    verification: Some(ReplayVerificationProvenance::Deterministic {
                        verifier: "pinned-oracle:v1".into(),
                    }),
                },
            },
        ],
        requests: Vec::new(),
    };

    let report = run_counterfactual_replays(
        &source,
        &candidates,
        ReplayBudget {
            top_k: 3,
            max_replays: 2,
            max_steps: 5,
            total_episode_cost: 20.0,
        },
        &mut replayer,
    )
    .unwrap();

    assert_eq!(replayer.requests.len(), 2);
    assert_eq!(replayer.requests[0].change.description, "change 0");
    assert_eq!(replayer.requests[1].change.description, "change 1");
    assert_eq!(replayer.requests[0].step_budget, 5);
    assert_eq!(replayer.requests[1].step_budget, 2);
    assert_eq!(report.replays_run, 2);
    assert_eq!(report.steps_spent, 5);
    assert_eq!(report.stop_reason, Some(BudgetStopReason::ReplayLimit));
    assert_eq!(report.attributions.len(), 2);
    assert!(!report.attributions[0].decisive);
    assert!(!report.attributions[1].decisive);
    assert_eq!(
        report.attributions[1].confidence,
        AttributionConfidence::Medium
    );
    assert_eq!(
        report.attributions[1].mechanism,
        AttributionMechanism::CounterfactualReplay
    );
    assert_eq!(report.total_cost, 25.0);
    assert_eq!(report.attribution_cost_ratio, 0.2);
    assert!(
        report
            .attributions
            .iter()
            .all(|item| item.attribution_cost_ratio == 0.2)
    );
    let AttributionEvidence::Replay { provenance, .. } = &report.attributions[1].evidence[0] else {
        panic!("expected replay evidence");
    };
    assert_eq!(provenance.source_trace_hash.as_deref(), Some("trace-hash"));
    assert_eq!(provenance.mutation_hash.as_deref(), Some("mutation-hash"));
}

#[test]
fn simulated_replay_without_a_trusted_receipt_is_inconclusive_and_top_k_stops_selection() {
    let source = failed_source();
    let candidates = vec![
        candidate(ProcedureId::new(), 0, CounterfactualMode::Simulated),
        candidate(ProcedureId::new(), 1, CounterfactualMode::Deterministic),
    ];
    let mut replayer = FakeReplayer {
        observations: vec![ReplayObservation {
            outcome: ReplayOutcome::Succeeded,
            steps_used: 4,
            details: "simulation improves outcome".into(),
            provenance: ReplayProvenance {
                source_trace_hash: Some("source".into()),
                mutation_hash: Some("simulated-change".into()),
                verification: Some(ReplayVerificationProvenance::Simulated {
                    receipt_id: None,
                    model_id: "kitchen-world".into(),
                    model_version: "1".into(),
                    assumptions: vec!["pan heat held constant".into()],
                }),
            },
        }],
        requests: Vec::new(),
    };

    let report = run_counterfactual_replays(
        &source,
        &candidates,
        ReplayBudget {
            top_k: 1,
            max_replays: 5,
            max_steps: 10,
            total_episode_cost: 40.0,
        },
        &mut replayer,
    )
    .unwrap();

    assert_eq!(replayer.requests.len(), 1);
    assert_eq!(report.stop_reason, Some(BudgetStopReason::TopK));
    assert_eq!(
        report.attributions[0].confidence,
        AttributionConfidence::Inconclusive
    );
    assert!(!report.attributions[0].decisive);
    assert!(report.attributions[0].limitations.iter().any(|limitation| {
        matches!(
            limitation,
            AttributionLimitation::UnverifiedReplayProvenance { reason }
                if reason.contains("trusted simulator receipt")
        )
    }));
    assert_eq!(report.total_cost, 44.0);
    assert!((report.attribution_cost_ratio - (4.0 / 44.0)).abs() < f64::EPSILON);
    let AttributionEvidence::Replay { provenance, .. } = &report.attributions[0].evidence[0] else {
        panic!("expected replay evidence");
    };
    assert!(matches!(
        provenance.verification,
        Some(ReplayVerificationProvenance::Simulated { .. })
    ));
}

#[test]
fn replay_budget_zero_performs_no_work() {
    let source = failed_source();
    let candidates = vec![candidate(
        ProcedureId::new(),
        0,
        CounterfactualMode::Deterministic,
    )];
    let mut replayer = FakeReplayer::default();

    let report = run_counterfactual_replays(
        &source,
        &candidates,
        ReplayBudget {
            top_k: 1,
            max_replays: 1,
            max_steps: 0,
            total_episode_cost: 10.0,
        },
        &mut replayer,
    )
    .unwrap();

    assert!(replayer.requests.is_empty());
    assert_eq!(report.stop_reason, Some(BudgetStopReason::StepLimit));
    assert_eq!(report.attribution_cost_ratio, 0.0);
}

#[test]
fn non_replayability_and_joint_responsibility_limits_remain_observable() {
    let source = failed_source();
    let candidates = vec![
        candidate(ProcedureId::new(), 0, CounterfactualMode::Deterministic),
        candidate(ProcedureId::new(), 1, CounterfactualMode::Deterministic),
    ];
    let mut replayer = FakeReplayer {
        observations: vec![ReplayObservation {
            outcome: ReplayOutcome::NotReplayable {
                reason: "external state changed".into(),
            },
            steps_used: 0,
            details: "effect cannot be reproduced".into(),
            provenance: ReplayProvenance::default(),
        }],
        requests: Vec::new(),
    };

    let report = run_counterfactual_replays(
        &source,
        &candidates,
        ReplayBudget {
            top_k: 1,
            max_replays: 1,
            max_steps: 10,
            total_episode_cost: 20.0,
        },
        &mut replayer,
    )
    .unwrap();

    let attribution = &report.attributions[0];
    assert_eq!(attribution.confidence, AttributionConfidence::Inconclusive);
    assert!(!attribution.decisive);
    assert!(attribution.limitations.iter().any(|limitation| matches!(
        limitation,
        AttributionLimitation::NotReplayable { reason }
            if reason == "external state changed"
    )));
    assert!(attribution.limitations.iter().any(|limitation| matches!(
        limitation,
        AttributionLimitation::SingleChangeCannotDetectInteractions { candidate_count: 2 }
    )));
}

#[test]
fn statistical_candidates_are_unique_per_versioned_procedure() {
    let procedure = ProcedureId::new();
    let failed = episode_with_trace(
        ExecTrace {
            steps: vec![
                step(
                    procedure,
                    1,
                    ExecStepStatus::Succeeded,
                    ContractChecks::default(),
                ),
                step(
                    procedure,
                    1,
                    ExecStepStatus::Failed { error: "x".into() },
                    ContractChecks::default(),
                ),
            ],
        },
        true,
        1,
    );

    let ranked = rank_statistical_suspects(&failed, std::slice::from_ref(&failed)).unwrap();
    let unique = ranked
        .iter()
        .map(|item| (item.suspect.procedure, item.suspect.version))
        .collect::<HashSet<_>>();
    assert_eq!(ranked.len(), unique.len());
}

#[test]
fn statistics_deduplicate_episodes_skip_untraced_history_and_weight_weak_evidence() {
    let procedure = ProcedureId::new();
    let trace = ExecTrace {
        steps: vec![step(
            procedure,
            1,
            ExecStepStatus::Failed {
                error: "injected".into(),
            },
            ContractChecks::default(),
        )],
    };
    let failed = episode_with_trace(trace.clone(), true, 1);
    let mut weak = episode_with_trace(trace, true, 2);
    weak.evaluation.as_mut().unwrap().tier = VerifiabilityTier::Deferred;
    let mut untraced = Episode::new("evaluated but not executable");
    untraced.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: false,
        details: "external failure".into(),
        surprise: None,
    });

    let ranked = rank_statistical_suspects(&failed, &[weak.clone(), weak, untraced]).unwrap();
    let AttributionEvidence::Statistics {
        exposures,
        failures,
        weighted_exposure,
        weighted_failures,
        ..
    } = ranked[0].evidence[0]
    else {
        panic!("expected statistical evidence");
    };

    assert_eq!((exposures, failures), (1, 1));
    assert!((weighted_exposure - 0.2).abs() < f64::EPSILON);
    assert!((weighted_failures - 0.2).abs() < f64::EPSILON);
}

#[test]
fn successful_replay_without_verified_provenance_is_inconclusive() {
    let source = failed_source();
    let candidates = vec![candidate(
        ProcedureId::new(),
        0,
        CounterfactualMode::Deterministic,
    )];
    let mut replayer = FakeReplayer {
        observations: vec![ReplayObservation {
            outcome: ReplayOutcome::Succeeded,
            steps_used: 1,
            details: "caller claims success".into(),
            provenance: ReplayProvenance::default(),
        }],
        requests: Vec::new(),
    };

    let report = run_counterfactual_replays(
        &source,
        &candidates,
        ReplayBudget {
            top_k: 1,
            max_replays: 1,
            max_steps: 2,
            total_episode_cost: 10.0,
        },
        &mut replayer,
    )
    .unwrap();

    assert_eq!(
        report.attributions[0].confidence,
        AttributionConfidence::Inconclusive
    );
    assert!(!report.attributions[0].decisive);
    assert!(
        report.attributions[0]
            .limitations
            .iter()
            .any(|item| matches!(
                item,
                AttributionLimitation::UnverifiedReplayProvenance { .. }
            ))
    );
}

#[test]
fn materialized_aggregate_ranking_matches_full_history_semantics() {
    let store = EpisodeStore::in_memory().unwrap();
    let left = ProcedureId::new();
    let right = ProcedureId::new();
    let history = vec![
        episode_with_trace(
            ExecTrace {
                steps: vec![
                    step(
                        left,
                        1,
                        ExecStepStatus::Succeeded,
                        ContractChecks::default(),
                    ),
                    step(
                        right,
                        2,
                        ExecStepStatus::Succeeded,
                        ContractChecks::default(),
                    ),
                ],
            },
            true,
            1,
        ),
        episode_with_trace(
            ExecTrace {
                steps: vec![step(
                    left,
                    1,
                    ExecStepStatus::Succeeded,
                    ContractChecks::default(),
                )],
            },
            false,
            2,
        ),
        episode_with_trace(
            ExecTrace {
                steps: vec![
                    step(
                        left,
                        1,
                        ExecStepStatus::Succeeded,
                        ContractChecks::default(),
                    ),
                    step(
                        right,
                        2,
                        ExecStepStatus::Succeeded,
                        ContractChecks::default(),
                    ),
                ],
            },
            true,
            3,
        ),
    ];
    for episode in &history {
        store.insert(episode).unwrap();
    }
    let source = &history[2];
    let snapshot = store
        .credit_aggregate_snapshot(
            &[
                CreditElementRef {
                    procedure: left,
                    version: 1,
                },
                CreditElementRef {
                    procedure: right,
                    version: 2,
                },
            ],
            source.id,
        )
        .unwrap();

    let scanned = rank_statistical_suspects_with_cost(source, &history).unwrap();
    let indexed = rank_statistical_suspects_from_aggregates(source, &snapshot).unwrap();
    assert_eq!(indexed.attributions.len(), scanned.attributions.len());
    for (indexed, scanned) in indexed.attributions.iter().zip(&scanned.attributions) {
        assert_eq!(indexed.suspect, scanned.suspect);
        assert_eq!(indexed.score, scanned.score);
        assert_eq!(
            serde_json::to_value(&indexed.evidence).unwrap(),
            serde_json::to_value(&scanned.evidence).unwrap()
        );
        assert_eq!(indexed.provenance.details, scanned.provenance.details);
        assert_eq!(
            indexed
                .provenance
                .episode_ids
                .iter()
                .copied()
                .collect::<HashSet<_>>(),
            scanned
                .provenance
                .episode_ids
                .iter()
                .copied()
                .collect::<HashSet<_>>()
        );
    }
    assert_eq!(indexed.cost.history_trace_steps_scanned, 0);
    assert_eq!(indexed.cost.aggregate_rows_read, 2);
    assert_eq!(indexed.cost.pair_aggregate_rows_read, 1);
}
