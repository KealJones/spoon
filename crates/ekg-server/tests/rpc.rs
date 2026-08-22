use ekg_server::RpcServer;
use serde_json::{Value, json};

fn call(server: &RpcServer, id: u64, method: &str, params: Value) -> Value {
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
    let server = RpcServer::in_memory().unwrap();

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
    let server = RpcServer::in_memory().unwrap();

    let created = call(
        &server,
        1,
        "concept.create",
        json!({ "name": "DOUBLE", "mutability": "Definitional" }),
    );
    let listed = call(&server, 2, "concept.list", json!({}));

    assert_eq!(created["name"], "DOUBLE");
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], created["id"]);
}

#[test]
fn kitchen_cycle_executes_records_and_replays_double() {
    let server = RpcServer::in_memory().unwrap();
    let procedure = call(
        &server,
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
        &server,
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
        &server,
        3,
        "episode.get",
        json!({ "episodeId": episode_id }),
    );
    assert_eq!(stored["observed_result"], 14);

    let replayed = call(
        &server,
        4,
        "episode.replay",
        json!({ "episodeId": episode_id, "substitutions": { "x": 9 } }),
    );
    assert_eq!(replayed["value"], 18);
}
