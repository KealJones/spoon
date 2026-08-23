use std::collections::{BTreeMap, HashSet};

use spoon_core::{BinOp, Condition, Expr, Param, Procedure, ProcedureId, Value};
use spoon_credit::{
    AttributionConfidence, CounterfactualCandidate, CounterfactualChange, CounterfactualMode,
    Suspect,
};
use spoon_engine::{
    CounterfactualMutation, Engine, FailureAnalysis, FailureAnalysisBudget, FailureAnalysisRequest,
    ProcedureVersionRef, ReplayVerification,
};

const METRIC_SEED: u64 = 0x05ee_dcaf_ed15_ca11;

#[derive(Debug, Clone, Copy)]
enum ExpectedResult {
    Culprit(ProcedureId),
    Abstain,
}

#[derive(Debug, Clone, Copy)]
enum FaultPlacement {
    Leaf,
    Middle,
    Root,
}

#[derive(Debug)]
struct CaseResult {
    family: &'static str,
    held_out: bool,
    expected: ExpectedResult,
    predicted: Option<ProcedureId>,
    rank: Option<usize>,
}

#[derive(Debug)]
struct MetricReport {
    evaluated: usize,
    covered: usize,
    abstentions: usize,
    top_1: usize,
    mean_rank: f64,
    mrr: f64,
    held_out_top_1: usize,
    held_out_evaluated: usize,
    tune_top_1: usize,
    tune_evaluated: usize,
    failures: Vec<&'static str>,
}

fn inputs(value: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([("x".into(), Value::Int(value))])
}

fn identity(name: impl Into<String>) -> Procedure {
    Procedure::new(name, vec![Param::named("x")], Expr::Var("x".into()))
}

fn rejecting_contract(value: i64) -> Condition {
    Condition::described(format!("injected fault rejects {value}")).with_check(Expr::BinOp {
        op: BinOp::Ne,
        left: Box::new(Expr::Var("x".into())),
        right: Box::new(Expr::Literal(Value::Int(value))),
    })
}

fn candidate_for(procedure: &Procedure, trace_step: usize) -> CounterfactualCandidate {
    CounterfactualCandidate {
        suspect: Suspect {
            procedure: procedure.id,
            version: 1,
            trace_step,
        },
        prior_score: 1_000_000.0,
        change: CounterfactualChange {
            description: "harness decoy mutation, never the injected answer".into(),
            replacement: serde_json::to_value(CounterfactualMutation::ReplaceBody {
                target: ProcedureVersionRef {
                    id: procedure.id,
                    version: 1,
                },
                body: Expr::Literal(Value::Int(-999)),
                verification: ReplayVerification::DeterministicExpected {
                    expected: Value::Int(-999),
                },
            })
            .unwrap(),
        },
        mode: CounterfactualMode::Deterministic,
    }
}

fn distinct_ranked_procedures(analysis: &FailureAnalysis) -> Vec<ProcedureId> {
    let mut seen = HashSet::new();
    analysis
        .ranked
        .iter()
        .filter(|attribution| seen.insert(attribution.suspect.procedure))
        .filter(|attribution| attribution.confidence >= AttributionConfidence::High)
        .map(|attribution| attribution.suspect.procedure)
        .collect()
}

fn contract_case(
    family: &'static str,
    held_out: bool,
    placement: FaultPlacement,
    include_noncausal_decoy: bool,
    fault_value: i64,
) -> CaseResult {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let decoy = identity(format!("{family}_CORRELATED_DECOY"));
    let mut leaf = identity(format!("{family}_LEAF"));
    let mut middle = Procedure::new(
        format!("{family}_MIDDLE"),
        vec![Param::named("x")],
        Expr::Call {
            procedure: leaf.id,
            args: vec![Expr::Var("x".into())],
        },
    );
    let mut root = Procedure::new(
        format!("{family}_ROOT"),
        vec![Param::named("x")],
        Expr::Block(vec![
            Expr::Call {
                procedure: decoy.id,
                args: vec![Expr::Var("x".into())],
            },
            Expr::Call {
                procedure: middle.id,
                args: vec![Expr::Var("x".into())],
            },
        ]),
    );
    match placement {
        FaultPlacement::Leaf => leaf.contract.requires.push(rejecting_contract(fault_value)),
        FaultPlacement::Middle => middle
            .contract
            .requires
            .push(rejecting_contract(fault_value)),
        FaultPlacement::Root => root.contract.requires.push(rejecting_contract(fault_value)),
    }
    for procedure in [&decoy, &leaf, &middle, &root] {
        engine.admin_insert_procedure(procedure).unwrap();
    }
    for value in [1, 2, 3, 4, 5, 6] {
        if value != fault_value {
            engine
                .execute_procedure(root.id, inputs(value), Some(Value::Int(value)))
                .unwrap();
        }
    }
    let failure = engine
        .execute_procedure(root.id, inputs(fault_value), Some(Value::Int(fault_value)))
        .unwrap_err();
    let spoon_engine::EngineError::ExecutionFailed { episode_id, .. } = failure else {
        panic!("injected contract must fail");
    };
    let candidates = include_noncausal_decoy
        .then(|| candidate_for(&decoy, 0))
        .into_iter()
        .collect();
    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id,
            selected_feedback_id: None,
            candidates,
            budget: FailureAnalysisBudget {
                top_k: 3,
                max_replays: 3,
                max_replay_steps: 100,
            },
        })
        .unwrap();
    let truth = match placement {
        FaultPlacement::Leaf => leaf.id,
        FaultPlacement::Middle => middle.id,
        FaultPlacement::Root => root.id,
    };
    let ranked = distinct_ranked_procedures(&analysis);
    CaseResult {
        family,
        held_out,
        expected: ExpectedResult::Culprit(truth),
        predicted: ranked.first().copied(),
        rank: ranked
            .iter()
            .position(|id| *id == truth)
            .map(|index| index + 1),
    }
}

fn interaction_nonreplayable_case(fault_value: i64) -> CaseResult {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let correlated = identity("INTERACTION_CORRELATED_ELEMENT");
    let root = Procedure::new(
        "INTERACTION_ROOT",
        vec![Param::named("x")],
        Expr::Block(vec![
            Expr::Call {
                procedure: correlated.id,
                args: vec![Expr::Var("x".into())],
            },
            Expr::Call {
                procedure: correlated.id,
                args: vec![Expr::Var("x".into())],
            },
        ]),
    );
    engine.admin_insert_procedure(&correlated).unwrap();
    engine.admin_insert_procedure(&root).unwrap();
    let failed = engine
        .execute_procedure(root.id, inputs(fault_value), Some(Value::Int(999)))
        .unwrap()
        .episode;
    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![candidate_for(&correlated, 0)],
            budget: FailureAnalysisBudget::default(),
        })
        .unwrap();
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
    CaseResult {
        family: "interaction_nonreplayable",
        held_out: true,
        expected: ExpectedResult::Abstain,
        predicted: distinct_ranked_procedures(&analysis).first().copied(),
        rank: None,
    }
}

fn deterministic_body_fault_case() -> CaseResult {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut leaf = identity("BODY_FAULT_LEAF");
    leaf.body = Expr::BinOp {
        op: BinOp::Add,
        left: Box::new(Expr::Var("x".into())),
        right: Box::new(Expr::Literal(Value::Int(1))),
    };
    let root = Procedure::new(
        "BODY_FAULT_ROOT",
        vec![Param::named("x")],
        Expr::Call {
            procedure: leaf.id,
            args: vec![Expr::Var("x".into())],
        },
    );
    engine.admin_insert_procedure(&leaf).unwrap();
    engine.admin_insert_procedure(&root).unwrap();
    let failed = engine
        .execute_procedure(root.id, inputs(41), Some(Value::Int(41)))
        .unwrap()
        .episode;
    assert!(!failed.succeeded());
    let candidate = CounterfactualCandidate {
        suspect: Suspect {
            procedure: leaf.id,
            version: 1,
            trace_step: 0,
        },
        prior_score: 1_000_000.0,
        change: CounterfactualChange {
            description: "replace the faulty leaf operator with identity".into(),
            replacement: serde_json::to_value(CounterfactualMutation::ReplaceBody {
                target: ProcedureVersionRef {
                    id: leaf.id,
                    version: 1,
                },
                body: Expr::Var("x".into()),
                verification: ReplayVerification::DeterministicExpected {
                    expected: Value::Int(41),
                },
            })
            .unwrap(),
        },
        mode: CounterfactualMode::Deterministic,
    };
    let analysis = engine
        .analyze_failure(FailureAnalysisRequest {
            episode_id: failed.id,
            selected_feedback_id: None,
            candidates: vec![candidate],
            budget: FailureAnalysisBudget {
                top_k: 2,
                max_replays: 2,
                max_replay_steps: 100,
            },
        })
        .unwrap();
    let ranked = distinct_ranked_procedures(&analysis);
    CaseResult {
        family: "heldout_deterministic_body_operator",
        held_out: true,
        expected: ExpectedResult::Culprit(leaf.id),
        predicted: ranked.first().copied(),
        rank: ranked
            .iter()
            .position(|id| *id == leaf.id)
            .map(|index| index + 1),
    }
}

fn summarize(results: &[CaseResult]) -> MetricReport {
    let evaluated = results.len();
    let covered = results
        .iter()
        .filter(|result| result.predicted.is_some())
        .count();
    let abstentions = evaluated - covered;
    let localized = results
        .iter()
        .filter_map(|result| match result.expected {
            ExpectedResult::Culprit(culprit) => Some((result, culprit)),
            ExpectedResult::Abstain => None,
        })
        .collect::<Vec<_>>();
    let top_1 = localized
        .iter()
        .filter(|(result, culprit)| result.predicted == Some(*culprit))
        .count();
    let ranks = localized
        .iter()
        .filter_map(|(result, _)| result.rank)
        .collect::<Vec<_>>();
    let mean_rank = ranks.iter().sum::<usize>() as f64 / ranks.len() as f64;
    let mrr = ranks.iter().map(|rank| 1.0 / *rank as f64).sum::<f64>() / localized.len() as f64;
    let held_out = localized
        .iter()
        .filter(|(result, _)| result.held_out)
        .collect::<Vec<_>>();
    let held_out_top_1 = held_out
        .iter()
        .filter(|(result, culprit)| result.predicted == Some(*culprit))
        .count();
    let tune = localized
        .iter()
        .filter(|(result, _)| !result.held_out)
        .collect::<Vec<_>>();
    let tune_top_1 = tune
        .iter()
        .filter(|(result, culprit)| result.predicted == Some(*culprit))
        .count();
    let failures = localized
        .iter()
        .filter_map(|(result, culprit)| {
            (result.predicted != Some(*culprit)).then_some(result.family)
        })
        .collect();
    MetricReport {
        evaluated,
        covered,
        abstentions,
        top_1,
        mean_rank,
        mrr,
        held_out_top_1,
        held_out_evaluated: held_out.len(),
        tune_top_1,
        tune_evaluated: tune.len(),
        failures,
    }
}

#[test]
fn seeded_injected_fault_metric_reports_accuracy_coverage_and_honest_abstention() {
    let offsets = [
        (METRIC_SEED & 0x7) as i64 + 10,
        ((METRIC_SEED >> 4) & 0x7) as i64 + 20,
        ((METRIC_SEED >> 8) & 0x7) as i64 + 30,
        ((METRIC_SEED >> 12) & 0x7) as i64 + 40,
        ((METRIC_SEED >> 16) & 0x7) as i64 + 50,
        ((METRIC_SEED >> 20) & 0x7) as i64 + 60,
    ];
    let results = vec![
        contract_case(
            "tune_nested_leaf_with_correlated_decoy",
            false,
            FaultPlacement::Leaf,
            true,
            offsets[0],
        ),
        contract_case(
            "tune_nested_middle",
            false,
            FaultPlacement::Middle,
            false,
            offsets[1],
        ),
        contract_case(
            "tune_root_contract",
            false,
            FaultPlacement::Root,
            true,
            offsets[2],
        ),
        contract_case(
            "heldout_nested_middle_with_correlated_decoy",
            true,
            FaultPlacement::Middle,
            true,
            offsets[3],
        ),
        contract_case(
            "heldout_nested_leaf",
            true,
            FaultPlacement::Leaf,
            true,
            offsets[4],
        ),
        contract_case(
            "heldout_root_contract",
            true,
            FaultPlacement::Root,
            false,
            offsets[5],
        ),
        interaction_nonreplayable_case(offsets[5] + 100),
        deterministic_body_fault_case(),
    ];
    let report = summarize(&results);

    eprintln!("metric7 failures: {:?}", report.failures);

    assert_eq!(report.evaluated, 8, "{results:#?}");
    assert_eq!(report.covered, 7, "{results:#?}");
    assert_eq!(report.abstentions, 1, "{results:#?}");
    assert_eq!(report.top_1, 7, "{results:#?}");
    assert_eq!(report.mean_rank, 1.0, "{results:#?}");
    assert_eq!(report.mrr, 1.0, "{results:#?}");
    assert_eq!(report.tune_top_1, 3, "{results:#?}");
    assert_eq!(report.tune_evaluated, 3, "{results:#?}");
    assert_eq!(report.held_out_top_1, 4, "{results:#?}");
    assert_eq!(report.held_out_evaluated, 4, "{results:#?}");
    assert!(report.failures.is_empty(), "{results:#?}");
    assert!(results.iter().any(|result| {
        result.family == "interaction_nonreplayable"
            && matches!(result.expected, ExpectedResult::Abstain)
            && result.predicted.is_none()
    }));
}
