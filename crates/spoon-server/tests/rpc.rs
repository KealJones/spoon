use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use serde_json::{Value, json};
use spoon_core::{
    BinOp, Concept, Condition, Evaluation, Expr, MutabilityClass, Param, Procedure,
    Value as SpoonValue, VerifiabilityTier,
};
use spoon_engine::{
    CapabilityBundle, CapabilityTest, Effect, Engine, EngineError, NativePrimitive, Permission,
    ResourceBounds, bundle_content_id, discover_interface,
};
use spoon_episode::{EpisodeFeedback, FeedbackSource};
use spoon_server::{CapabilityHostAdapters, RpcServer};

const ADMIN_TOKEN: &str = "test-bootstrap-secret";

fn test_server() -> RpcServer {
    RpcServer::in_memory()
        .unwrap()
        .with_admin_token(ADMIN_TOKEN)
        .unwrap()
}

fn call(server: &mut RpcServer, id: u64, method: &str, mut params: Value) -> Value {
    if matches!(
        method,
        "concept.create"
            | "concept.update"
            | "concept.delete"
            | "relationship.create"
            | "relationship.update"
            | "relationship.delete"
            | "procedure.create"
            | "procedure.update"
            | "procedure.delete"
            | "capability.grant"
            | "capability.revoke"
            | "capability.provisionWebFetch"
            | "observation.recordAuthenticated"
            | "adaptation.applyOffline"
            | "contradiction.record"
            | "contradiction.refine"
    ) {
        params
            .as_object_mut()
            .unwrap()
            .insert("adminToken".into(), json!(ADMIN_TOKEN));
    }
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

fn raw_call(server: &mut RpcServer, id: u64, method: &str, params: Value) -> Value {
    serde_json::from_str(
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
    .unwrap()
}

struct TestDirectory(std::path::PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("spoon-rpc-files-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn file_bundle(logical_path: &str, primitive: NativePrimitive, max_bytes: u64) -> CapabilityBundle {
    let operation_name = match primitive {
        NativePrimitive::FileRead => "read_file",
        NativePrimitive::FileWrite => "write_file",
        _ => panic!("test helper only constructs file capabilities"),
    };
    let description = spoon_engine::InterfaceDescription {
        source: format!("rpc-file-{operation_name}"),
        fingerprint: format!("rpc-file-{operation_name}-v1"),
        operations: vec![spoon_engine::DiscoveredOperation {
            name: operation_name.into(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            host: "placeholder.invalid".into(),
            method: "GET".into(),
            response_fixture: json!({}),
        }],
    };
    let mut bundle = discover_interface(&description).unwrap();
    let procedure = &mut bundle.procedures[0];
    procedure.primitive = primitive.clone();
    procedure.input_schema = match primitive {
        NativePrimitive::FileWrite => json!({"type": "string"}),
        _ => json!({"type": "object"}),
    };
    procedure.output_schema = json!({"type": "object"});
    procedure.contract = json!({"path": logical_path, "bytes": max_bytes});
    procedure.permissions = vec![match primitive {
        NativePrimitive::FileRead => Permission::FileReadPrefix {
            path_prefix: "workspace".into(),
        },
        NativePrimitive::FileWrite => Permission::FileWritePrefix {
            path_prefix: "workspace".into(),
        },
        _ => unreachable!(),
    }];
    procedure.effects = vec![match primitive {
        NativePrimitive::FileRead => Effect::FileRead,
        NativePrimitive::FileWrite => Effect::FileWrite,
        _ => unreachable!(),
    }];
    procedure.bounds = ResourceBounds {
        max_bytes,
        max_steps: 8,
        max_millis: 2_000,
    };
    procedure.tests = vec![CapabilityTest {
        name: "portable fixture".into(),
        input: match primitive {
            NativePrimitive::FileWrite => json!("fixture"),
            _ => json!({}),
        },
        expected_output: match primitive {
            NativePrimitive::FileWrite => json!({"bytesWritten": 7}),
            _ => json!({"bytes": [102, 105, 120, 116, 117, 114, 101]}),
        },
        fixture_output: match primitive {
            NativePrimitive::FileWrite => json!({"bytesWritten": 7}),
            _ => json!({"bytes": [102, 105, 120, 116, 117, 114, 101]}),
        },
    }];
    bundle.content_id = bundle_content_id(&bundle).unwrap();
    bundle
}

fn revalidate_without_grant(
    server: &mut RpcServer,
    bundle: &CapabilityBundle,
) -> (String, String, Value) {
    let imported = call(server, 900, "capability.import", json!({"bundle": bundle}));
    let content_id = imported["contentId"].as_str().unwrap().to_owned();
    let validation_episode = call(
        server,
        901,
        "observation.recordAuthenticated",
        json!({
            "predicate": "capability.fixture",
            "value": true,
            "scope": {},
            "evaluation": {
                "tier": "Hard",
                "success": true,
                "details": "fixture matched",
                "surprise": 0.0
            },
            "verifierIdentity": "rpc-file-test"
        }),
    );
    call(
        server,
        902,
        "capability.revalidate",
        json!({
            "contentId": content_id,
            "validation": {
                "passed": true,
                "validationEpisodes": [validation_episode["id"]],
                "environmentDigest": "rpc-file-test-v1"
            }
        }),
    );
    let permission = serde_json::to_value(&bundle.procedures[0].permissions[0]).unwrap();
    (content_id, bundle.procedures[0].id.clone(), permission)
}

fn revalidate_and_grant(
    server: &mut RpcServer,
    bundle: &CapabilityBundle,
) -> (String, String, Value) {
    let (content_id, procedure_id, permission) = revalidate_without_grant(server, bundle);
    call(
        server,
        903,
        "capability.grant",
        json!({"contentId": content_id, "permission": permission}),
    );
    (content_id, procedure_id, permission)
}

#[test]
fn capability_invoke_reaches_bounded_web_fetch_adapter() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server_thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 12\r\n\r\nhello spoon!",
            )
            .unwrap();
    });

    let adapters = CapabilityHostAdapters::with_web_fetch(
        vec![address.clone()],
        ResourceBounds {
            max_bytes: 4096,
            max_steps: 8,
            max_millis: 2_000,
        },
    )
    .unwrap();
    let mut server = test_server().with_capability_host_adapters(adapters);
    let bundle = discover_interface(&spoon_engine::InterfaceDescription {
        source: "web-fetch-test".into(),
        fingerprint: "web-fetch-test-v1".into(),
        operations: vec![spoon_engine::DiscoveredOperation {
            name: "web.fetch".into(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            host: address.clone(),
            method: "GET".into(),
            response_fixture: json!({"status": 200}),
        }],
    })
    .unwrap();
    let (content_id, procedure_id, _) = revalidate_and_grant(&mut server, &bundle);
    let denied = raw_call(
        &mut server,
        939,
        "capability.invoke",
        json!({
            "contentId": content_id,
            "procedureId": procedure_id,
            "input": {"url": "http://example.invalid/escape"}
        }),
    );
    assert_eq!(
        denied["error"]["data"]["kind"],
        "capability_invocation_failed"
    );
    let fetched = call(
        &mut server,
        940,
        "capability.invoke",
        json!({
            "contentId": content_id,
            "procedureId": procedure_id,
            "input": {"url": format!("http://{address}/health")}
        }),
    );
    assert_eq!(fetched["output"]["status"], 200);
    assert_eq!(fetched["output"]["body"], "hello spoon!");
    server_thread.join().unwrap();
}

#[test]
fn provisioning_web_fetch_creates_a_teacher_authorable_capability_without_a_grant() {
    let adapters = CapabilityHostAdapters::with_web_fetch(
        vec!["www.google.com".into()],
        ResourceBounds {
            max_bytes: 4096,
            max_steps: 8,
            max_millis: 2_000,
        },
    )
    .unwrap();
    let mut server = test_server().with_capability_host_adapters(adapters);
    let provisioned = call(
        &mut server,
        941,
        "capability.provisionWebFetch",
        json!({"host": "www.google.com"}),
    );
    assert_eq!(provisioned["capability"]["locallyValidated"], true);
    assert_eq!(provisioned["permissionGranted"], false);
    assert!(
        provisioned["procedureId"]
            .as_str()
            .is_some_and(|id| id.ends_with(":web.fetch"))
    );
    let inventory = call(&mut server, 942, "capability.list", json!({}));
    assert_eq!(inventory["imported"].as_array().map(Vec::len), Some(1));
    assert!(
        inventory["nativeBoundaries"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| {
                item["kind"] == "network_request" && item["hostAdapterConfigured"] == true
            }))
    );
}

#[test]
fn capability_rpc_round_trip_keeps_imports_provisional_and_grants_local() {
    let mut server = test_server();
    let bundle = call(
        &mut server,
        1,
        "capability.discover",
        json!({
            "source": "weather-api",
            "fingerprint": "weather-v1",
            "operations": [{
                "name": "forecast",
                "inputSchema": {"type": "object"},
                "outputSchema": {"type": "object"},
                "host": "api.example.test",
                "method": "GET",
                "responseFixture": {"temperature": 72}
            }]
        }),
    );
    let imported = call(
        &mut server,
        2,
        "capability.import",
        json!({"bundle": bundle}),
    );
    assert_eq!(imported["status"], "quarantined");
    let content_id = imported["contentId"].as_str().unwrap();
    let validation_episode = call(
        &mut server,
        2,
        "observation.recordAuthenticated",
        json!({
            "predicate": "weather.forecast",
            "value": {"temperature": 72},
            "scope": {},
            "evaluation": {
                "tier": "Hard",
                "success": true,
                "details": "fixture matched",
                "surprise": 0.0
            },
            "verifierIdentity": "rpc-test"
        }),
    );
    let validation_episode_id = validation_episode["id"].as_str().unwrap();
    let validation = call(
        &mut server,
        3,
        "capability.revalidate",
        json!({
            "contentId": content_id,
            "validation": {
                "passed": true,
                "validationEpisodes": [validation_episode_id],
                "environmentDigest": "local"
            }
        }),
    );
    assert_eq!(validation["status"], "provisional");
    let permission = json!({"kind": "network_host", "host": "api.example.test"});
    let granted = call(
        &mut server,
        4,
        "capability.grant",
        json!({"contentId": content_id, "permission": permission}),
    );
    assert_eq!(granted["granted"], true);
    let exported = call(
        &mut server,
        5,
        "capability.export",
        json!({"contentId": content_id}),
    );
    assert_eq!(exported["bundle"]["contentId"], content_id);
}

#[test]
fn capability_invoke_reaches_real_scoped_file_effects_and_revocation_is_immediate() {
    let directory = TestDirectory::new();
    let bounds = ResourceBounds {
        max_bytes: 4096,
        max_steps: 16,
        max_millis: 2_000,
    };
    let adapters =
        CapabilityHostAdapters::with_scoped_files("workspace", &directory.0, bounds).unwrap();
    let mut server = test_server().with_capability_host_adapters(adapters);
    let path = directory.0.join("effect.txt");

    let write_bundle = file_bundle("workspace/effect.txt", NativePrimitive::FileWrite, 4096);
    let (write_content_id, write_procedure_id, write_permission) =
        revalidate_and_grant(&mut server, &write_bundle);
    let written = call(
        &mut server,
        910,
        "capability.invoke",
        json!({
            "contentId": write_content_id,
            "procedureId": write_procedure_id,
            "input": "real temporary-directory effect"
        }),
    );
    assert_eq!(written["output"]["bytesWritten"], 31);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "real temporary-directory effect"
    );
    assert_eq!(written["redacted"], true);
    assert_eq!(written["receipt"]["redacted"], true);
    assert!(written["receipt"].get("target").is_none());
    assert!(written["receipt"].get("permission").is_none());

    let read_bundle = file_bundle("workspace/effect.txt", NativePrimitive::FileRead, 4096);
    let (read_content_id, read_procedure_id, _) = revalidate_and_grant(&mut server, &read_bundle);
    let read = call(
        &mut server,
        911,
        "capability.invoke",
        json!({
            "contentId": read_content_id,
            "procedureId": read_procedure_id,
            "input": {}
        }),
    );
    assert_eq!(
        read["output"]["bytes"],
        serde_json::to_value(b"real temporary-directory effect").unwrap()
    );

    call(
        &mut server,
        912,
        "capability.revoke",
        json!({
            "contentId": write_content_id,
            "permission": write_permission
        }),
    );
    let denied = raw_call(
        &mut server,
        913,
        "capability.invoke",
        json!({
            "contentId": write_content_id,
            "procedureId": write_procedure_id,
            "input": "must not be written"
        }),
    );
    assert_eq!(
        denied["error"]["data"]["kind"],
        "capability_authorization_failed"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "real temporary-directory effect"
    );
}

#[test]
fn capability_invoke_rejects_malformed_ambient_authority_and_resource_overruns() {
    let directory = TestDirectory::new();
    let bounds = ResourceBounds {
        max_bytes: 4096,
        max_steps: 16,
        max_millis: 2_000,
    };
    let adapters =
        CapabilityHostAdapters::with_scoped_files("workspace", &directory.0, bounds).unwrap();
    let mut server = test_server().with_capability_host_adapters(adapters);
    let path = directory.0.join("bounded.txt");
    let bundle = file_bundle("workspace/bounded.txt", NativePrimitive::FileWrite, 64);
    let (content_id, procedure_id, permission) = revalidate_without_grant(&mut server, &bundle);

    let ungranted = raw_call(
        &mut server,
        919,
        "capability.invoke",
        json!({
            "contentId": content_id,
            "procedureId": procedure_id,
            "input": "payload"
        }),
    );
    assert_eq!(
        ungranted["error"]["data"]["kind"],
        "capability_authorization_failed"
    );
    assert!(!path.exists());
    call(
        &mut server,
        918,
        "capability.grant",
        json!({"contentId": content_id, "permission": permission}),
    );

    let ambient = raw_call(
        &mut server,
        920,
        "capability.invoke",
        json!({
            "contentId": content_id,
            "procedureId": procedure_id,
            "input": "payload",
            "permissionMode": "full-access",
            "root": "/"
        }),
    );
    assert_eq!(ambient["error"]["code"], -32602);
    assert_eq!(ambient["error"]["data"]["kind"], "invalid_params");

    let overrun = raw_call(
        &mut server,
        921,
        "capability.invoke",
        json!({
            "contentId": content_id,
            "procedureId": procedure_id,
            "input": "x".repeat(65)
        }),
    );
    assert_eq!(
        overrun["error"]["data"]["kind"],
        "capability_invocation_failed"
    );
    assert_eq!(overrun["error"]["data"]["redacted"], true);
    assert!(overrun["error"]["data"].get("cause").is_none());
    assert!(!path.exists());

    let protocol_overrun = raw_call(
        &mut server,
        922,
        "capability.invoke",
        json!({
            "contentId": content_id,
            "procedureId": procedure_id,
            "input": "x".repeat(1024 * 1024 + 1)
        }),
    );
    assert_eq!(protocol_overrun["error"]["code"], -32602);
    assert_eq!(
        protocol_overrun["error"]["data"]["kind"],
        "capability_input_too_large"
    );
}

#[cfg(unix)]
#[test]
fn capability_invoke_rejects_symlink_escape_and_unsupported_primitive() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let outside = TestDirectory::new();
    let outside_path = outside.0.join("outside.txt");
    std::fs::write(&outside_path, "unchanged").unwrap();
    let link = directory.0.join("escape.txt");
    symlink(&outside_path, &link).unwrap();
    let bounds = ResourceBounds {
        max_bytes: 4096,
        max_steps: 16,
        max_millis: 2_000,
    };
    let adapters =
        CapabilityHostAdapters::with_scoped_files("workspace", &directory.0, bounds).unwrap();
    let mut server = test_server().with_capability_host_adapters(adapters);
    let bundle = file_bundle("workspace/escape.txt", NativePrimitive::FileWrite, 4096);
    let (content_id, procedure_id, _) = revalidate_and_grant(&mut server, &bundle);
    let escaped = raw_call(
        &mut server,
        930,
        "capability.invoke",
        json!({
            "contentId": content_id,
            "procedureId": procedure_id,
            "input": "escaped"
        }),
    );
    assert_eq!(
        escaped["error"]["data"]["kind"],
        "capability_invocation_failed"
    );
    assert_eq!(std::fs::read_to_string(&outside_path).unwrap(), "unchanged");
    assert!(!escaped.to_string().contains(outside_path.to_str().unwrap()));

    let network_bundle = discover_interface(&spoon_engine::InterfaceDescription {
        source: "unsupported-network".into(),
        fingerprint: "unsupported-network-v1".into(),
        operations: vec![spoon_engine::DiscoveredOperation {
            name: "request".into(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            host: "api.example.test".into(),
            method: "GET".into(),
            response_fixture: json!({"ok": true}),
        }],
    })
    .unwrap();
    let (network_content_id, network_procedure_id, _) =
        revalidate_and_grant(&mut server, &network_bundle);
    let unsupported = raw_call(
        &mut server,
        931,
        "capability.invoke",
        json!({
            "contentId": network_content_id,
            "procedureId": network_procedure_id,
            "input": {}
        }),
    );
    assert_eq!(
        unsupported["error"]["data"]["kind"],
        "capability_adapter_unavailable"
    );
}

#[test]
fn metrics_goals_and_curiosity_endpoints_are_bounded_and_camel_case() {
    let mut server = test_server();
    let metrics = call(&mut server, 50, "metrics.snapshot", json!({}));
    assert_eq!(metrics["episodeCount"], 0);
    assert_eq!(metrics["verifiedAnswerCount"], 0);
    assert_eq!(metrics["phase6"]["teacherInteractionEpisodes"], 0);
    assert_eq!(metrics["phase6"]["replayPreservedSkillVerdicts"], 0);
    assert_eq!(metrics["phase6"]["postPromotionSkillSuccesses"], 0);
    assert_eq!(metrics["section38"]["measurements"], 0);
    assert_eq!(
        metrics["section38"]["metrics"].as_array().unwrap().len(),
        12
    );
    assert!(metrics["intuition"]["indexedDocuments"].is_number());
    let ranking = call(
        &mut server,
        56,
        "intuition.evaluateRanking",
        json!({"query": "math", "candidateLimit": 8, "holdoutExamples": 4}),
    );
    assert_eq!(ranking["heldOutExamples"], 0);

    let standing = call(
        &mut server,
        51,
        "goal.create",
        json!({"kind": "standing", "statement": "remain accurate"}),
    );
    assert_eq!(standing["immutable"], true);
    assert_eq!(standing["kind"], "standing");
    assert_eq!(
        call(&mut server, 52, "goal.list", json!({}))
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let gap = json!({
        "id": "gap-1",
        "kind": "failed_prediction",
        "statement": "the prediction was surprising",
        "blastRadius": 2.0,
        "goalRelevance": 3.0,
        "learningProgress": 1.0,
        "costToClose": 1.0,
        "valueScore": 6.0,
        "sourceEpisode": null,
        "resolved": false,
        "createdAt": 1
    });
    assert_eq!(
        call(&mut server, 53, "curiosity.record", gap)["recorded"],
        true
    );
    assert_eq!(
        call(&mut server, 54, "curiosity.rank", json!({"limit": 1}))[0]["id"],
        "gap-1"
    );
    let clock = call(
        &mut server,
        55,
        "primitive.observe",
        json!({"target": "clock"}),
    );
    assert_eq!(clock["receipt"]["target"], "clock");
    assert_eq!(clock["output"]["source"], "native:clock");
}

#[test]
fn session_lifecycle_is_public_and_preserves_isolation_metadata() {
    let mut server = test_server();
    let session = call(
        &mut server,
        700,
        "session.create",
        json!({"name": "private-chat", "visibility": "isolated"}),
    );
    assert_eq!(session["visibility"], "isolated");
    assert_eq!(session["state"], "active");
    let listed = call(&mut server, 701, "session.list", json!({}));
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let found = call(
        &mut server,
        702,
        "session.get",
        json!({"idOrName": "private-chat"}),
    );
    assert_eq!(found["id"], session["id"]);
    let ended = call(
        &mut server,
        703,
        "session.end",
        json!({"idOrName": "private-chat"}),
    );
    assert_eq!(ended["state"], "ended");
    assert!(ended["endedAt"].is_number());
}

fn complete_cycle(server: &mut RpcServer, id: u64, situation: &str, session_id: &str) -> Value {
    let started = call(
        server,
        id,
        "cycle.begin",
        json!({
            "situation": situation,
            "environment": {},
            "assumptions": [],
            "budget": {
                "maxExecSteps": 100,
                "maxContextItems": 16,
                "maxTeacherTurns": 1
            },
            "teacherAllowed": true,
            "sessionId": session_id,
            "recallMode": "session"
        }),
    );
    assert_eq!(started["status"], "need_teacher");
    call(
        server,
        id + 1,
        "cycle.resume",
        json!({
            "cycleId": started["cycleId"],
            "proposal": {
                "content": { "interpretations": [], "answer": situation },
                "source": "human:test",
                "status": "unverified",
                "provenance": {
                    "provider": "human",
                    "teacher": "human:test",
                    "requestId": format!("request-{id}"),
                    "generatedAt": "2026-08-22T00:00:00.000Z",
                    "situation": situation
                }
            }
        }),
    )
}

#[test]
fn episode_list_filters_by_session() {
    let mut server = test_server();
    let alpha = call(&mut server, 800, "session.create", json!({"name": "alpha"}));
    let beta = call(&mut server, 801, "session.create", json!({"name": "beta"}));
    let alpha_id = alpha["id"].as_str().unwrap();
    let beta_id = beta["id"].as_str().unwrap();

    complete_cycle(&mut server, 810, "alpha question", alpha_id);
    complete_cycle(&mut server, 820, "beta question", beta_id);

    let alpha_episodes = call(
        &mut server,
        830,
        "episode.list",
        json!({"sessionId": alpha_id}),
    );
    let beta_episodes = call(
        &mut server,
        831,
        "episode.list",
        json!({"sessionId": beta_id}),
    );
    let alpha_rows = alpha_episodes.as_array().unwrap();
    let beta_rows = beta_episodes.as_array().unwrap();
    assert_eq!(alpha_rows.len(), 1);
    assert_eq!(beta_rows.len(), 1);
    assert_eq!(alpha_rows[0]["situation"], "alpha question");
    assert_eq!(beta_rows[0]["situation"], "beta question");
}

#[test]
fn falsification_telemetry_rpc_rejects_teacher_off_leakage_and_exposes_samples() {
    let mut server = test_server();
    let run = call(
        &mut server,
        901,
        "telemetry.createRun",
        json!({"label":"rpc telemetry", "benchmark":"unit"}),
    );
    let run_id = run["id"].as_str().unwrap();
    let valid = json!({
        "domain":"math", "family":"doubles", "cohort":"heldOut",
        "probeId":"heldout-7", "noveltyIdentity":"double-7",
        "teacherMode":"off", "teacherUsed":false, "teacherCalls":0,
        "rung":"Direct", "steps":1, "candidates":1, "traceSteps":1,
        "cost":1.0, "abstained":false, "clarified":false,
        "confidence":0.9, "groundingTier":"strong", "usedSkillId":"double",
        "correct":true, "regressionProbe":true,
        "attributionCorrect":true, "attributionCost":0.2
    });
    let recorded = call(
        &mut server,
        902,
        "telemetry.recordMeasurement",
        json!({"runId":run_id, "measurement":valid}),
    );
    assert_eq!(recorded["runId"], run_id);
    let metrics = call(&mut server, 903, "metrics.snapshot", json!({}));
    assert_eq!(metrics["section38"]["measurements"], 1);
    assert_eq!(metrics["section38"]["metrics"][1]["sampleSize"], 1);

    let bad: Value = serde_json::from_str(&server.handle_line(
        &json!({"jsonrpc":"2.0", "id":904, "method":"telemetry.recordMeasurement", "params":{"runId":run_id, "measurement":{
          "domain":"math", "family":"doubles", "cohort":"training",
          "probeId":"leak", "noveltyIdentity":"leak", "teacherMode":"off",
          "teacherUsed":true, "teacherCalls":1, "rung":"Ask", "steps":1,
          "candidates":1, "traceSteps":1, "cost":1.0, "abstained":false,
          "clarified":false, "groundingTier":"teacher"
        }}}).to_string(),
    )).unwrap();
    assert_eq!(bad["error"]["data"]["kind"], "application_error");
}

#[test]
fn admin_revisions_require_exact_versions_and_expose_immutable_history() {
    let mut server = test_server();
    let concept = call(
        &mut server,
        1,
        "concept.create",
        json!({ "name": "VERSIONED", "description": "v1" }),
    );
    let mut revised = concept.clone();
    revised["description"] = json!("v2");
    let concept_revision = call(
        &mut server,
        2,
        "concept.update",
        json!({ "concept": revised, "expectedVersion": 1 }),
    );
    assert_eq!(concept_revision["version"], 2);
    assert_eq!(concept_revision["concept"]["description"], "v2");

    let v1 = call(
        &mut server,
        3,
        "concept.getVersion",
        json!({ "conceptId": concept["id"], "version": 1 }),
    );
    let history = call(
        &mut server,
        4,
        "concept.listVersions",
        json!({ "conceptId": concept["id"] }),
    );
    assert_eq!(v1["version"], 1);
    assert_eq!(v1["concept"]["description"], "v1");
    assert_eq!(history.as_array().unwrap().len(), 2);

    let stale: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "concept.update",
                "params": {
                    "concept": concept_revision["concept"],
                    "expectedVersion": 1,
                    "adminToken": ADMIN_TOKEN
                }
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(stale["error"]["data"]["kind"], "revision_conflict");
    assert_eq!(stale["error"]["data"]["expectedVersion"], 1);
    assert_eq!(stale["error"]["data"]["actualVersion"], 2);

    let target = call(
        &mut server,
        6,
        "concept.create",
        json!({ "name": "TARGET" }),
    );
    let relationship = call(
        &mut server,
        7,
        "relationship.create",
        json!({
            "source": concept["id"],
            "target": target["id"],
            "kind": "depends-on",
            "strength": 0.5
        }),
    );
    let mut revised_relationship = relationship.clone();
    revised_relationship["strength"] = json!(0.75);
    let relationship_revision = call(
        &mut server,
        8,
        "relationship.update",
        json!({ "relationship": revised_relationship, "expectedVersion": 1 }),
    );
    assert_eq!(relationship_revision["version"], 2);
    let relationship_history = call(
        &mut server,
        9,
        "relationship.listVersions",
        json!({ "relationshipId": relationship["id"] }),
    );
    assert_eq!(relationship_history[0]["relationship"]["strength"], 0.5);
    assert_eq!(relationship_history[1]["relationship"]["strength"], 0.75);

    let procedure = call(
        &mut server,
        10,
        "procedure.create",
        json!({ "name": "VERSIONED_PROC", "params": [], "body": { "Literal": 1 } }),
    );
    let mut revised_procedure = procedure.clone();
    revised_procedure["version"] = json!(2);
    revised_procedure["body"] = json!({ "Literal": 2 });
    let procedure_revision = call(
        &mut server,
        11,
        "procedure.update",
        json!({ "procedure": revised_procedure, "expectedVersion": 1 }),
    );
    assert_eq!(procedure_revision["version"], 2);
    let procedure_history = call(
        &mut server,
        12,
        "procedure.listVersions",
        json!({ "procedureId": procedure["id"] }),
    );
    assert_eq!(
        procedure_history[0]["procedure"]["body"],
        json!({ "Literal": 1 })
    );
    assert_eq!(
        procedure_history[1]["procedure"]["body"],
        json!({ "Literal": 2 })
    );
}

#[test]
fn malformed_json_and_unknown_methods_return_json_rpc_errors() {
    let mut server = test_server();

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
fn relationship_list_is_read_only_bounded_and_deterministic() {
    let mut server = test_server();
    let source = call(
        &mut server,
        1,
        "concept.create",
        json!({ "name": "SOURCE" }),
    );
    let target = call(
        &mut server,
        2,
        "concept.create",
        json!({ "name": "TARGET" }),
    );
    let first = call(
        &mut server,
        3,
        "relationship.create",
        json!({
            "source": source["id"],
            "target": target["id"],
            "kind": "supports"
        }),
    );
    let second = call(
        &mut server,
        4,
        "relationship.create",
        json!({
            "source": source["id"],
            "target": target["id"],
            "kind": "tests"
        }),
    );

    let limited = call(&mut server, 5, "relationship.list", json!({ "limit": 1 }));
    assert_eq!(limited.as_array().unwrap().len(), 1);
    let all = call(
        &mut server,
        6,
        "relationship.list",
        json!({ "limit": 10_000 }),
    );
    assert_eq!(all.as_array().unwrap().len(), 2);
    let ids: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert!(ids[0] < ids[1]);

    let empty = call(&mut server, 7, "relationship.list", json!({ "limit": 0 }));
    assert_eq!(empty, json!([]));
    let after = call(
        &mut server,
        8,
        "relationship.get",
        json!({ "relationshipId": first["id"] }),
    );
    assert_eq!(after["id"], first["id"]);
    assert_eq!(after["kind"], first["kind"]);
    assert_ne!(first["id"], second["id"]);
}

#[test]
fn phase_two_requests_reject_unknown_snake_case_and_offline_capability_fields() {
    let mut server = test_server();
    let mut invalid_requests = vec![
        (
            "adaptation.get",
            json!({
                "planId": "00000000-0000-0000-0000-000000000001",
                "plan_id": "00000000-0000-0000-0000-000000000001"
            }),
        ),
        (
            "adaptation.apply",
            json!({
                "planId": "00000000-0000-0000-0000-000000000001",
                "idempotencyKey": "apply-1",
                "appliedAt": 1,
                "offlineCapability": "caller-forged"
            }),
        ),
        ("contradiction.list", json!({ "unexpected": true })),
        ("contradiction.get", json!({ "contradiction_id": 1 })),
        ("contradiction.get", json!({ "contradictionId": 0 })),
    ];
    let plan = json!({
        "idempotencyKey": "strict-plan",
        "analysis": {
            "episodeId": "00000000-0000-0000-0000-000000000001",
            "candidates": [],
            "budget": { "topK": 1, "maxReplays": 0, "maxReplaySteps": 0 }
        },
        "attribution": {
            "suspect": {
                "procedure": "00000000-0000-0000-0000-000000000002",
                "version": 1,
                "traceStep": 0
            },
            "mechanism": "contract_violation"
        },
        "evidence": [{
            "episodeId": "00000000-0000-0000-0000-000000000001"
        }],
        "target": { "kind": "unusual_input", "reason": "edge case" },
        "createdAt": 1
    });
    for parent in ["target", "attribution", "evidence"] {
        let mut invalid = plan.clone();
        let object = if parent == "evidence" {
            invalid[parent][0].as_object_mut().unwrap()
        } else {
            invalid[parent].as_object_mut().unwrap()
        };
        object.insert("unexpected".into(), json!(true));
        invalid_requests.push(("adaptation.plan", invalid));
    }

    for (index, (method, params)) in invalid_requests.into_iter().enumerate() {
        let response: Value = serde_json::from_str(
            &server.handle_line(
                &json!({
                    "jsonrpc": "2.0",
                    "id": index,
                    "method": method,
                    "params": params,
                })
                .to_string(),
            ),
        )
        .unwrap();
        assert_eq!(response["error"]["code"], -32602, "{method}: {response}");
    }
}

#[test]
fn contradiction_read_endpoints_return_empty_and_null_without_records() {
    let mut server = test_server();

    let listed = call(&mut server, 1, "contradiction.list", json!({}));
    let missing = call(
        &mut server,
        2,
        "contradiction.get",
        json!({
            "contradictionId": 404
        }),
    );

    assert_eq!(listed, json!([]));
    assert_eq!(missing, Value::Null);
}

#[test]
fn contradiction_reads_use_exact_camel_case_fields() {
    let engine = Engine::in_memory_with_admin(ADMIN_TOKEN).unwrap();
    let concept = Concept::new("pancake rise", MutabilityClass::DefeasibleGeneral);
    engine.admin_insert_concept(&concept).unwrap();
    let procedure = Procedure::new("OBSERVE", vec![], Expr::Literal(SpoonValue::Bool(true)))
        .with_concept(concept.id);
    engine.admin_insert_procedure(&procedure).unwrap();
    let left_episode = engine
        .execute_procedure(procedure.id, BTreeMap::new(), Some(SpoonValue::Bool(true)))
        .unwrap()
        .episode
        .id;
    let false_procedure = Procedure::new(
        "OBSERVE_FALSE",
        vec![],
        Expr::Literal(SpoonValue::Bool(false)),
    )
    .with_concept(concept.id);
    engine.admin_insert_procedure(&false_procedure).unwrap();
    let right_episode = engine
        .execute_procedure(
            false_procedure.id,
            BTreeMap::new(),
            Some(SpoonValue::Bool(false)),
        )
        .unwrap()
        .episode
        .id;
    let contradiction = engine
        .list_held_contradictions()
        .unwrap()
        .into_iter()
        .next()
        .expect("conflicting observed facts must create a contradiction");
    let mut server = RpcServer::from_engine(engine)
        .with_admin_token(ADMIN_TOKEN)
        .unwrap();

    let listed = call(&mut server, 1, "contradiction.list", json!({}));
    let fetched = call(
        &mut server,
        2,
        "contradiction.get",
        json!({ "contradictionId": contradiction.id.0 }),
    );

    assert_eq!(listed, json!([fetched.clone()]));
    assert_eq!(
        fetched["left"]["supportingEpisodes"][0],
        left_episode.0.to_string()
    );
    assert_eq!(
        fetched["right"]["supportingEpisodes"][0],
        right_episode.0.to_string()
    );
    assert!(fetched["createdAt"].is_number());
    assert!(fetched.get("created_at").is_none());
    assert!(fetched["left"].get("supporting_episodes").is_none());
}

#[test]
fn contradiction_record_refine_and_uncertainty_use_verified_episode_evidence() {
    let engine = Engine::in_memory_with_admin(ADMIN_TOKEN).unwrap();
    let concept = Concept::new("pancake rise", MutabilityClass::DefeasibleGeneral);
    engine.admin_insert_concept(&concept).unwrap();
    let left_procedure = Procedure::new(
        "OBSERVE_CONVECTION",
        vec![Param::named("ovenType")],
        Expr::Literal(SpoonValue::Bool(true)),
    )
    .with_concept(concept.id);
    let right_procedure = Procedure::new(
        "OBSERVE_CONVENTIONAL",
        vec![Param::named("ovenType")],
        Expr::Literal(SpoonValue::Bool(false)),
    )
    .with_concept(concept.id);
    engine.admin_insert_procedure(&left_procedure).unwrap();
    engine.admin_insert_procedure(&right_procedure).unwrap();
    let left_episode = engine
        .execute_procedure(
            left_procedure.id,
            BTreeMap::from([("ovenType".into(), SpoonValue::Text("convection".into()))]),
            Some(SpoonValue::Bool(true)),
        )
        .unwrap()
        .episode;
    let right_episode = engine
        .execute_procedure(
            right_procedure.id,
            BTreeMap::from([("ovenType".into(), SpoonValue::Text("conventional".into()))]),
            Some(SpoonValue::Bool(false)),
        )
        .unwrap()
        .episode;
    let predicate = format!("concept:{}", concept.id);
    let mut server = RpcServer::from_engine(engine)
        .with_admin_token(ADMIN_TOKEN)
        .unwrap();

    let recorded = call(
        &mut server,
        1,
        "contradiction.record",
        json!({
            "left": {
                "id": "pancakes-rise",
                "statement": "pancakes rise",
                "implication": { "predicate": predicate, "value": true },
                "supportingEpisodes": [left_episode.id],
                "scope": []
            },
            "right": {
                "id": "pancakes-flat",
                "statement": "pancakes stay flat",
                "implication": { "predicate": predicate, "value": false },
                "supportingEpisodes": [right_episode.id],
                "scope": []
            },
            "createdAt": 700
        }),
    );
    let held = call(
        &mut server,
        2,
        "contradiction.uncertainty",
        json!({ "claimId": "pancakes-rise" }),
    );
    let refined = call(
        &mut server,
        3,
        "contradiction.refine",
        json!({
            "contradictionId": recorded["id"],
            "discriminator": {
                "feature": "ovenType",
                "leftValue": "convection",
                "leftEpisode": left_episode.id,
                "rightValue": "conventional",
                "rightEpisode": right_episode.id
            },
            "updatedAt": 701
        }),
    );
    let certain = call(
        &mut server,
        4,
        "contradiction.uncertainty",
        json!({ "claimId": "pancakes-rise" }),
    );

    assert_eq!(held["status"], "held_contradictions");
    assert_eq!(held["contradictionIds"], json!([recorded["id"].clone()]));
    assert_eq!(refined["discriminator"]["feature"], "ovenType");
    assert_eq!(certain["status"], "certain");
}

#[test]
fn adaptation_failures_return_structured_application_errors() {
    let mut server = test_server();
    let response: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "adaptation.apply",
                "params": {
                    "planId": "00000000-0000-0000-0000-000000000001",
                    "idempotencyKey": "missing-plan-apply",
                    "appliedAt": 1
                }
            })
            .to_string(),
        ),
    )
    .unwrap();

    assert_eq!(response["error"]["code"], -32024);
    assert_eq!(response["error"]["message"], "adaptation not found");
    assert_eq!(response["error"]["data"]["kind"], "adaptation_not_found");
    assert!(response["error"]["data"]["identifier"].is_string());
}

#[test]
fn adaptation_plan_get_and_apply_are_camel_case_persisted_and_idempotent() {
    let mut server = test_server();
    let procedure = call(
        &mut server,
        1,
        "procedure.create",
        json!({
            "name": "USE_POSITIVE_FACTOR",
            "params": [{ "name": "factor", "description": null }],
            "body": { "Var": "factor" },
            "contract": {
                "requires": [{
                    "description": "factor is positive",
                    "check": {
                        "BinOp": {
                            "op": "Gt",
                            "left": { "Var": "factor" },
                            "right": { "Literal": 0 }
                        }
                    }
                }],
                "promises": [],
                "fails_when": [],
                "costs": { "operations": 1, "description": "identity" },
                "confidence": {
                    "support_count": 0,
                    "contradiction_count": 0,
                    "scope": [],
                    "sources": [],
                    "last_tested": null
                }
            }
        }),
    );
    let admitted = call(
        &mut server,
        2,
        "procedure.execute",
        json!({
            "procedureId": procedure["id"],
            "inputs": { "factor": 1 },
            "prediction": 1
        }),
    );
    let failed: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "procedure.execute",
                "params": {
                    "procedureId": procedure["id"],
                    "inputs": { "factor": -1 }
                }
            })
            .to_string(),
        ),
    )
    .unwrap();
    let episode_id = failed["error"]["data"]["episodeId"].clone();
    let plan_params = json!({
        "idempotencyKey": "positive-factor-plan-1",
        "analysis": {
            "episodeId": episode_id,
            "candidates": [],
            "budget": { "topK": 1, "maxReplays": 0, "maxReplaySteps": 0 }
        },
        "attribution": {
            "suspect": {
                "procedure": procedure["id"],
                "version": 1,
                "traceStep": 0
            },
            "mechanism": "contract_violation"
        },
        "evidence": [
            { "episodeId": episode_id },
            { "episodeId": admitted["episode"]["id"] }
        ],
        "target": {
            "kind": "procedure_scope",
            "procedureId": procedure["id"],
            "expectedVersion": 1,
            "condition": {
                "description": "factor must be non-negative",
                "check": {
                    "BinOp": {
                        "op": "Ge",
                        "left": { "Var": "factor" },
                        "right": { "Literal": 0 }
                    }
                }
            },
            "learnedFrom": episode_id
        },
        "createdAt": 500
    });

    let plan = call(&mut server, 3, "adaptation.plan", plan_params.clone());
    let retried_plan = call(&mut server, 4, "adaptation.plan", plan_params);
    let before = call(
        &mut server,
        5,
        "adaptation.get",
        json!({ "planId": plan["id"] }),
    );
    let apply_params = json!({
        "planId": plan["id"],
        "idempotencyKey": "positive-factor-apply-1",
        "appliedAt": 501
    });
    let receipt = call(&mut server, 6, "adaptation.apply", apply_params.clone());
    let retried_receipt = call(&mut server, 7, "adaptation.apply", apply_params);
    let after = call(
        &mut server,
        8,
        "adaptation.get",
        json!({ "planId": plan["id"] }),
    );
    let updated = call(
        &mut server,
        9,
        "procedure.get",
        json!({ "procedureId": procedure["id"] }),
    );

    assert_eq!(plan, retried_plan);
    assert_eq!(plan["mutationScope"], "online_narrow");
    assert_eq!(plan["action"]["kind"], "narrow_scope");
    assert!(plan["evidence"][0]["selectedFeedbackId"].is_null());
    assert!(plan.get("idempotency_key").is_none());
    assert!(plan["evidence"][0].get("selected_feedback_id").is_none());
    assert!(before["receipt"].is_null());
    assert_eq!(receipt, retried_receipt);
    assert_eq!(receipt["outcome"]["kind"], "procedure_updated");
    assert_eq!(after["receipt"], receipt);
    assert_eq!(updated["version"], 2);
}

#[test]
fn offline_adaptation_is_admin_only_and_consumes_capability_internally() {
    let engine = Engine::in_memory_with_admin(ADMIN_TOKEN).unwrap();
    let concept = Concept::new("leavening rule", MutabilityClass::DefeasibleGeneral);
    engine.admin_insert_concept(&concept).unwrap();
    let mut procedure = Procedure::new(
        "CHECK_FACTOR",
        vec![Param::named("factor")],
        Expr::Var("factor".into()),
    )
    .with_concept(concept.id);
    procedure
        .contract
        .requires
        .push(
            Condition::described("factor is positive").with_check(Expr::BinOp {
                op: BinOp::Gt,
                left: Box::new(Expr::Var("factor".into())),
                right: Box::new(Expr::Literal(SpoonValue::Int(0))),
            }),
        );
    engine.admin_insert_procedure(&procedure).unwrap();
    // Broad revisions now require an independently recorded, replayable
    // baseline for every affected executable behavior. The failing contract
    // episodes below are evidence for the revision, not a passing regression
    // case, so establish the ordinary positive-factor behavior first.
    engine
        .execute_procedure(
            procedure.id,
            BTreeMap::from([("factor".into(), SpoonValue::Int(1))]),
            Some(SpoonValue::Int(1)),
        )
        .unwrap();
    let mut failures = Vec::new();
    let mut feedback_ids = Vec::new();
    for index in 0..5 {
        let failure = engine.execute_procedure(
            procedure.id,
            BTreeMap::from([("factor".into(), SpoonValue::Int(-1))]),
            None,
        );
        let Err(EngineError::ExecutionFailed { episode_id, .. }) = failure else {
            panic!("negative factor must fail its hard contract")
        };
        failures.push(episode_id);
        let verifier = if index % 2 == 0 { "lab-a" } else { "lab-b" };
        let feedback = engine
            .record_authenticated_verifier_feedback(
                &EpisodeFeedback::new(
                    episode_id,
                    SpoonValue::Text("independently observed failure".into()),
                    Evaluation {
                        tier: VerifiabilityTier::Hard,
                        success: false,
                        details: "trusted maintenance evidence".into(),
                        surprise: Some(1.0),
                    },
                    FeedbackSource::new("trusted_lab", Some(verifier.into())),
                    format!("trusted-offline-evidence-{index}"),
                ),
                verifier,
            )
            .unwrap();
        feedback_ids.push(feedback.id);
    }
    let mut server = RpcServer::from_engine(engine)
        .with_admin_token(ADMIN_TOKEN)
        .unwrap();
    let evidence = failures
        .iter()
        .zip(&feedback_ids)
        .map(|(episode_id, feedback_id)| {
            json!({ "episodeId": episode_id, "selectedFeedbackId": feedback_id })
        })
        .collect::<Vec<_>>();
    let plan = call(
        &mut server,
        1,
        "adaptation.plan",
        json!({
            "idempotencyKey": "offline-concept-plan-1",
            "analysis": {
                "episodeId": failures[0],
                "selectedFeedbackId": feedback_ids[0],
                "candidates": [],
                "budget": { "topK": 1, "maxReplays": 0, "maxReplaySteps": 0 }
            },
            "attribution": {
                "suspect": {
                    "procedure": procedure.id,
                    "version": 1,
                    "traceStep": 0
                },
                "mechanism": "contract_violation"
            },
            "evidence": evidence,
            "target": {
                "kind": "concept_revision",
                "conceptId": concept.id,
                "expectedVersion": 1,
                "revisedDescription": "Leavening requires a positive scaling factor"
            },
            "createdAt": 800
        }),
    );
    assert_eq!(plan["mutationScope"], "offline_broad", "{plan}");
    let online: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "adaptation.apply",
                "params": {
                    "planId": plan["id"],
                    "idempotencyKey": "online-broad-rejected",
                    "appliedAt": 801
                }
            })
            .to_string(),
        ),
    )
    .unwrap();
    let receipt = call(
        &mut server,
        3,
        "adaptation.applyOffline",
        json!({
            "planId": plan["id"],
            "idempotencyKey": "offline-broad-applied",
            "appliedAt": 802
        }),
    );
    let revised = call(
        &mut server,
        4,
        "concept.get",
        json!({ "conceptId": concept.id }),
    );

    assert_eq!(online["error"]["code"], -32022);
    assert_eq!(
        online["error"]["data"]["kind"],
        "offline_adaptation_required"
    );
    assert_eq!(receipt["outcome"]["kind"], "concept_updated");
    assert_eq!(
        revised["description"],
        "Leavening requires a positive scaling factor"
    );
}

#[test]
fn raw_graph_mutation_is_disabled_without_explicit_admin_authorization() {
    let mut server = RpcServer::in_memory().unwrap();
    let response: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "concept.create",
                "params": { "name": "UNAUTHORIZED" }
            })
            .to_string(),
        ),
    )
    .unwrap();

    assert_eq!(response["error"]["code"], -32001);
    assert_eq!(
        response["error"]["data"]["kind"],
        "admin_authorization_required"
    );

    let mut authorized_server = RpcServer::in_memory()
        .unwrap()
        .with_admin_token(ADMIN_TOKEN)
        .unwrap();
    let wrong: Value = serde_json::from_str(
        &authorized_server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "concept.create",
                "params": { "name": "WRONG", "adminToken": "not-the-token" }
            })
            .to_string(),
        ),
    )
    .unwrap();
    let authorized: Value = serde_json::from_str(
        &authorized_server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "concept.create",
                "params": { "name": "AUTHORIZED", "adminToken": ADMIN_TOKEN }
            })
            .to_string(),
        ),
    )
    .unwrap();

    assert_eq!(wrong["error"]["code"], -32001);
    assert_eq!(authorized["result"]["name"], "AUTHORIZED");
    assert!(authorized.get("error").is_none());
}

#[test]
fn feedback_accepts_only_raw_observation_and_derives_deferred_trust() {
    let mut server = test_server();
    let procedure = call(
        &mut server,
        1,
        "procedure.create",
        json!({
            "name": "OBSERVE_FEEDBACK",
            "params": [],
            "body": { "Literal": "expected" }
        }),
    );
    let executed = call(
        &mut server,
        2,
        "procedure.execute",
        json!({ "procedureId": procedure["id"], "prediction": "expected" }),
    );
    let raw = call(
        &mut server,
        3,
        "feedback.record",
        json!({
            "episodeId": executed["episode"]["id"],
            "observedResult": "different",
            "idempotencyKey": "raw-feedback-1"
        }),
    );
    let forged: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "feedback.record",
                "params": {
                    "episodeId": executed["episode"]["id"],
                    "observedResult": "different",
                    "idempotencyKey": "raw-feedback-2",
                    "evaluation": {
                        "tier": "Hard",
                        "success": false,
                        "details": "caller assertion"
                    },
                    "source": { "kind": "independent_lab", "actor": "forged" }
                }
            })
            .to_string(),
        ),
    )
    .unwrap();

    assert_eq!(raw["evaluation"]["tier"], "Deferred");
    assert_eq!(raw["evaluation"]["success"], false);
    assert_eq!(raw["source"]["kind"], "rpc_observation");
    assert_eq!(forged["error"]["code"], -32602);
}

#[test]
fn concepts_can_be_created_and_listed() {
    let mut server = test_server();

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
    let mut server = test_server();
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
fn procedure_execution_failure_returns_a_structured_episode_id() {
    let mut server = test_server();
    let procedure = call(
        &mut server,
        1,
        "procedure.create",
        json!({
            "name": "DIVIDE_BY_ZERO",
            "params": [{ "name": "x", "description": null }],
            "body": {
                "BinOp": {
                    "op": "Div",
                    "left": { "Var": "x" },
                    "right": { "Literal": 0 }
                }
            }
        }),
    );

    let response: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "procedure.execute",
                "params": { "procedureId": procedure["id"], "inputs": { "x": 7 } }
            })
            .to_string(),
        ),
    )
    .unwrap();

    assert_eq!(response["error"]["message"], "execution failed");
    assert!(response["error"]["data"]["episodeId"].is_string());
    assert!(response["error"]["data"]["cause"].is_string());
}

#[test]
fn late_feedback_is_append_only_idempotent_and_uses_camel_case_wire_fields() {
    let mut server = test_server();
    let procedure = call(
        &mut server,
        1,
        "procedure.create",
        json!({
            "name": "PANCAKE_RESULT",
            "params": [],
            "body": { "Literal": "served" }
        }),
    );
    let executed = call(
        &mut server,
        2,
        "procedure.execute",
        json!({ "procedureId": procedure["id"] }),
    );
    let params = json!({
        "episodeId": executed["episode"]["id"],
        "observedResult": "flat pancakes",
        "idempotencyKey": "flat-feedback-1"
    });

    let first = call(&mut server, 3, "feedback.record", params.clone());
    let retry = call(&mut server, 4, "feedback.record", params);

    assert_eq!(first, retry);
    assert_eq!(first["episodeId"], executed["episode"]["id"]);
    assert_eq!(first["observedResult"], "flat pancakes");
    assert_eq!(first["evaluation"]["tier"], "Deferred");
    assert_eq!(first["source"]["kind"], "rpc_observation");
    assert_eq!(first["idempotencyKey"], "flat-feedback-1");
    assert!(first.get("episode_id").is_none());
}

#[test]
fn credit_analysis_uses_exact_camel_case_wire_fields_and_reports_cost() {
    let mut server = test_server();
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
            "prediction": 15
        }),
    );
    let analysis_params = json!({
        "idempotencyKey": "double-analysis-1",
        "episodeId": executed["episode"]["id"],
        "candidates": [{
            "suspect": {
                "procedure": procedure["id"],
                "version": 1,
                "traceStep": 0
            },
            "priorScore": 0.8,
            "change": {
                "description": "replace multiplier",
                "replacement": {
                    "kind": "replace_body",
                    "target": { "id": procedure["id"], "version": 1 },
                    "body": {
                        "BinOp": {
                            "op": "Mul",
                            "left": { "Var": "x" },
                            "right": { "Literal": 3 }
                        }
                    },
                    "verification": {
                        "kind": "deterministic_expected",
                        "expected": 21
                    }
                }
            },
            "mode": "deterministic"
        }],
        "budget": { "topK": 1, "maxReplays": 1, "maxReplaySteps": 100 }
    });
    let analyzed = call(&mut server, 3, "credit.analyze", analysis_params.clone());
    let retried = call(&mut server, 4, "credit.analyze", analysis_params.clone());
    let fetched = call(
        &mut server,
        5,
        "credit.get",
        json!({ "analysisId": analyzed["analysisId"] }),
    );
    let fetched_by_key = call(
        &mut server,
        6,
        "credit.getByKey",
        json!({ "idempotencyKey": "double-analysis-1" }),
    );
    let mut conflicting_params = analysis_params;
    conflicting_params["budget"]["maxReplaySteps"] = json!(99);
    let conflict: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "credit.analyze",
                "params": conflicting_params
            })
            .to_string(),
        ),
    )
    .unwrap();

    assert_eq!(analyzed, retried);
    assert_eq!(analyzed, fetched);
    assert_eq!(analyzed, fetched_by_key);
    assert_eq!(conflict["error"]["code"], -32015);
    assert_eq!(
        conflict["error"]["data"]["kind"],
        "credit_idempotency_conflict"
    );
    assert_eq!(analyzed["episodeId"], executed["episode"]["id"]);
    assert!(analyzed["cost"]["attributionCostRatio"].as_f64().unwrap() > 0.0);
    assert_eq!(analyzed["ranked"][0]["suspect"]["traceStep"], 0);
    assert!(analyzed.get("episode_id").is_none());

    let rejected: Value = serde_json::from_str(
        &server.handle_line(
            &json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "credit.analyze",
                "params": {
                    "episode_id": executed["episode"]["id"],
                    "candidates": [],
                    "budget": { "topK": 0, "maxReplays": 0, "maxReplaySteps": 0 }
                }
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(rejected["error"]["code"], -32602);
}

#[test]
fn credit_get_survives_file_backed_server_reopen() {
    let database = std::env::temp_dir().join(format!(
        "spoon-server-credit-persistence-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let database_path = database.to_string_lossy().into_owned();
    let analysis = {
        let mut server = RpcServer::open(&database_path)
            .unwrap()
            .with_admin_token(ADMIN_TOKEN)
            .unwrap();
        let procedure = call(
            &mut server,
            1,
            "procedure.create",
            json!({
                "name": "PERSISTED_ANALYSIS",
                "params": [],
                "body": { "Literal": 2 }
            }),
        );
        let executed = call(
            &mut server,
            2,
            "procedure.execute",
            json!({ "procedureId": procedure["id"], "prediction": 3 }),
        );
        call(
            &mut server,
            3,
            "credit.analyze",
            json!({
                "idempotencyKey": "persisted-analysis-1",
                "episodeId": executed["episode"]["id"],
                "candidates": [],
                "budget": { "topK": 0, "maxReplays": 0, "maxReplaySteps": 0 }
            }),
        )
    };
    let mut reopened = RpcServer::open(&database_path).unwrap();

    let by_id = call(
        &mut reopened,
        4,
        "credit.get",
        json!({ "analysisId": analysis["analysisId"] }),
    );
    let by_key = call(
        &mut reopened,
        5,
        "credit.getByKey",
        json!({ "idempotencyKey": "persisted-analysis-1" }),
    );

    assert_eq!(by_id, analysis);
    assert_eq!(by_key, analysis);
    drop(reopened);
    let _ = std::fs::remove_file(database);
}

#[test]
fn cycle_begin_and_resume_use_camel_case_wire_fields() {
    let mut server = test_server();
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
    let mut server = test_server();
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
    let mut server = test_server();
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

#[test]
fn language_render_is_public_bounded_and_never_elevates_caller_provenance() {
    let mut server = test_server();
    let plan = json!({
        "dialogueMove": {"act": "Inform", "relatesToTurn": null},
        "claims": [
            {"Grounded": {
                "id": "letter-count",
                "text": "There are 3 r characters in strawberry.",
                "evidence": [{
                    "id": "episode:checked-letter-count",
                    "sourceKind": "SelfVerified",
                    "linkedEpisode": null
                }],
                "provenance": ["procedure:private-letter-count"]
            }},
            {"Unsupported": {
                "id": "guess",
                "reason": "No observation supports this second claim."
            }}
        ],
        "uncertainty": {"level": "Certain", "disclosure": null},
        "tone": "Neutral",
        "variant": "Plain"
    });

    let rendered = call(
        &mut server,
        1,
        "language.render",
        json!({"plan": plan, "options": {"tone": "Warm", "variant": "Bulleted"}}),
    );
    assert_eq!(
        rendered["text"],
        "- There are 3 r characters in strawberry."
    );
    assert_eq!(rendered["includedClaimIds"], json!(["letter-count"]));
    assert_eq!(rendered["omittedClaimIds"], json!(["guess"]));
    assert_eq!(rendered["tone"], "Warm");
    assert_eq!(rendered["dialogueMove"]["act"], "Inform");
    assert_eq!(
        rendered["audit"],
        json!({
            "renderer": "bounded_response_plan_v1",
            "claimsSubmitted": 2,
            "evidenceStatus": "caller_supplied_unverified",
            "provenanceRedacted": true,
            "redacted": true,
        })
    );
    let public = rendered.to_string();
    assert!(!public.contains("private-letter-count"));
    assert!(!public.contains("SelfVerified"));

    let ungrounded = raw_call(
        &mut server,
        2,
        "language.render",
        json!({
            "plan": {
                "dialogueMove": {"act": "Inform", "relatesToTurn": null},
                "claims": [{"Grounded": {
                    "id": "unsupported-as-fact",
                    "text": "This must not be rendered.",
                    "evidence": [],
                    "provenance": []
                }}],
                "uncertainty": {"level": "Certain", "disclosure": null},
                "tone": "Neutral",
                "variant": "Plain"
            }
        }),
    );
    assert_eq!(ungrounded["error"]["code"], -32602);
    assert_eq!(
        ungrounded["error"]["data"]["kind"],
        "invalid_language_response_plan"
    );
    assert_eq!(ungrounded["error"]["data"]["redacted"], true);

    let ambient_trust = raw_call(
        &mut server,
        3,
        "language.render",
        json!({
            "plan": {
                "dialogueMove": {"act": "Inform", "relatesToTurn": null},
                "claims": [{"Grounded": {
                    "id": "ambient-trust",
                    "text": "No ambient trust field is accepted.",
                    "evidence": [{
                        "id": "unverified",
                        "sourceKind": "Observed",
                        "linkedEpisode": null,
                        "trust": "administrator"
                    }],
                    "provenance": []
                }}],
                "uncertainty": {"level": "Certain", "disclosure": null},
                "tone": "Neutral",
                "variant": "Plain"
            }
        }),
    );
    assert_eq!(ambient_trust["error"]["code"], -32602);

    let oversized = raw_call(
        &mut server,
        4,
        "language.render",
        json!({
            "plan": {
                "dialogueMove": {"act": "Abstain", "relatesToTurn": null},
                "claims": [{"Unsupported": {
                    "id": "large-metadata",
                    "reason": "x".repeat(130 * 1024)
                }}],
                "uncertainty": {"level": "Unknown", "disclosure": null},
                "tone": "Neutral",
                "variant": "Plain"
            }
        }),
    );
    assert_eq!(oversized["error"]["code"], -32602);
    assert_eq!(
        oversized["error"]["data"]["kind"],
        "language_render_input_too_large"
    );
}
