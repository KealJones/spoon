use std::collections::BTreeMap;

use serde_json::json;
use spoon_core::{BinOp, Concept, Expr, MutabilityClass, Param, Procedure, Value};
use spoon_engine::{
    CycleBudget, CycleDisposition, CycleInput, CycleProgress, Engine, TeacherProposalWire,
};
use spoon_exec::{ExecStepStatus, ExecTrace};

fn cycle_input(situation: &str, teacher_allowed: bool) -> CycleInput {
    CycleInput {
        situation: situation.into(),
        working_directory: None,
        environment: BTreeMap::new(),
        assumptions: Vec::new(),
        budget: CycleBudget {
            max_exec_steps: 1_000,
            max_context_items: 32,
            max_teacher_turns: 1,
        },
        teacher_allowed,
        session_id: None,
        recall_mode: Default::default(),
        permission_mode: None,
    }
}

#[test]
fn cycle_persists_working_directory_provenance_on_episode() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut input = cycle_input("working directory provenance", false);
    input.working_directory = Some("/workspace/example-repo".into());

    let CycleProgress::Completed(outcome) = engine.begin_cycle(input).unwrap() else {
        panic!("bounded no-teacher cycle should complete");
    };
    assert_eq!(
        outcome.episode.working_directory.as_deref(),
        Some("/workspace/example-repo")
    );
    assert_eq!(
        engine
            .episodes()
            .get(outcome.episode.id)
            .unwrap()
            .working_directory
            .as_deref(),
        Some("/workspace/example-repo")
    );
}

fn seed_double(engine: &Engine) -> (Concept, Procedure) {
    let concept = Concept::new("DOUBLE", MutabilityClass::Definitional);
    engine.admin_insert_concept(&concept).unwrap();
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
    engine.admin_insert_procedure(&procedure).unwrap();
    (concept, procedure)
}

#[test]
fn isolated_session_episode_is_not_in_global_recall_context() {
    let mut engine = Engine::in_memory_with_admin("session-test-admin").unwrap();
    let isolated = engine
        .create_session(
            Some("private".into()),
            spoon_core::SessionVisibility::Isolated,
        )
        .unwrap();
    let mut private_input = cycle_input("private memory", false);
    private_input.session_id = Some(isolated.id.to_string());
    private_input.recall_mode = spoon_engine::RecallMode::Session;
    let private = engine.begin_cycle(private_input).unwrap();
    let CycleProgress::Completed(private) = private else {
        panic!("private cycle should complete");
    };
    assert_eq!(private.episode.session_id, Some(isolated.id));
    assert_eq!(
        private.episode.session_visibility,
        spoon_core::SessionVisibility::Isolated
    );
    assert_eq!(private.episode.turn_index, Some(0));

    let global = engine
        .begin_cycle(cycle_input("global check", false))
        .unwrap();
    let CycleProgress::Completed(global) = global else {
        panic!("global cycle should complete");
    };
    assert!(
        global
            .episode
            .context
            .recent_episodes
            .iter()
            .all(|episode| episode.episode_id != private.episode.id)
    );
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

fn reusable_double_lesson(value: i64) -> serde_json::Value {
    json!({
        "proposalKind": "reusable_lesson",
        "interpretations": [],
        "lesson": {
            "primitiveSet": "pure_rpn_v1",
            "concepts": [{
                "key": "double",
                "name": "DOUBLE",
                "description": "Multiply any numeric input by two"
            }],
            "relationships": [],
            "procedures": [{
                "key": "double-procedure",
                "name": "DOUBLE",
                "concept": { "kind": "new_concept", "key": "double" },
                "parameters": [{ "name": "x", "description": "numeric input" }],
                "body": {
                    "instructions": [
                        { "op": "load_parameter", "name": "x" },
                        { "op": "push_literal", "value": 2 },
                        { "op": "multiply" }
                    ]
                },
                "contract": {
                    "requires": [],
                    "promises": [{
                        "description": "result is twice x",
                        "check": {
                            "instructions": [
                                { "op": "load_result" },
                                { "op": "load_parameter", "name": "x" },
                                { "op": "push_literal", "value": 2 },
                                { "op": "multiply" },
                                { "op": "equal" }
                            ]
                        }
                    }],
                    "failsWhen": []
                }
            }],
            "invocation": {
                "procedureKey": "double-procedure",
                "inputs": [{ "name": "x", "value": value }]
            }
        },
        "procedure": null,
        "answer": value * 2,
        "abstainReason": null
    })
}

fn reusable_count_occurrences_lesson(
    items: serde_json::Value,
    needle: serde_json::Value,
) -> serde_json::Value {
    json!({
        "proposalKind": "reusable_lesson",
        "interpretations": [],
        "lesson": {
            "primitiveSet": "pure_expr_v2",
            "concepts": [{
                "key": "count-occurrences",
                "name": "COUNT OCCURRENCES",
                "description": "Count values equal to a requested needle"
            }],
            "relationships": [],
            "procedures": [{
                "key": "count-occurrences-procedure",
                "name": "COUNT OCCURRENCES",
                "concept": { "kind": "new_concept", "key": "count-occurrences" },
                "parameters": [
                    { "name": "items", "description": "values to inspect" },
                    { "name": "needle", "description": "value to count" }
                ],
                "body": {
                    "kind": "intrinsic",
                    "version": 1,
                    "op": "count_equal",
                    "args": [
                        { "kind": "parameter", "name": "items" },
                        { "kind": "parameter", "name": "needle" }
                    ]
                },
                "contract": { "requires": [], "promises": [], "failsWhen": [] }
            }],
            "invocation": {
                "procedureKey": "count-occurrences-procedure",
                "inputs": [
                    { "name": "items", "value": items },
                    { "name": "needle", "value": needle }
                ]
            }
        },
        "procedure": null,
        "answer": 2,
        "abstainReason": null
    })
}

fn reusable_count_letter_lesson(text: &str, letter: &str) -> serde_json::Value {
    json!({
        "proposalKind": "reusable_lesson",
        "interpretations": [],
        "lesson": {
            "primitiveSet": "pure_expr_v2",
            "concepts": [{
                "key": "count-letter",
                "name": "COUNT LETTER",
                "description": "Count a requested letter in Unicode text without case sensitivity"
            }],
            "relationships": [],
            "procedures": [{
                "key": "count-letter-procedure",
                "name": "COUNT LETTER",
                "concept": { "kind": "new_concept", "key": "count-letter" },
                "parameters": [
                    { "name": "letter", "description": "letter to count" },
                    { "name": "text", "description": "text to inspect" }
                ],
                "body": {
                    "kind": "intrinsic", "version": 1, "op": "length",
                    "args": [{
                        "kind": "filter",
                        "collection": {
                            "kind": "intrinsic", "version": 1, "op": "text_split",
                            "args": [
                                { "kind": "intrinsic", "version": 1, "op": "text_lowercase", "args": [{ "kind": "parameter", "name": "text" }] },
                                { "kind": "literal", "value": "" }
                            ]
                        },
                        "var": "grapheme",
                        "predicate": {
                            "kind": "binary", "op": "equal",
                            "left": { "kind": "parameter", "name": "grapheme" },
                            "right": { "kind": "intrinsic", "version": 1, "op": "text_lowercase", "args": [{ "kind": "parameter", "name": "letter" }] }
                        }
                    }]
                },
                "contract": { "requires": [], "promises": [], "failsWhen": [] }
            }],
            "invocation": {
                "procedureKey": "count-letter-procedure",
                "inputs": [
                    { "name": "letter", "value": letter },
                    { "name": "text", "value": text }
                ]
            }
        },
        "procedure": null,
        "answer": 3,
        "abstainReason": null
    })
}

fn reusable_json_path_lesson() -> serde_json::Value {
    json!({
        "proposalKind": "reusable_lesson",
        "interpretations": [],
        "lesson": {
            "primitiveSet": "pure_expr_v2",
            "concepts": [{
                "key": "json-path",
                "name": "JSON PATH",
                "description": "Read a value from JSON using a path"
            }],
            "relationships": [],
            "procedures": [{
                "key": "json-path-procedure",
                "name": "JSON PATH",
                "concept": { "kind": "new_concept", "key": "json-path" },
                "parameters": [
                    { "name": "document", "description": "JSON text" },
                    { "name": "path", "description": "path to read" }
                ],
                "body": {
                    "kind": "intrinsic", "version": 1, "op": "path_get",
                    "args": [
                        { "kind": "intrinsic", "version": 1, "op": "json_parse", "args": [{ "kind": "parameter", "name": "document" }] },
                        { "kind": "parameter", "name": "path" }
                    ]
                },
                "contract": { "requires": [], "promises": [], "failsWhen": [] }
            }],
            "invocation": {
                "procedureKey": "json-path-procedure",
                "inputs": [
                    { "name": "document", "value": r#"{"items":[{"id":7}]}"# },
                    { "name": "path", "value": "items[0]" }
                ]
            }
        },
        "procedure": null,
        "answer": { "id": 7 },
        "abstainReason": null
    })
}

fn begin_teacher_cycle(engine: &mut Engine, situation: &str) -> spoon_engine::CycleId {
    let CycleProgress::NeedTeacher { cycle_id, .. } =
        engine.begin_cycle(cycle_input(situation, true)).unwrap()
    else {
        panic!("unknown input must ask the teacher");
    };
    cycle_id
}

fn begin_teacher_cycle_with_request(
    engine: &mut Engine,
    situation: &str,
) -> (spoon_engine::CycleId, spoon_engine::TeacherRequestWire) {
    let CycleProgress::NeedTeacher { cycle_id, request } =
        engine.begin_cycle(cycle_input(situation, true)).unwrap()
    else {
        panic!("unknown input must ask the teacher");
    };
    (cycle_id, request)
}

fn reusable_dependency_lesson(alias: &str, value: i64) -> serde_json::Value {
    let call = |argument| {
        json!({
            "kind": "dependency",
            "alias": alias,
            "args": [argument]
        })
    };
    json!({
        "proposalKind": "reusable_lesson",
        "interpretations": [],
        "lesson": {
            "primitiveSet": "pure_expr_v2",
            "concepts": [{
                "key": "quadruple",
                "name": "QUADRUPLE",
                "description": "Apply the advertised doubling procedure twice"
            }],
            "relationships": [],
            "procedures": [{
                "key": "quadruple-procedure",
                "name": "QUADRUPLE",
                "concept": { "kind": "new_concept", "key": "quadruple" },
                "parameters": [{ "name": "x", "description": "numeric input" }],
                "body": call(call(json!({ "kind": "parameter", "name": "x" }))),
                "contract": { "requires": [], "promises": [], "failsWhen": [] }
            }],
            "invocation": {
                "procedureKey": "quadruple-procedure",
                "inputs": [{ "name": "x", "value": value }]
            }
        },
        "procedure": null,
        "answer": value * 4,
        "abstainReason": null
    })
}

#[test]
fn reusable_lesson_admits_general_facts_and_composable_sibling_procedures() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let situation = "increment 3 and then double the result";
    let cycle_id = begin_teacher_cycle(&mut engine, situation);
    let lesson = json!({
        "proposalKind": "reusable_lesson",
        "interpretations": [],
        "lesson": {
            "primitiveSet": "pure_expr_v2",
            "concepts": [
                {
                    "key": "successor-fact",
                    "name": "SUCCESSOR",
                    "description": "The successor of an integer is the next integer, one greater than it.",
                    "mutability": "definitional"
                },
                {
                    "key": "increment",
                    "name": "INCREMENT",
                    "description": "Add one to a numeric input.",
                    "mutability": "procedural"
                },
                {
                    "key": "double-after-increment",
                    "name": "DOUBLE AFTER INCREMENT",
                    "description": "Increment a numeric input and double that intermediate result.",
                    "mutability": "procedural"
                }
            ],
            "relationships": [],
            "procedures": [
                {
                    "key": "increment-procedure",
                    "name": "INCREMENT",
                    "concept": { "kind": "new_concept", "key": "increment" },
                    "parameters": [{ "name": "x", "description": "numeric input" }],
                    "body": {
                        "kind": "binary", "op": "add",
                        "left": { "kind": "parameter", "name": "x" },
                        "right": { "kind": "literal", "value": 1 }
                    },
                    "contract": { "requires": [], "promises": [], "failsWhen": [] }
                },
                {
                    "key": "double-after-increment-procedure",
                    "name": "DOUBLE AFTER INCREMENT",
                    "concept": { "kind": "new_concept", "key": "double-after-increment" },
                    "parameters": [{ "name": "x", "description": "numeric input" }],
                    "body": {
                        "kind": "binary", "op": "multiply",
                        "left": {
                            "kind": "dependency",
                            "alias": "lesson:increment-procedure",
                            "args": [{ "kind": "parameter", "name": "x" }]
                        },
                        "right": { "kind": "literal", "value": 2 }
                    },
                    "contract": { "requires": [], "promises": [], "failsWhen": [] }
                }
            ],
            "invocation": {
                "procedureKey": "double-after-increment-procedure",
                "inputs": [{ "name": "x", "value": 3 }]
            }
        },
        "procedure": null,
        "answer": 8,
        "abstainReason": null
    });

    let CycleProgress::Completed(outcome) = engine
        .resume_cycle(cycle_id, proposal(situation, lesson))
        .unwrap()
    else {
        panic!("composable lesson should execute");
    };
    assert_eq!(outcome.answer, Some(Value::Int(8)));
    assert_eq!(engine.graph().list_concepts().unwrap().len(), 3);
    assert_eq!(engine.graph().list_procedures().unwrap().len(), 2);
    assert!(
        engine
            .graph()
            .list_concepts()
            .unwrap()
            .iter()
            .any(|concept| concept.name == "SUCCESSOR"
                && concept.mutability == MutabilityClass::Definitional)
    );
}

#[test]
fn pure_expr_v2_count_occurrences_compiles_executes_persists_and_reuses_without_teacher() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let cycle_id = begin_teacher_cycle(&mut engine, "count occurrences of red");
    let learned = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "count occurrences of red",
                reusable_count_occurrences_lesson(json!(["red", "blue", "red"]), json!("red")),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(learned) = learned else {
        panic!("v2 lesson should execute");
    };
    assert_eq!(learned.answer, Some(Value::Int(2)));
    assert_eq!(engine.graph().list_procedures().unwrap().len(), 1);

    let mut held_out = cycle_input("count occurrences", false);
    held_out.environment.insert(
        "items".into(),
        Value::List(vec![
            Value::Text("red".into()),
            Value::Text("red".into()),
            Value::Text("blue".into()),
        ]),
    );
    held_out
        .environment
        .insert("needle".into(), Value::Text("red".into()));
    let CycleProgress::Completed(reused) = engine.begin_cycle(held_out).unwrap() else {
        panic!("persisted v2 lesson should run locally");
    };
    assert_eq!(reused.disposition, CycleDisposition::Provisional);
    assert_eq!(reused.answer, Some(Value::Int(2)));
    assert!(reused.episode.teacher_interaction.is_none());
}

#[test]
fn pure_expr_v2_dependency_aliases_pin_exact_pure_procedure_versions_and_reuse_offline() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let (_, double) = seed_double(&engine);
    let (cycle_id, request) = begin_teacher_cycle_with_request(&mut engine, "quadruple 3");
    let dependencies = request.context["pureProcedureDependencies"]
        .as_array()
        .expect("teacher context must advertise pure dependency aliases");
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0]["name"], "DOUBLE");
    assert!(dependencies[0].get("id").is_none());
    let alias = dependencies[0]["alias"].as_str().unwrap();

    let CycleProgress::Completed(learned) = engine
        .resume_cycle(
            cycle_id,
            proposal("quadruple 3", reusable_dependency_lesson(alias, 3)),
        )
        .unwrap()
    else {
        panic!("engine-owned dependency alias should compose");
    };
    assert_eq!(learned.answer, Some(Value::Int(12)));
    let learned_procedure = engine
        .graph()
        .list_procedures()
        .unwrap()
        .into_iter()
        .find(|procedure| procedure.name == "QUADRUPLE")
        .unwrap();
    let stored_body = serde_json::to_value(&learned_procedure.body).unwrap();
    assert_eq!(
        stored_body["CallExact"]["procedure"],
        serde_json::to_value(double.id).unwrap()
    );
    assert_eq!(stored_body["CallExact"]["version"], json!(1));

    let mut revised_double = double.clone();
    revised_double.version = 2;
    revised_double.body = Expr::BinOp {
        op: BinOp::Mul,
        left: Box::new(Expr::Var("x".into())),
        right: Box::new(Expr::Literal(Value::Int(3))),
    };
    engine.admin_revise_procedure(&revised_double, 1).unwrap();

    let mut held_out = cycle_input("quadruple", false);
    held_out.environment.insert("x".into(), Value::Int(4));
    let CycleProgress::Completed(reused) = engine.begin_cycle(held_out).unwrap() else {
        panic!("stored dependency lesson should run with Teacher disabled");
    };
    assert_eq!(reused.answer, Some(Value::Int(16)));
    assert!(reused.episode.teacher_interaction.is_none());
    let trace: ExecTrace = serde_json::from_value(reused.episode.execution_trace.unwrap()).unwrap();
    assert!(trace.steps.iter().any(|step| {
        step.procedure_called == Some(double.id) && step.procedure_version == Some(1)
    }));
}

#[test]
fn unsafe_or_unadvertised_dependency_aliases_do_not_mutate_lesson_knowledge() {
    for (case, alias, extra) in [
        ("unknown", "not-advertised", json!({})),
        ("draft", "draft-only", json!({})),
        (
            "effect-shaped",
            "not-advertised",
            json!({ "effect": "file_write" }),
        ),
    ] {
        let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
        seed_double(&engine);
        let mut draft =
            Procedure::new("DRAFT ONLY", vec![Param::named("x")], Expr::Var("x".into()));
        draft.lifecycle = spoon_core::Lifecycle::Provisional;
        engine.admin_insert_procedure(&draft).unwrap();
        let (cycle_id, request) = begin_teacher_cycle_with_request(&mut engine, "quadruple 3");
        let advertised = request.context["pureProcedureDependencies"]
            .as_array()
            .unwrap();
        assert!(advertised.iter().all(|item| item["name"] != "DRAFT ONLY"));
        let mut lesson = reusable_dependency_lesson(alias, 3);
        if !extra.as_object().unwrap().is_empty() {
            lesson["lesson"]["procedures"][0]["body"]["effect"] = extra["effect"].clone();
        }
        let before_concepts = engine.graph().list_concepts().unwrap().len();
        let before_procedures = engine.graph().list_procedures().unwrap().len();
        let CycleProgress::Completed(_outcome) = engine
            .resume_cycle(cycle_id, proposal("quadruple 3", lesson))
            .unwrap()
        else {
            panic!("unsafe dependency lesson must complete as an abstention");
        };
        // A separately useful Teacher answer may remain provisional, but the
        // rejected lesson itself must be atomic: no new durable knowledge.
        assert_eq!(
            engine.graph().list_concepts().unwrap().len(),
            before_concepts,
            "{case}"
        );
        assert_eq!(
            engine.graph().list_procedures().unwrap().len(),
            before_procedures,
            "{case}"
        );
    }
}

#[test]
fn dependency_alias_revision_drift_rejects_lesson_admission_atomically() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let (_, double) = seed_double(&engine);
    let (cycle_id, request) = begin_teacher_cycle_with_request(&mut engine, "quadruple 3");
    let alias = request.context["pureProcedureDependencies"][0]["alias"]
        .as_str()
        .unwrap();
    let mut revised_double = double;
    revised_double.version = 2;
    engine.admin_revise_procedure(&revised_double, 1).unwrap();

    let before_concepts = engine.graph().list_concepts().unwrap().len();
    let before_procedures = engine.graph().list_procedures().unwrap().len();
    let CycleProgress::Completed(_outcome) = engine
        .resume_cycle(
            cycle_id,
            proposal("quadruple 3", reusable_dependency_lesson(alias, 3)),
        )
        .unwrap()
    else {
        panic!("revision-drifted lesson must not be persisted");
    };
    assert_eq!(
        engine.graph().list_concepts().unwrap().len(),
        before_concepts
    );
    assert_eq!(
        engine.graph().list_procedures().unwrap().len(),
        before_procedures
    );
}

#[test]
fn pure_expr_v2_json_path_procedure_uses_only_engine_intrinsics() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let cycle_id = begin_teacher_cycle(&mut engine, "read json path");
    let CycleProgress::Completed(outcome) = engine
        .resume_cycle(
            cycle_id,
            proposal("read json path", reusable_json_path_lesson()),
        )
        .unwrap()
    else {
        panic!("json/path lesson should execute");
    };
    assert_eq!(
        outcome.answer,
        Some(serde_json::from_value(json!({ "id": 7 })).unwrap())
    );
    assert_eq!(engine.graph().list_procedures().unwrap().len(), 1);
}

#[test]
fn pure_expr_v2_authors_new_v1_intrinsics_through_the_schema_and_compiler() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let cycle_id = begin_teacher_cycle(&mut engine, "find a text position");
    let mut lesson = reusable_count_occurrences_lesson(json!("a👩‍💻b"), json!("b"));
    lesson["lesson"]["procedures"][0]["body"] = json!({
        "kind": "intrinsic", "version": 1, "op": "text_index_of",
        "args": [
            { "kind": "parameter", "name": "items" },
            { "kind": "parameter", "name": "needle" }
        ]
    });
    lesson["answer"] = json!(2);
    let CycleProgress::Completed(outcome) = engine
        .resume_cycle(cycle_id, proposal("find a text position", lesson))
        .unwrap()
    else {
        panic!("expanded intrinsic should compile and execute");
    };
    assert_eq!(outcome.answer, Some(Value::Int(2)));
}

#[test]
fn pure_expr_v2_collection_find_index_compiles_through_schema_and_compiler() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let cycle_id = begin_teacher_cycle(&mut engine, "find the matching value");
    let mut lesson =
        reusable_count_occurrences_lesson(json!(["first", "needle", "needle"]), json!("needle"));
    lesson["lesson"]["procedures"][0]["body"] = json!({
        "kind": "intrinsic",
        "version": 1,
        "op": "collection_find_index",
        "args": [
            { "kind": "parameter", "name": "items" },
            { "kind": "parameter", "name": "needle" }
        ]
    });
    lesson["answer"] = json!(1);

    let CycleProgress::Completed(outcome) = engine
        .resume_cycle(cycle_id, proposal("find the matching value", lesson))
        .unwrap()
    else {
        panic!("collection_find_index lesson should compile and execute");
    };
    assert_eq!(outcome.answer, Some(Value::Int(1)));
}

#[test]
fn pure_expr_v2_map_from_entries_compiles_dynamic_objects() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let cycle_id = begin_teacher_cycle(&mut engine, "build an object from entries");
    let mut lesson = reusable_count_occurrences_lesson(
        json!([["name", "Spoon"], ["count", 3], ["name", "EKG"]]),
        json!("unused"),
    );
    lesson["lesson"]["procedures"][0]["body"] = json!({
        "kind": "intrinsic",
        "version": 1,
        "op": "map_from_entries",
        "args": [{ "kind": "parameter", "name": "items" }]
    });
    lesson["answer"] = json!({ "name": "EKG", "count": 3 });

    let CycleProgress::Completed(outcome) = engine
        .resume_cycle(cycle_id, proposal("build an object from entries", lesson))
        .unwrap()
    else {
        panic!("map_from_entries lesson should compile and execute");
    };
    assert_eq!(
        outcome.answer,
        Some(serde_json::from_value(json!({ "name": "EKG", "count": 3 })).unwrap())
    );
}

#[test]
fn pure_expr_v2_json_pointer_set_compiles_immutable_updates() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let cycle_id = begin_teacher_cycle(&mut engine, "update a JSON field");
    let mut lesson = reusable_count_occurrences_lesson(json!({ "answer": 7 }), json!("unused"));
    lesson["lesson"]["procedures"][0]["body"] = json!({
        "kind": "intrinsic",
        "version": 1,
        "op": "json_pointer_set",
        "args": [
            { "kind": "parameter", "name": "items" },
            { "kind": "literal", "value": "/answer" },
            { "kind": "literal", "value": 42 }
        ]
    });
    lesson["answer"] = json!({ "answer": 42 });

    let CycleProgress::Completed(outcome) = engine
        .resume_cycle(cycle_id, proposal("update a JSON field", lesson))
        .unwrap()
    else {
        panic!("json_pointer_set lesson should compile and execute");
    };
    assert_eq!(
        outcome.answer,
        Some(serde_json::from_value(json!({ "answer": 42 })).unwrap())
    );
}

#[test]
fn rich_count_letter_lesson_reuses_explicit_quoted_text_with_teacher_disabled() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let cycle_id = begin_teacher_cycle(&mut engine, "count \"R\" in \"strawberry\"");
    let CycleProgress::Completed(learned) = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "count \"R\" in \"strawberry\"",
                reusable_count_letter_lesson("strawberry", "R"),
            ),
        )
        .unwrap()
    else {
        panic!("generic rich letter-count lesson should execute");
    };
    assert_eq!(learned.answer, Some(Value::Int(3)));

    let CycleProgress::Completed(reused) = engine
        .begin_cycle(cycle_input("count \"r\" in \"raspberry\"", false))
        .unwrap()
    else {
        panic!("Teacher-OFF reuse must terminalize");
    };
    assert_eq!(reused.answer, Some(Value::Int(3)));
    assert!(reused.episode.teacher_interaction.is_none());
}

#[test]
fn quoted_text_binding_is_escape_aware_and_malformed_requests_do_not_rebind() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let cycle_id = begin_teacher_cycle(&mut engine, "count \"R\" in \"strawberry\"");
    engine
        .resume_cycle(
            cycle_id,
            proposal(
                "count \"R\" in \"strawberry\"",
                reusable_count_letter_lesson("strawberry", "R"),
            ),
        )
        .unwrap();
    let procedures_before = engine.graph().list_procedures().unwrap().len();

    let CycleProgress::Completed(escaped) = engine
        .begin_cycle(cycle_input("count \"\\\"\" in \"a\\\"b\\\"\"", false))
        .unwrap()
    else {
        panic!("escaped quoted values should remain explicit local inputs");
    };
    assert_eq!(escaped.answer, Some(Value::Int(2)));

    let CycleProgress::Completed(malformed) = engine
        .begin_cycle(cycle_input("count \"r in raspberry", false))
        .unwrap()
    else {
        panic!("Teacher-OFF malformed input must terminalize safely");
    };
    assert_eq!(malformed.disposition, CycleDisposition::Abstained);
    assert_eq!(malformed.answer, None);
    assert!(malformed.episode.teacher_interaction.is_none());
    assert_eq!(
        engine.graph().list_procedures().unwrap().len(),
        procedures_before
    );
}

#[test]
fn oversized_quoted_text_is_rejected_before_cycle_state_changes() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let oversized = format!("count \"r\" in \"{}\"", "x".repeat(65_537));
    assert!(engine.begin_cycle(cycle_input(&oversized, false)).is_err());
    assert_eq!(engine.graph().list_procedures().unwrap().len(), 0);
    assert_eq!(engine.episodes().count().unwrap(), 0);
}

#[test]
fn pure_expr_v2_numeric_intrinsic_is_teacher_authorable_and_reusable() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let cycle_id = begin_teacher_cycle(&mut engine, "absolute value");
    let mut lesson = reusable_count_occurrences_lesson(json!([-9]), json!(-9));
    lesson["lesson"]["procedures"][0]["parameters"] = json!([
        { "name": "x", "description": "numeric input" }
    ]);
    lesson["lesson"]["procedures"][0]["body"] = json!({
        "kind": "intrinsic", "version": 1, "op": "numeric_abs",
        "args": [{ "kind": "parameter", "name": "x" }]
    });
    lesson["lesson"]["invocation"]["inputs"] = json!([
        { "name": "x", "value": -9 }
    ]);
    lesson["answer"] = json!(9);
    let CycleProgress::Completed(outcome) = engine
        .resume_cycle(cycle_id, proposal("absolute value", lesson))
        .unwrap()
    else {
        panic!("numeric intrinsic should compile and execute");
    };
    assert_eq!(outcome.answer, Some(Value::Int(9)));
}

#[test]
fn unsafe_pure_expr_v2_drafts_do_not_mutate_knowledge() {
    for body in [
        json!({ "kind": "literal", "value": null, "unknown": true }),
        json!({ "kind": "call", "procedure": "teacher-minted", "args": [] }),
        json!({ "kind": "intrinsic", "version": 1, "op": "network_fetch", "args": [] }),
        (0..=MAX_TEST_EXPR_DEPTH).enumerate().fold(
            json!({ "kind": "parameter", "name": "items" }),
            |body, (index, _)| json!({ "kind": "let", "name": format!("x{index}"), "value": { "kind": "literal", "value": null }, "body": body }),
        ),
    ] {
        let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
        let cycle_id = begin_teacher_cycle(&mut engine, "count occurrences");
        let mut proposal_body = reusable_count_occurrences_lesson(json!(["red", "red"]), json!("red"));
        proposal_body["lesson"]["procedures"][0]["body"] = body;
        let progress = engine
            .resume_cycle(cycle_id, proposal("count occurrences", proposal_body))
            .unwrap();
        assert!(matches!(progress, CycleProgress::Completed(_)));
        assert!(engine.graph().list_concepts().unwrap().is_empty());
        assert!(engine.graph().list_procedures().unwrap().is_empty());
    }
}

const MAX_TEST_EXPR_DEPTH: usize = 33;

#[test]
fn empty_graph_teacher_lesson_bootstraps_generic_double_then_runs_locally() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let start = engine
        .begin_cycle(cycle_input("what is double 7?", true))
        .unwrap();
    let CycleProgress::NeedTeacher {
        cycle_id, request, ..
    } = start
    else {
        panic!("empty graph must ask");
    };
    assert!(request.desired_output["properties"]["lesson"].is_object());
    assert_eq!(
        request.desired_output["properties"]["procedure"]["type"],
        "null"
    );
    assert_eq!(
        request.context["authoringProtocol"]["primitiveSet"],
        "pure_expr_v2"
    );

    let learned = engine
        .resume_cycle(
            cycle_id,
            proposal("what is double 7?", reusable_double_lesson(7)),
        )
        .unwrap();
    let CycleProgress::Completed(learned) = learned else {
        panic!("valid reusable lesson should execute and complete");
    };
    assert_eq!(learned.answer, Some(Value::Int(14)));
    assert_eq!(learned.disposition, CycleDisposition::Provisional);
    let concepts = engine.graph().list_concepts().unwrap();
    let procedures = engine.graph().list_procedures().unwrap();
    assert_eq!(concepts.len(), 1);
    assert_eq!(procedures.len(), 1);
    assert_eq!(concepts[0].name, "DOUBLE");
    assert_eq!(concepts[0].mutability, MutabilityClass::Procedural);
    assert_eq!(concepts[0].lifecycle, spoon_core::Lifecycle::Provisional);
    assert_eq!(procedures[0].concept, Some(concepts[0].id));
    assert_eq!(procedures[0].lifecycle, spoon_core::Lifecycle::Provisional);
    assert!(!learned.episode.interpretations.is_empty());

    let local = engine
        .begin_cycle(cycle_input("what is double 9?", true))
        .unwrap();
    let CycleProgress::Completed(local) = local else {
        panic!("learned generic procedure should run without a teacher");
    };
    assert_eq!(local.answer, Some(Value::Int(18)));
    assert_eq!(local.episode.cost.rung_reached as u8, 2);
    assert_eq!(engine.graph().list_procedures().unwrap().len(), 1);
}

#[test]
fn reusable_lesson_compilation_has_recovery_stable_knowledge_ids() {
    let learn = || {
        let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
        let CycleProgress::NeedTeacher { cycle_id, .. } = engine
            .begin_cycle(cycle_input("what is double 7?", true))
            .unwrap()
        else {
            panic!("empty graph must ask");
        };
        engine
            .resume_cycle(
                cycle_id,
                proposal("what is double 7?", reusable_double_lesson(7)),
            )
            .unwrap();
        let concept_id = engine.graph().list_concepts().unwrap()[0].id;
        let procedure_id = engine.graph().list_procedures().unwrap()[0].id;
        assert_ne!(concept_id.0, procedure_id.0);
        (concept_id, procedure_id)
    };
    assert_eq!(learn(), learn());
}

#[test]
fn reusable_lesson_must_match_its_claimed_answer_before_atomic_learning() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = engine
        .begin_cycle(cycle_input("what is double 7?", true))
        .unwrap()
    else {
        panic!("empty graph must ask");
    };
    let mut lesson = reusable_double_lesson(7);
    lesson["answer"] = json!(15);
    let CycleProgress::Completed(outcome) = engine
        .resume_cycle(cycle_id, proposal("what is double 7?", lesson))
        .unwrap()
    else {
        panic!("contradictory lesson must terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
    assert!(engine.graph().list_concepts().unwrap().is_empty());
    assert!(engine.graph().list_procedures().unwrap().is_empty());
}

#[test]
fn external_observation_answer_does_not_create_a_fake_procedure() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let situation = "what time is it right now?";
    let start = engine.begin_cycle(cycle_input(situation, true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unknown observation must ask");
    };
    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                situation,
                json!({
                    "proposalKind": "external_observation",
                    "interpretations": [],
                    "lesson": null,
                    "procedure": null,
                    "answer": "12:34",
                    "abstainReason": null
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("observation answer should terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Provisional);
    assert_eq!(outcome.answer, Some(Value::Text("12:34".into())));
    assert!(engine.graph().list_concepts().unwrap().is_empty());
    assert!(engine.graph().list_procedures().unwrap().is_empty());
}

#[test]
fn constant_reusable_lesson_is_not_learned_as_an_observation_sensor() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let situation = "what time is it right now?";
    let start = engine.begin_cycle(cycle_input(situation, true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unknown observation must ask");
    };
    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                situation,
                json!({
                    "proposalKind": "reusable_lesson",
                    "interpretations": [],
                    "lesson": {
                        "primitiveSet": "pure_rpn_v1",
                        "concepts": [{
                            "key": "clock",
                            "name": "CURRENT_TIME",
                            "description": "current time",
                            "mutability": "particular"
                        }],
                        "relationships": [],
                        "procedures": [{
                            "key": "clock-procedure",
                            "name": "CURRENT_TIME",
                            "concept": { "kind": "new_concept", "key": "clock" },
                            "parameters": [],
                            "body": { "instructions": [{ "op": "push_literal", "value": "12:34" }] },
                            "contract": { "requires": [], "promises": [], "failsWhen": [] }
                        }],
                        "invocation": { "procedureKey": "clock-procedure", "inputs": [] }
                    },
                    "procedure": null,
                    "answer": "12:34",
                    "abstainReason": null
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("teacher budget is exhausted, so safe answer fallback should terminalize");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Provisional);
    assert_eq!(outcome.answer, Some(Value::Text("12:34".into())));
    assert!(engine.graph().list_procedures().unwrap().is_empty());
}

#[test]
fn malformed_reusable_lesson_gets_one_targeted_retry_when_budget_allows() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut input = cycle_input("what is double 7?", true);
    input.budget.max_teacher_turns = 2;
    let start = engine.begin_cycle(input).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("empty graph must ask");
    };
    let mut malformed = reusable_double_lesson(7);
    malformed["lesson"]["procedures"][0]["body"]["instructions"] = json!([{ "op": "multiply" }]);

    let retry = engine
        .resume_cycle(cycle_id, proposal("what is double 7?", malformed))
        .unwrap();
    let CycleProgress::NeedTeacher {
        cycle_id: retry_id,
        request,
    } = retry
    else {
        panic!("unsafe reusable lesson should receive a bounded retry");
    };
    assert_eq!(retry_id, cycle_id);
    assert!(
        request
            .specific_question
            .as_deref()
            .unwrap()
            .contains("could not be safely compiled")
    );
    assert!(engine.graph().list_procedures().unwrap().is_empty());

    let completed = engine
        .resume_cycle(
            retry_id,
            proposal("what is double 7?", reusable_double_lesson(7)),
        )
        .unwrap();
    assert!(matches!(completed, CycleProgress::Completed(_)));
    assert_eq!(engine.graph().list_procedures().unwrap().len(), 1);
    assert_eq!(engine.episodes().count().unwrap(), 1);
}

#[test]
fn run_matches_a_linked_procedure_without_domain_special_cases() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
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
    assert!(engine.intuition_metrics().unwrap().ranking_examples > 0);
}

#[test]
fn local_interpretation_matches_a_safe_inflection_of_a_learned_concept() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("Doubling", MutabilityClass::Procedural);
    engine.admin_insert_concept(&concept).unwrap();
    let procedure = Procedure::new(
        "Double a Value",
        vec![Param::named("value")],
        Expr::BinOp {
            op: BinOp::Mul,
            left: Box::new(Expr::Var("value".into())),
            right: Box::new(Expr::Literal(Value::Int(2))),
        },
    )
    .with_concept(concept.id);
    engine.admin_insert_procedure(&procedure).unwrap();

    let CycleProgress::Completed(outcome) = engine
        .begin_cycle(cycle_input("what is double 9?", false))
        .unwrap()
    else {
        panic!("the learned concept should resolve locally");
    };
    assert_eq!(outcome.answer, Some(Value::Int(18)));
    assert!(outcome.episode.teacher_interaction.is_none());
}

#[test]
fn unknown_with_teacher_enabled_returns_a_nonterminal_continuation() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();

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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    for index in 0..100 {
        let concept = Concept::new(
            format!("CONCEPT_{index}_{}", "x".repeat(4_000)),
            MutabilityClass::Definitional,
        );
        engine.admin_insert_concept(&concept).unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut input = cycle_input("unknown", true);
    input.budget.max_context_items = 1_025;

    assert!(engine.begin_cycle(input).is_err());
    assert_eq!(engine.episodes().count().unwrap(), 0);
}

#[test]
fn teacher_disabled_unknown_abstains_and_records_one_episode() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();

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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut input = cycle_input("what is the answer?", true);
    input
        .environment
        .insert("topic".into(), Value::Text("life".into()));
    input.assumptions.push(spoon_core::Assumption {
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
fn malformed_optional_procedure_is_discarded_without_losing_a_valid_answer() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let start = engine
        .begin_cycle(cycle_input("what is double 7?", true))
        .unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unknown input should ask");
    };

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "what is double 7?",
                json!({
                    "interpretations": [],
                    "procedure": "Return the number multiplied by two.",
                    "answer": 14,
                    "abstainReason": null
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("valid answer should terminalize even when optional procedure is unusable");
    };

    assert_eq!(outcome.disposition, CycleDisposition::Provisional);
    assert_eq!(outcome.answer, Some(Value::Int(14)));
    assert!(engine.graph().list_procedures().unwrap().is_empty());
}

#[test]
fn unresolvable_optional_interpretation_is_discarded_without_losing_a_valid_answer() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let start = engine
        .begin_cycle(cycle_input("what is double 7?", true))
        .unwrap();
    let CycleProgress::NeedTeacher { cycle_id, .. } = start else {
        panic!("unknown input should ask");
    };

    let resumed = engine
        .resume_cycle(
            cycle_id,
            proposal(
                "what is double 7?",
                json!({
                    "interpretations": [{
                        "concept": { "name": "unknown-doubling-label" },
                        "weight": 1.0,
                        "inputs": [{ "name": "number", "value": 7 }]
                    }],
                    "procedure": null,
                    "answer": 14,
                    "abstainReason": null
                }),
            ),
        )
        .unwrap();
    let CycleProgress::Completed(outcome) = resumed else {
        panic!("valid answer should terminalize when optional interpretation is unusable");
    };

    assert_eq!(outcome.disposition, CycleDisposition::Provisional);
    assert_eq!(outcome.answer, Some(Value::Int(14)));
    assert!(outcome.episode.interpretations.is_empty());
}

#[test]
fn ambiguous_local_literals_do_not_get_guessed() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let (mut concept, mut procedure) = seed_double(&engine);
    concept.lifecycle = spoon_core::Lifecycle::Invalid;
    engine.admin_update_concept(&concept).unwrap();
    procedure.lifecycle = spoon_core::Lifecycle::Retired;
    procedure.version += 1;
    engine.admin_update_procedure(&procedure).unwrap();

    let progress = engine.begin_cycle(cycle_input("double 7", false)).unwrap();
    let CycleProgress::Completed(outcome) = progress else {
        panic!("inactive knowledge must not run");
    };
    assert_eq!(outcome.disposition, CycleDisposition::Abstained);
}

#[test]
fn exact_verified_history_resolves_at_recall() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("recall arithmetic", MutabilityClass::Definitional);
    engine.admin_insert_concept(&concept).unwrap();
    let procedure = Procedure::new("RECALL DOUBLE", Vec::new(), Expr::Literal(Value::Int(14)))
        .with_concept(concept.id);
    engine.admin_insert_procedure(&procedure).unwrap();
    let prior = engine
        .execute_procedure(procedure.id, BTreeMap::new(), Some(Value::Int(14)))
        .unwrap()
        .episode;

    let progress = engine
        .begin_cycle(cycle_input("execute RECALL DOUBLE", true))
        .unwrap();
    let CycleProgress::Completed(outcome) = progress else {
        panic!("verified history should recall");
    };

    assert_eq!(outcome.answer, Some(Value::Int(14)));
    assert_eq!(outcome.episode.cost.rung_reached as u8, 1);
    assert_eq!(engine.episodes().count().unwrap(), 2);
    assert!(engine.trust_receipt_for_episode(&prior).unwrap().is_some());
}

#[test]
fn a_teacher_continuation_can_only_be_consumed_once() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
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
fn pending_teacher_cycle_survives_reopen_and_is_claimed_by_one_engine() {
    let path = std::env::temp_dir().join(format!(
        "spoon-pending-cycle-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().into_owned();
    let cycle_id = {
        let mut first = Engine::open(&path_text).unwrap();
        let CycleProgress::NeedTeacher { cycle_id, .. } = first
            .begin_cycle(cycle_input("durable unknown", true))
            .unwrap()
        else {
            panic!("unknown input should persist a teacher continuation");
        };
        cycle_id
    };

    {
        let mut recovered = Engine::open(&path_text).unwrap();
        let completed = recovered
            .resume_cycle(
                cycle_id,
                proposal(
                    "durable unknown",
                    json!({ "interpretations": [], "answer": "recovered" }),
                ),
            )
            .unwrap();
        let CycleProgress::Completed(outcome) = completed else {
            panic!("recovered continuation should complete");
        };
        assert_eq!(outcome.answer, Some(Value::Text("recovered".into())));
    }

    let mut reopened = Engine::open(&path_text).unwrap();
    assert!(
        reopened
            .resume_cycle(
                cycle_id,
                proposal(
                    "durable unknown",
                    json!({ "interpretations": [], "answer": "again" }),
                ),
            )
            .is_err()
    );
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn teacher_provenance_must_match_the_pending_request() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("TRIPLE", MutabilityClass::Definitional);
    engine.admin_insert_concept(&concept).unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let first = Concept::new("FIRST", MutabilityClass::Definitional);
    let second = Concept::new("SECOND", MutabilityClass::Definitional);
    engine.admin_insert_concept(&first).unwrap();
    engine.admin_insert_concept(&second).unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("BREAK", MutabilityClass::Definitional);
    engine.admin_insert_concept(&concept).unwrap();
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
    engine.admin_insert_procedure(&procedure).unwrap();

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
fn failed_local_run_is_persisted_before_teacher_escalation() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("BREAK", MutabilityClass::Definitional);
    engine.admin_insert_concept(&concept).unwrap();
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
    engine.admin_insert_procedure(&procedure).unwrap();

    let started = engine.begin_cycle(cycle_input("break 9", true)).unwrap();
    let CycleProgress::NeedTeacher { cycle_id, request } = started else {
        panic!("failed run should ask while a teacher is available");
    };
    assert_eq!(engine.episodes().count().unwrap(), 1);
    let failures = engine.episodes().list_failures(10).unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].action.as_deref(),
        Some("failed:awaiting-teacher")
    );
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
    assert_eq!(engine.episodes().count().unwrap(), 2);
}

#[test]
fn run_then_teacher_procedure_preserves_both_execution_attempts() {
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("RECOVER", MutabilityClass::Definitional);
    engine.admin_insert_concept(&concept).unwrap();
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
    engine.admin_insert_procedure(&failing).unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let mut concept = Concept::new("RETIRED", MutabilityClass::Definitional);
    concept.lifecycle = spoon_core::Lifecycle::Retired;
    engine.admin_insert_concept(&concept).unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("BREAK", MutabilityClass::Definitional);
    engine.admin_insert_concept(&concept).unwrap();
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
    let mut engine = Engine::in_memory_with_admin("test-admin").unwrap();
    let concept = Concept::new("DOUBLE", MutabilityClass::Definitional);
    engine.admin_insert_concept(&concept).unwrap();
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
