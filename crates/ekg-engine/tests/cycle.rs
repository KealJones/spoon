use std::collections::BTreeMap;

use ekg_core::{
    BinOp, Concept, Episode, Evaluation, Expr, MutabilityClass, Param, Procedure, Value,
    VerifiabilityTier,
};
use ekg_engine::{
    CycleBudget, CycleDisposition, CycleInput, CycleProgress, Engine, TeacherProposalWire,
};
use ekg_exec::{ExecStepStatus, ExecTrace};
use serde_json::json;

fn cycle_input(situation: &str, teacher_allowed: bool) -> CycleInput {
    CycleInput {
        situation: situation.into(),
        environment: BTreeMap::new(),
        assumptions: Vec::new(),
        budget: CycleBudget {
            max_exec_steps: 1_000,
            max_context_items: 32,
            max_teacher_turns: 1,
        },
        teacher_allowed,
    }
}

fn seed_double(engine: &Engine) -> (Concept, Procedure) {
    let concept = Concept::new("DOUBLE", MutabilityClass::Definitional);
    engine.graph().insert_concept(&concept).unwrap();
    let procedure = Procedure::new(
        "DOUBLE",
        vec![Param::named("x")],
        Expr::BinOp {
            op: BinOp::Mul,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(2))),
        },
    )
    .with_concept(concept.id);
    engine.graph().insert_procedure(&procedure).unwrap();
    (concept, procedure)
}

fn proposal(situation: &str, content: serde_json::Value) -> TeacherProposalWire {
    TeacherProposalWire {
        content,
        source: "claude:test".into(),
        status: "unverified".into(),
        provenance: json!({
            "provider": "claude",
            "teacher": "claude:test",
            "requestId": "test-1",
            "generatedAt": "2026-08-22T00:00:00Z",
            "situation": situation
        }),
        validation: None,
    }
}

#[test]
fn run_matches_a_linked_procedure_without_domain_special_cases() {
    let mut engine = Engine::in_memory().unwrap();
    seed_double(&engine);

    let progress = engine
        .begin_cycle(cycle_input("what is double 7?", true))
        .unwrap();
    let CycleProgress::Completed(outcome) = progress else {
        panic!("known procedure should resolve locally");
    };

    assert_eq!(outcome.disposition, CycleDisposition::Verified);
    assert_eq!(outcome.answer, Some(Value::Int(14)));
    assert_eq!(outcome.episode.situation, "what is double 7?");
    assert_eq!(outcome.episode.cost.rung_reached as u8, 2);
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn unknown_with_teacher_enabled_returns_a_nonterminal_continuation() {
    let mut engine = Engine::in_memory().unwrap();

    let progress = engine
        .begin_cycle(cycle_input("explain the moon", true))
        .unwrap();
    let CycleProgress::NeedTeacher { request, .. } = progress else {
        panic!("unknown input should ask the teacher");
    };

    assert_eq!(request.situation, "explain the moon");
    assert!(request.desired_output.is_object());
    assert_openai_strict_objects(&request.desired_output);
    assert_eq!(engine.episodes().count().unwrap(), 0);
}

fn assert_openai_strict_objects(schema: &serde_json::Value) {
    if schema.get("type").and_then(serde_json::Value::as_str) == Some("object") {
        assert_eq!(schema["additionalProperties"], false);
        let properties = schema["properties"].as_object().unwrap();
        let required = schema["required"].as_array().unwrap();
        assert_eq!(properties.len(), required.len());
        for key in properties.keys() {
            assert!(required.iter().any(|required| required == key));
        }
    }
    if let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        for child in properties.values() {
            assert_openai_strict_objects(child);
        }
    }
    if let Some(items) = schema.get("items") {
        assert_openai_strict_objects(items);
    }
    for keyword in ["anyOf", "oneOf", "allOf"] {
        if let Some(children) = schema.get(keyword).and_then(serde_json::Value::as_array) {
            for child in children {
                assert_openai_strict_objects(child);
            }
        }
    }
}

#[test]
fn teacher_request_context_has_absolute_bounds() {
    let mut engine = Engine::in_memory().unwrap();
    for index in 0..100 {
        let concept = Concept::new(
            format!("CONCEPT_{index}_{}", "x".repeat(4_000)),
            MutabilityClass::Definitional,
        );
        engine.graph().insert_concept(&concept).unwrap();
    }
    let mut input = cycle_input("unknown", true);
    input.budget.max_context_items = 1_024;
    input.environment.insert(
        "oversized".into(),
        Value::List((0..100).map(Value::Int).collect()),
    );

    let progress = engine.begin_cycle(input).unwrap();
    let CycleProgress::NeedTeacher { request, .. } = progress else {
        panic!("unknown input should ask");
    };
    assert_eq!(request.context["concepts"].as_array().unwrap().len(), 64);
    assert_eq!(
        request.context["environment"]["oversized"]
            .as_array()
            .unwrap()
            .len(),
        64
    );
    assert!(serde_json::to_string(&request.context).unwrap().len() < 300_000);
}

#[test]
fn oversized_cycle_context_is_rejected_before_a_continuation_is_stored() {
    let mut engine = Engine::in_memory().unwrap();
    let mut input = cycle_input("unknown", true);
    input.budget.max_context_items = 1_025;

    assert!(engine.begin_cycle(input).is_err());
    assert_eq!(engine.episodes().count().unwrap(), 0);
}

#[test]
fn teacher_disabled_unknown_abstains_and_records_one_episode() {
    let mut engine = Engine::in_memory().unwrap();

    let progress = engine
        .begin_cycle(cycle_input("explain the moon", false))
        .unwrap();
    let CycleProgress::Completed(outcome) = progress else {
        panic!("teacher-disabled unknown input must terminate");
    };

    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert_eq!(outcome.answer, None);
    assert_eq!(outcome.episode.action.as_deref(), Some("abstain"));
    assert_eq!(outcome.episode.cost.rung_reached as u8, 7);
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn teacher_can_resolve_inputs_for_an_existing_procedure() {
    let mut engine = Engine::in_memory().unwrap();
    let (concept, _) = seed_double(&engine);
    let start = engine
        .begin_cycle(cycle_input("please calculate it", true))
        .unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unresolved input should ask");
    };

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "please calculate it",
                json!({
                    "interpretations": [{
                        "concept": { "id": concept.id.to_string() },
                        "weight": 1.0,
                        "inputs": { "x": 7 }
                    }]
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("teacher-resolved intent should execute");
    };

    assert_eq!(outcome.disposition, CycleDisposition::Provisional);
    assert_eq!(outcome.answer, Some(Value::Int(14)));
    assert_eq!(outcome.episode.cost.rung_reached as u8, 6);
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn answer_only_teacher_output_remains_provisional() {
    let mut engine = Engine::in_memory().unwrap();
    let mut input = cycle_input("what is the answer?", true);
    input
        .environment
        .insert("topic".into(), Value::Text("life".into()));
    input.assumptions.push(ekg_core::Assumption {
        description: "the question has one answer".into(),
        basis: "assumed".into(),
        concept: None,
    });
    let start = engine.begin_cycle(input).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unknown input should ask");
    };

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "what is the answer?",
                json!({ "interpretations": [], "answer": 42 }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("answer-only proposal should terminalize");
    };

    assert_eq!(outcome.disposition, CycleDisposition::Provisional);
    assert_eq!(outcome.answer, Some(Value::Int(42)));
    assert_eq!(outcome.episode.prediction, Some(Value::Int(42)));
    assert_eq!(outcome.episode.observed_result, None);
    assert!(outcome.episode.evaluation.is_none());
    assert_eq!(
        outcome.episode.context.environment.get("topic"),
        Some(&Value::Text("life".into()))
    );
    assert_eq!(outcome.episode.context.assumptions.len(), 1);
    assert!(outcome.episode.context.budget_remaining.is_some());
}

#[test]
fn ambiguous_local_literals_do_not_get_guessed() {
    let mut engine = Engine::in_memory().unwrap();
    seed_double(&engine);

    let progress = engine
        .begin_cycle(cycle_input("double 7 or 8", false))
        .unwrap();
    let CycleProgress::Completed(outcome) = progress else {
        panic!("teacher-disabled ambiguity must terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert_eq!(outcome.answer, None);
}

#[test]
fn inactive_concepts_and_procedures_are_not_executed() {
    let mut engine = Engine::in_memory().unwrap();
    let (mut concept, mut procedure) = seed_double(&engine);
    concept.lifecycle = ekg_core::Lifecycle::Invalid;
    engine.graph().update_concept(&concept).unwrap();
    procedure.lifecycle = ekg_core::Lifecycle::Retired;
    procedure.version += 1;
    engine.graph().update_procedure(&procedure).unwrap();

    let progress = engine.begin_cycle(cycle_input("double 7", false)).unwrap();
    let CycleProgress::Completed(outcome) = progress else {
        panic!("inactive knowledge must not run");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
}

#[test]
fn exact_verified_history_resolves_at_recall() {
    let mut engine = Engine::in_memory().unwrap();
    let mut prior = Episode::new("what is double 7?");
    prior.observed_result = Some(Value::Int(14));
    prior.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: true,
        details: "verified earlier".into(),
        surprise: Some(0.0),
    });
    engine.episodes().insert(&prior).unwrap();

    let progress = engine
        .begin_cycle(cycle_input("what is double 7?", true))
        .unwrap();
    let CycleProgress::Completed(outcome) = progress else {
        panic!("verified history should recall");
    };

    assert_eq!(outcome.answer, Some(Value::Int(14)));
    assert_eq!(outcome.episode.cost.rung_reached as u8, 1);
    assert_eq!(engine.episodes().count().unwrap(), 2);
}

#[test]
fn a_teacher_continuation_can_only_be_consumed_once() {
    let mut engine = Engine::in_memory().unwrap();
    let start = engine.begin_cycle(cycle_input("unknown", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unknown input should ask");
    };
    let teacher = proposal(
        "unknown",
        json!({ "interpretations": [], "answer": "provisional" }),
    );

    engine.resume_cycle(cycle_id, teacher.clone()).unwrap();
    let second = engine.resume_cycle(cycle_id, teacher);

    assert!(second.unwrap_err().to_string().contains("cycle"));
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn teacher_provenance_must_match_the_pending_request() {
    let mut engine = Engine::in_memory().unwrap();
    let start = engine.begin_cycle(cycle_input("alpha", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unknown input should ask");
    };

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "different situation",
                json!({ "interpretations": [], "answer": "forged" }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("mismatched provenance must terminalize");
    };

    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert_eq!(outcome.answer, None);
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn non_unverified_teacher_status_is_rejected() {
    let mut engine = Engine::in_memory().unwrap();
    let start = engine.begin_cycle(cycle_input("alpha", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unknown input should ask");
    };
    let mut teacher = proposal("alpha", json!({ "interpretations": [], "answer": 1 }));
    teacher.status = "verified".into();

    let resumed = engine.resume_cycle(cycle_id, teacher).unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("invalid status must terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert!(outcome.episode.teacher_interaction.is_some());
}

#[test]
fn rejected_external_validation_cannot_be_integrated() {
    let mut engine = Engine::in_memory().unwrap();
    let start = engine.begin_cycle(cycle_input("alpha", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unknown input should ask");
    };
    let mut teacher = proposal("alpha", json!({ "interpretations": [], "answer": 1 }));
    teacher.validation = Some(json!({
        "status": "rejected",
        "validatedAt": "2026-08-22T00:00:00.000Z",
        "checks": [{
            "validator": "proposal-envelope",
            "status": "rejected",
            "reason": "request fingerprint mismatch"
        }]
    }));

    let resumed = engine.resume_cycle(cycle_id, teacher).unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("rejected validation must terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert_eq!(outcome.answer, None);
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn teacher_procedure_is_provisionally_learned_and_then_runs_locally() {
    let mut engine = Engine::in_memory().unwrap();
    let concept = Concept::new("TRIPLE", MutabilityClass::Definitional);
    engine.graph().insert_concept(&concept).unwrap();
    let procedure = Procedure::new(
        "TRIPLE",
        vec![Param::named("x")],
        Expr::BinOp {
            op: BinOp::Mul,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(3))),
        },
    )
    .with_concept(concept.id);

    let start = engine
        .begin_cycle(cycle_input("what is triple 5?", true))
        .unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("concept without a procedure should ask");
    };
    let learned = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "what is triple 5?",
                json!({
                    "interpretations": [{
                        "concept": { "id": concept.id.to_string() },
                        "weight": 1.0,
                        "inputs": { "x": 5 }
                    }],
                    "procedure": procedure
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(learned) = learned else {
        panic!("executable teacher procedure should complete provisionally");
    };
    assert_eq!(learned.disposition, CycleDisposition::Provisional);
    assert_eq!(learned.answer, Some(Value::Int(15)));
    assert!(learned.episode.teacher_interaction.is_some());
    assert!(
        learned
            .episode
            .reasoning_trace
            .steps
            .windows(2)
            .all(|pair| pair[0].rung <= pair[1].rung)
    );

    let local = engine
        .begin_cycle(cycle_input("please triple 6", true))
        .unwrap();
    let CycleProgress::Completed(local) = local else {
        panic!("learned procedure should run without another teacher call");
    };
    assert_eq!(local.answer, Some(Value::Int(18)));
    assert_eq!(local.disposition, CycleDisposition::Provisional);
    assert_eq!(local.episode.cost.rung_reached as u8, 2);
    assert_eq!(engine.episodes().count().unwrap(), 2);
}

#[test]
fn ambiguous_teacher_interpretations_are_preserved_without_guessing() {
    let mut engine = Engine::in_memory().unwrap();
    let first = Concept::new("FIRST", MutabilityClass::Definitional);
    let second = Concept::new("SECOND", MutabilityClass::Definitional);
    engine.graph().insert_concept(&first).unwrap();
    engine.graph().insert_concept(&second).unwrap();
    let start = engine.begin_cycle(cycle_input("ambiguous", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unknown input should ask");
    };

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "ambiguous",
                json!({
                    "interpretations": [
                        { "concept": { "id": first.id.to_string() }, "weight": 0.5, "inputs": {} },
                        { "concept": { "id": second.id.to_string() }, "weight": 0.5, "inputs": {} }
                    ],
                    "abstainReason": "meaning remains ambiguous"
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("teacher turn budget is exhausted");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert_eq!(outcome.episode.interpretations.len(), 2);
    assert!(
        outcome
            .episode
            .interpretations
            .iter()
            .all(|interpretation| !interpretation.chosen)
    );
}

#[test]
fn failed_execution_records_one_complete_terminal_episode() {
    let mut engine = Engine::in_memory().unwrap();
    let concept = Concept::new("BREAK", MutabilityClass::Definitional);
    engine.graph().insert_concept(&concept).unwrap();
    let procedure = Procedure::new(
        "BREAK",
        vec![Param::named("x")],
        Expr::BinOp {
            op: BinOp::Div,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(0))),
        },
    )
    .with_concept(concept.id);
    engine.graph().insert_procedure(&procedure).unwrap();

    let progress = engine.begin_cycle(cycle_input("break 9", false)).unwrap();
    let CycleProgress::Completed(outcome) = progress else {
        panic!("a failed run must still terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert!(outcome.episode.execution_trace.is_some());
    assert!(
        outcome
            .episode
            .evaluation
            .as_ref()
            .is_some_and(|evaluation| !evaluation.success)
    );
    assert_eq!(outcome.episode.cost.rung_reached as u8, 7);
    assert!(
        outcome
            .episode
            .reasoning_trace
            .steps
            .windows(2)
            .all(|pair| pair[0].rung <= pair[1].rung)
    );
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn failed_local_run_escalates_to_teacher_before_recording() {
    let mut engine = Engine::in_memory().unwrap();
    let concept = Concept::new("BREAK", MutabilityClass::Definitional);
    engine.graph().insert_concept(&concept).unwrap();
    let procedure = Procedure::new(
        "BREAK",
        vec![Param::named("x")],
        Expr::BinOp {
            op: BinOp::Div,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(0))),
        },
    )
    .with_concept(concept.id);
    engine.graph().insert_procedure(&procedure).unwrap();

    let started = engine.begin_cycle(cycle_input("break 9", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, request } = started else {
        panic!("failed run should ask while a teacher is available");
    };
    assert_eq!(engine.episodes().count().unwrap(), 0);
    assert!(
        request.context["budget"]["max_exec_steps"]
            .as_u64()
            .unwrap()
            < 1_000
    );

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "break 9",
                json!({ "interpretations": [], "answer": "fallback" }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("one teacher turn should terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Provisional);
    assert!(outcome.episode.execution_trace.is_some());
    assert_eq!(outcome.episode.cost.rung_reached as u8, 6);
    assert!(outcome.episode.cost.steps_taken > 0);
    assert!(outcome.episode.cost.budget_spent > 1.0);
    assert!(outcome.episode.reasoning_trace.steps.len() >= 4);
    assert!(
        outcome
            .episode
            .context
            .budget_remaining
            .is_some_and(|budget| budget.steps < 1_000 && budget.teacher_calls == 0)
    );
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn run_then_teacher_procedure_preserves_both_execution_attempts() {
    let mut engine = Engine::in_memory().unwrap();
    let concept = Concept::new("RECOVER", MutabilityClass::Definitional);
    engine.graph().insert_concept(&concept).unwrap();
    let failing = Procedure::new(
        "RECOVER_BROKEN",
        vec![Param::named("x")],
        Expr::BinOp {
            op: BinOp::Div,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(0))),
        },
    )
    .with_concept(concept.id);
    engine.graph().insert_procedure(&failing).unwrap();
    let started = engine.begin_cycle(cycle_input("recover 9", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = started else {
        panic!("failed local run should ask");
    };
    let recovery = Procedure::new(
        "RECOVER_SAFE",
        vec![Param::named("x")],
        Expr::Var("x".into()),
    )
    .with_concept(concept.id);

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "recover 9",
                json!({
                    "interpretations": [{
                        "concept": { "id": concept.id.to_string() },
                        "weight": 1.0,
                        "inputs": { "x": 9 }
                    }],
                    "procedure": recovery,
                    "answer": 9
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("teacher recovery should terminalize");
    };
    let trace: ExecTrace =
        serde_json::from_value(outcome.episode.execution_trace.clone().unwrap()).unwrap();

    assert_eq!(outcome.disposition, CycleDisposition::Provisional);
    assert!(trace.steps.len() >= 2);
    assert!(
        trace
            .steps
            .iter()
            .any(|step| matches!(&step.status, ExecStepStatus::Failed { .. }))
    );
    assert!(
        trace
            .steps
            .iter()
            .any(|step| step.status == ExecStepStatus::Succeeded)
    );
    assert_eq!(outcome.episode.cost.steps_taken, trace.len() as u32);
    assert!(outcome.episode.cost.budget_spent > 1.0);
}

#[test]
fn provider_failure_aborts_a_pending_cycle_into_one_episode() {
    let mut engine = Engine::in_memory().unwrap();
    let started = engine.begin_cycle(cycle_input("unknown", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = started else {
        panic!("unknown input should ask");
    };

    let aborted = engine
        .abort_cycle(cycle_id, "provider authentication failed")
        .unwrap();
    let CycleProgress::Completed(outcome) = aborted else {
        panic!("abort must terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert_eq!(outcome.episode.cost.rung_reached as u8, 7);
    assert!(
        outcome
            .episode
            .teacher_interaction
            .as_ref()
            .and_then(|interaction| interaction.get("providerError"))
            .is_some()
    );
    assert_eq!(engine.episodes().count().unwrap(), 1);
    assert!(engine.abort_cycle(cycle_id, "again").is_err());
}

#[test]
fn teacher_cannot_reactivate_an_inactive_concept() {
    let mut engine = Engine::in_memory().unwrap();
    let mut concept = Concept::new("RETIRED", MutabilityClass::Definitional);
    concept.lifecycle = ekg_core::Lifecycle::Retired;
    engine.graph().insert_concept(&concept).unwrap();
    let started = engine.begin_cycle(cycle_input("unknown", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = started else {
        panic!("unknown input should ask");
    };

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "unknown",
                json!({
                    "interpretations": [{
                        "concept": { "id": concept.id.to_string() },
                        "weight": 1.0,
                        "inputs": {}
                    }]
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("inactive proposal must terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn failing_teacher_procedure_reaches_abstain_without_being_learned() {
    let mut engine = Engine::in_memory().unwrap();
    let concept = Concept::new("BREAK", MutabilityClass::Definitional);
    engine.graph().insert_concept(&concept).unwrap();
    let procedure = Procedure::new(
        "BREAK",
        vec![Param::named("x")],
        Expr::BinOp {
            op: BinOp::Div,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(0))),
        },
    )
    .with_concept(concept.id);
    let started = engine.begin_cycle(cycle_input("break 9", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = started else {
        panic!("missing procedure should ask");
    };

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "break 9",
                json!({
                    "interpretations": [{
                        "concept": { "id": concept.id.to_string() },
                        "weight": 1.0,
                        "inputs": { "x": 9 }
                    }],
                    "procedure": procedure
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("failing teacher procedure should terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert_eq!(outcome.episode.cost.rung_reached as u8, 7);
    assert!(
        engine
            .graph()
            .get_procedure_by_name("BREAK")
            .unwrap()
            .is_none()
    );
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn teacher_procedure_must_match_its_claimed_answer() {
    let mut engine = Engine::in_memory().unwrap();
    let concept = Concept::new("DOUBLE", MutabilityClass::Definitional);
    engine.graph().insert_concept(&concept).unwrap();
    let identity = Procedure::new("DOUBLE", vec![Param::named("x")], Expr::Var("x".into()))
        .with_concept(concept.id);
    let started = engine
        .begin_cycle(cycle_input("what is double 7?", true))
        .unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = started else {
        panic!("missing procedure should ask");
    };

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "what is double 7?",
                json!({
                    "interpretations": [{
                        "concept": { "id": concept.id.to_string() },
                        "weight": 1.0,
                        "inputs": { "x": 7 }
                    }],
                    "procedure": identity,
                    "answer": 14
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("inconsistent proposal should terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert_eq!(outcome.answer, None);
    assert!(
        engine
            .graph()
            .get_procedure_by_name("DOUBLE")
            .unwrap()
            .is_none()
    );
    assert!(
        outcome
            .episode
            .evaluation
            .as_ref()
            .is_some_and(|evaluation| !evaluation.success)
    );
}
