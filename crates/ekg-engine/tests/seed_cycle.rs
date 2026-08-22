use std::collections::BTreeMap;

use ekg_core::{
    BinOp, Concept, Condition, Contract, Expr, MutabilityClass, Param, Procedure, TraceStepStatus,
    Value,
};
use ekg_engine::{Engine, EngineError};

fn double_procedure() -> Procedure {
    let requires = Condition::described("x is non-negative").with_check(Expr::BinOp {
        op: BinOp::Ge,
        left: Box::new(Expr::Var("x".into())),
        right: Box::new(Expr::Literal(Value::Int(0))),
    });
    let promises = Condition::described("result is double x").with_check(Expr::BinOp {
        op: BinOp::Eq,
        left: Box::new(Expr::Var("result".into())),
        right: Box::new(Expr::BinOp {
            op: BinOp::Mul,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(2))),
        }),
    });

    Procedure::new(
        "DOUBLE",
        vec![Param::named("x")],
        Expr::BinOp {
            op: BinOp::Mul,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(2))),
        },
    )
    .with_contract(Contract {
        requires: vec![requires],
        promises: vec![promises],
        ..Contract::default()
    })
}

fn inputs(x: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([("x".into(), Value::Int(x))])
}

#[test]
fn execution_records_and_evaluates_a_complete_episode() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double_procedure();
    engine.admin_insert_procedure(&procedure).unwrap();

    let outcome = engine
        .execute_procedure(procedure.id, inputs(7), Some(Value::Int(14)))
        .unwrap();

    assert_eq!(outcome.value, Value::Int(14));
    assert_eq!(outcome.trace.steps.len(), 1);
    assert_eq!(outcome.trace.steps[0].procedure_version, Some(1));
    assert!(outcome.episode.succeeded());
    assert_eq!(outcome.episode.prediction, Some(Value::Int(14)));
    assert_eq!(outcome.episode.observed_result, Some(Value::Int(14)));

    let stored = engine.episodes().get(outcome.episode.id).unwrap();
    assert_eq!(stored.id, outcome.episode.id);
    let regressions = engine
        .episodes()
        .list_verified_regression_cases(procedure.id, procedure.version)
        .unwrap();
    assert_eq!(regressions.len(), 1);
    assert_eq!(regressions[0].test_case.expected_output, Value::Int(14));
    assert_eq!(
        regressions[0].test_case.from_episode,
        Some(outcome.episode.id)
    );
}

#[test]
fn replay_uses_pinned_versions_and_named_substitutions() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double_procedure();
    engine.admin_insert_procedure(&procedure).unwrap();
    let original = engine
        .execute_procedure(procedure.id, inputs(7), Some(Value::Int(14)))
        .unwrap();

    let replayed = engine
        .replay_episode(original.episode.id, inputs(9))
        .unwrap();

    assert_eq!(replayed.value, Value::Int(18));
    assert_eq!(replayed.trace.steps[0].procedure_version, Some(1));
    assert_eq!(replayed.source_episode, original.episode.id);
}

#[test]
fn execution_rejects_missing_and_extra_named_inputs() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double_procedure();
    engine.admin_insert_procedure(&procedure).unwrap();

    let missing = engine.execute_procedure(procedure.id, BTreeMap::new(), None);
    assert!(
        missing
            .unwrap_err()
            .to_string()
            .contains("missing input 'x'")
    );

    let extra = engine.execute_procedure(
        procedure.id,
        BTreeMap::from([("x".into(), Value::Int(2)), ("y".into(), Value::Int(3))]),
        None,
    );
    assert!(
        extra
            .unwrap_err()
            .to_string()
            .contains("unexpected input 'y'")
    );
}

#[test]
fn execution_episode_is_indexed_by_the_procedure_concept() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("DOUBLE", MutabilityClass::Definitional);
    engine.admin_insert_concept(&concept).unwrap();
    let procedure = double_procedure().with_concept(concept.id);
    engine.admin_insert_procedure(&procedure).unwrap();

    let outcome = engine
        .execute_procedure(procedure.id, inputs(4), None)
        .unwrap();
    let found = engine.episodes().find_by_concept(concept.id).unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, outcome.episode.id);
}

#[test]
fn failed_execution_records_the_episode_and_partial_trace() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double_procedure();
    engine.admin_insert_procedure(&procedure).unwrap();

    let error = engine
        .execute_procedure(procedure.id, inputs(-1), Some(Value::Int(-2)))
        .unwrap_err();
    let EngineError::ExecutionFailed { episode_id, .. } = error else {
        panic!("expected a recorded execution failure");
    };
    let stored = engine.episodes().get(episode_id).unwrap();

    assert!(stored.failed());
    assert!(stored.observed_result.is_none());
    assert_eq!(stored.evaluation.as_ref().unwrap().surprise, Some(1.0));
    let reasoning_step = &stored.reasoning_trace.steps[0];
    assert!(
        !reasoning_step
            .contract_check
            .as_ref()
            .unwrap()
            .all_requires_met
    );
    assert!(
        reasoning_step
            .contract_check
            .as_ref()
            .unwrap()
            .all_promises_met
    );
    assert!(reasoning_step.output.is_none());
    assert!(matches!(
        reasoning_step.status,
        TraceStepStatus::Failed { .. }
    ));
    let trace = stored.execution_trace.unwrap();
    assert_eq!(
        trace["steps"][0]["status"]["Failed"]["error"],
        "contract violation: requires condition violated: x is non-negative"
    );
}

#[test]
fn failed_execution_without_prediction_has_no_surprise_signal() {
    let engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let procedure = double_procedure();
    engine.admin_insert_procedure(&procedure).unwrap();

    let error = engine
        .execute_procedure(procedure.id, inputs(-1), None)
        .unwrap_err();
    let EngineError::ExecutionFailed { episode_id, .. } = error else {
        panic!("expected a recorded execution failure");
    };
    let stored = engine.episodes().get(episode_id).unwrap();

    assert_eq!(stored.evaluation.unwrap().surprise, None);
}
