use ekg_server::RpcServer;
use serde_json::{Value, json};

fn call(server: &mut RpcServer, id: u64, method: &str, params: Value) -> Value {
    let response: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], id);
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

#[test]
fn malformed_json_and_unknown_methods_return_json_rpc_errors() {
    let mut server = RpcServer::in_memory().unwrap();

    let parse_error: Value = serde_json::from_str(&server.handle_line("{")).unwrap();
    assert_eq!(parse_error["error"]["code"], -32700);

    let missing: Value = serde_json::from_str(&server.handle_line(
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "missing", "params": {} }).to_string(),
    ))
    .unwrap();
    assert_eq!(missing["id"], 7);
    assert_eq!(missing["error"]["code"], -32601);
}

#[test]
fn concepts_can_be_created_and_listed() {
    let mut server = RpcServer::in_memory().unwrap();

    let created = call(
        &mut server,
        1,
        "concept.create",
        json!({ "name": "DOUBLE", "mutability": "Definitional" }),
    );
    let listed = call(&mut server, 2, "concept.list", json!({}));

    assert_eq!(created["name"], "DOUBLE");
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], created["id"]);
}

#[test]
fn kitchen_cycle_executes_records_and_replays_double() {
    let mut server = RpcServer::in_memory().unwrap();
    let procedure = call(
        &mut server,
        1,
        "procedure.create",
        json!({
            "name": "DOUBLE",
            "params": [{ "name": "x", "description": null }],
            "body": {
                "BinOp": {
                    "op": "Mul",
                    "left": { "Var": "x" },
                    "right": { "Literal": 2 }
                }
            }
        }),
    );

    let executed = call(
        &mut server,
        2,
        "procedure.execute",
        json!({
            "procedureId": procedure["id"],
            "inputs": { "x": 7 },
            "prediction": 14
        }),
    );

    assert_eq!(executed["value"], 14);
    assert_eq!(executed["episode"]["evaluation"]["success"], true);
    assert_eq!(executed["trace"]["steps"][0]["procedure_version"], 1);

    let episode_id = executed["episode"]["id"].clone();
    let stored = call(
        &mut server,
        3,
        "episode.get",
        json!({ "episodeId": episode_id }),
    );
    assert_eq!(stored["observed_result"], 14);

    let replayed = call(
        &mut server,
        4,
        "episode.replay",
        json!({ "episodeId": episode_id, "substitutions": { "x": 9 } }),
    );
    assert_eq!(replayed["value"], 18);
}

#[test]
fn cycle_begin_and_resume_use_camel_case_wire_fields() {
    let mut server = RpcServer::in_memory().unwrap();
    let started = call(
        &mut server,
        1,
        "cycle.begin",
        json!({
            "situation": "what is the answer?",
            "environment": {},
            "assumptions": [],
            "budget": {
                "maxExecSteps": 100,
                "maxContextItems": 16,
                "maxTeacherTurns": 1
            },
            "teacherAllowed": true
        }),
    );

    assert_eq!(started["status"], "need_teacher");
    assert!(started.get("cycleId").is_some());
    assert!(started.get("cycle_id").is_none());
    assert_eq!(started["request"]["situation"], "what is the answer?");
    assert!(started["request"].get("specificQuestion").is_some());
    assert!(started["request"].get("desiredOutput").is_some());

    let resumed = call(
        &mut server,
        2,
        "cycle.resume",
        json!({
            "cycleId": started["cycleId"],
            "proposal": {
                "content": { "interpretations": [], "answer": 42 },
                "source": "human:test",
                "status": "unverified",
                "provenance": {
                    "provider": "human",
                    "teacher": "human:test",
                    "requestId": "request-1",
                    "generatedAt": "2026-08-22T00:00:00.000Z",
                    "situation": "what is the answer?"
                }
            }
        }),
    );

    assert_eq!(resumed["status"], "completed");
    assert_eq!(resumed["cycleId"], started["cycleId"]);
    assert!(resumed.get("cycle_id").is_none());
    assert_eq!(resumed["disposition"], "provisional");
    assert_eq!(resumed["answer"], 42);
    assert_eq!(resumed["episode"]["prediction"], 42);
    assert!(resumed["episode"]["observed_result"].is_null());
}

#[test]
fn cycle_begin_rejects_snake_case_transport_fields() {
    let mut server = RpcServer::in_memory().unwrap();
    let response: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "cycle.begin",
                "params": {
                    "situation": "unknown",
                    "environment": {},
                    "assumptions": [],
                    "budget": {
                        "max_exec_steps": 100,
                        "max_context_items": 16,
                        "max_teacher_turns": 1
                    },
                    "teacher_allowed": true
                }
            })
            .to_string(),
        ),
    )
    .unwrap();

    assert_eq!(response["error"]["code"], -32602);
}

#[test]
fn cycle_abort_records_provider_failure_as_a_terminal_attempt() {
    let mut server = RpcServer::in_memory().unwrap();
    let started = call(
        &mut server,
        1,
        "cycle.begin",
        json!({
            "situation": "unknown",
            "environment": {},
            "assumptions": [],
            "budget": {
                "maxExecSteps": 100,
                "maxContextItems": 16,
                "maxTeacherTurns": 1
            },
            "teacherAllowed": true
        }),
    );
    let aborted = call(
        &mut server,
        2,
        "cycle.abort",
        json!({
            "cycleId": started["cycleId"],
            "reason": "provider unavailable"
        }),
    );

    assert_eq!(aborted["status"], "completed");
    assert_eq!(aborted["disposition"], "abstained");
    assert_eq!(aborted["episode"]["cost"]["rung_reached"], "Abstain");
    assert_eq!(
        aborted["episode"]["teacher_interaction"]["providerError"],
        "provider unavailable"
    );
}
