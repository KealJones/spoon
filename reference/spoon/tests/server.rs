use std::sync::Arc;

use spoon::cli::{Cli, Command};
use spoon::server::router;
use spoon::stdio::handle_line;
use spoon_mind::brain::{Brain, BrainConfig};

fn test_brain_cfg() -> BrainConfig {
    BrainConfig { db_path: None, offline: true, debug: true, ..Default::default() }
}

async fn spawn_server() -> (Arc<Brain>, std::net::SocketAddr) {
    let brain = Brain::open(test_brain_cfg()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(Arc::clone(&brain));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    // Give the server a moment to start accepting connections.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    (brain, addr)
}

// ---- 1. stdio handle_line --------------------------------------------------

#[tokio::test]
async fn stdio_handle_line_roundtrip() {
    let brain = Brain::open(test_brain_cfg()).await.unwrap();

    // Valid input
    let out = handle_line(&*brain, r#"{"session":"t1","text":"hello"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&out).expect("output must be JSON");
    assert!(!v["text"].as_str().unwrap_or("").is_empty(), "text must be non-empty");
    assert_eq!(v["metrics"]["interior_llm_calls"], 0, "interior_llm_calls must be 0");
    assert!(v["episode_id"].as_i64().unwrap_or(0) > 0, "episode_id must be > 0");

    // Malformed input
    let err_out = handle_line(&*brain, "not json at all").await;
    let ev: serde_json::Value = serde_json::from_str(&err_out).expect("error output must be JSON");
    assert!(ev.get("error").is_some(), "malformed line must return error key");
}

// ---- 2. non-stream chat completion -----------------------------------------

#[tokio::test]
async fn server_chat_completion_non_stream() {
    let (_brain, addr) = spawn_server().await;
    let client = reqwest::Client::new();

    let res = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body(r#"{"messages":[{"role":"user","content":"hello"}]}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["role"], "assistant");
    assert!(!body["choices"][0]["message"]["content"].as_str().unwrap_or("").is_empty());
    assert_eq!(body["spoon"]["metrics"]["interior_llm_calls"], 0);
}

// ---- 3. streaming chat completion ------------------------------------------

#[tokio::test]
async fn server_chat_completion_stream() {
    let (_brain, addr) = spawn_server().await;
    let client = reqwest::Client::new();

    let res = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body(r#"{"messages":[{"role":"user","content":"hello"}],"stream":true}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body_text = res.text().await.unwrap();
    assert!(body_text.contains("data: "), "stream must have data: lines");
    assert!(
        body_text.contains(r#""finish_reason":"stop""#),
        "stream must have a finish_reason:stop chunk"
    );
    assert!(body_text.contains("data: [DONE]"), "stream must end with [DONE]");
}

// ---- 4. models and debug endpoints -----------------------------------------

#[tokio::test]
async fn server_models_and_debug() {
    let (_brain, addr) = spawn_server().await;
    let client = reqwest::Client::new();

    // /v1/models
    let models: serde_json::Value = client
        .get(format!("http://{addr}/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"][0]["id"], "spoon");

    // /debug/metrics
    let metrics: serde_json::Value = client
        .get(format!("http://{addr}/debug/metrics"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(metrics["interior_llm_calls"], 0);

    // GET / returns HTML with "Spoon"
    let html = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Spoon"), "inspector page must mention Spoon");
}

// ---- 5. bad JSON returns 400 -----------------------------------------------

#[tokio::test]
async fn server_bad_json_400() {
    let (_brain, addr) = spawn_server().await;
    let client = reqwest::Client::new();

    let res = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body("this is not json")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body.get("error").is_some(), "must have error object");
}

// ---- 6. debug tables -------------------------------------------------------

async fn chat(client: &reqwest::Client, addr: &std::net::SocketAddr, session: &str, text: &str) -> String {
    let res = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("X-Spoon-Session", session)
        .json(&serde_json::json!({"messages": [{"role": "user", "content": text}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "turn failed: {text}");
    let body: serde_json::Value = res.json().await.unwrap();
    body["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string()
}

async fn get_rows(client: &reqwest::Client, addr: &std::net::SocketAddr, path: &str) -> Vec<serde_json::Value> {
    let res = client.get(format!("http://{addr}{path}")).send().await.unwrap();
    assert_eq!(res.status(), 200, "GET {path}");
    let body: serde_json::Value = res.json().await.unwrap();
    body.as_array().unwrap_or_else(|| panic!("{path} must return a JSON array, got {body}")).clone()
}

#[tokio::test]
async fn server_debug_tables() {
    let (_brain, addr) = spawn_server().await;
    let client = reqwest::Client::new();

    chat(&client, &addr, "dbg", "John owns a dog.").await;
    chat(&client, &addr, "dbg", "Assistant, double 21!").await;
    let reply = chat(&client, &addr, "dbg", "The double of 3 is 6. The double of 5 is 10.").await;
    assert!(reply.contains("42"), "double must be learned from the examples and run: {reply}");

    // facts: newest first, the own fact is there, ?q= narrows and can empty the list
    let facts = get_rows(&client, &addr, "/debug/facts").await;
    assert!(facts.iter().any(|f| f["relation"] == "own"), "no own fact in {facts:?}");
    let own = facts.iter().find(|f| f["relation"] == "own").unwrap();
    assert_eq!(own["text"].as_str().unwrap(), "own(John, dog_1)");
    assert_eq!(own["superseded"], false);
    assert_eq!(own["source"], "user");
    let ids: Vec<i64> = facts.iter().map(|f| f["id"].as_i64().unwrap()).collect();
    assert!(ids.windows(2).all(|w| w[0] > w[1]), "facts must be newest first: {ids:?}");
    let owns = get_rows(&client, &addr, "/debug/facts?q=own").await;
    assert!(!owns.is_empty() && owns.len() < facts.len());
    assert!(owns.iter().all(|f| serde_json::to_string(f).unwrap().to_lowercase().contains("own")));
    assert!(get_rows(&client, &addr, "/debug/facts?q=zzqx_nothing").await.is_empty());

    // actions: the learned double has a program body and a store timestamp
    let doubles = get_rows(&client, &addr, "/debug/actions?q=double").await;
    let learned = doubles
        .iter()
        .find(|a| a["tier"] != "Kernel" && a["verbs"].as_array().is_some_and(|v| v.iter().any(|x| x == "double")))
        .unwrap_or_else(|| panic!("no learned double action in {doubles:?}"));
    assert!(!learned["program"].as_str().unwrap_or("").is_empty(), "program must be pretty-printed");
    assert!(learned["ir"].is_object(), "raw IR must ride along");
    assert!(learned["updated_at"].is_i64(), "learned actions are persisted");
    let all_actions = get_rows(&client, &addr, "/debug/actions?limit=1000").await;
    assert!(all_actions.iter().any(|a| a["tier"] == "Kernel"), "kernel actions are listed too");
    assert_ne!(all_actions[0]["tier"], "Kernel", "learned actions sort first");

    // concepts: Dog was minted for the noun, with dog_1 as an example
    let dogs = get_rows(&client, &addr, "/debug/concepts?q=dog").await;
    let dog = dogs.iter().find(|c| c["id"] == "Dog").unwrap_or_else(|| panic!("no Dog concept in {dogs:?}"));
    assert_eq!(dog["kind"], "entity");
    assert!(dog["examples"].as_array().is_some_and(|e| e.iter().any(|x| x == "dog_1")));

    // episodes: 3 rows newest first, paging and filtering work
    let episodes = get_rows(&client, &addr, "/debug/episodes").await;
    assert_eq!(episodes.len(), 3);
    assert_eq!(episodes[0]["user_text"], "The double of 3 is 6. The double of 5 is 10.");
    assert_eq!(episodes[2]["user_text"], "John owns a dog.");
    assert_eq!(episodes[0]["session_id"], "dbg");
    assert_eq!(episodes[0]["metrics"]["interior_llm_calls"], 0);
    assert!(episodes[0]["ears_path"].is_string());
    assert_eq!(episodes[0]["mouth_path"], "template");
    let page = get_rows(&client, &addr, "/debug/episodes?limit=1&offset=1").await;
    assert_eq!(page.len(), 1);
    assert_eq!(page[0]["id"], episodes[1]["id"]);
    assert_eq!(get_rows(&client, &addr, "/debug/episodes?q=owns").await.len(), 1);

    // pairs, stances, kv all answer
    let pairs = get_rows(&client, &addr, "/debug/pairs?limit=1000").await;
    get_rows(&client, &addr, "/debug/stances").await;
    let kv = get_rows(&client, &addr, "/debug/kv").await;
    assert!(kv.iter().any(|r| r["key"] == "schema_version"));

    // counts agree with the tables
    let counts: serde_json::Value =
        client.get(format!("http://{addr}/debug/counts")).send().await.unwrap().json().await.unwrap();
    assert_eq!(counts["episodes"], 3);
    let all_facts = get_rows(&client, &addr, "/debug/facts?limit=1000").await;
    assert_eq!(counts["facts"].as_u64().unwrap() as usize, all_facts.len());
    assert_eq!(counts["pairs"].as_u64().unwrap() as usize, pairs.len());
    assert_eq!(counts["kv"].as_u64().unwrap() as usize, kv.len());
    let persisted = all_actions.iter().filter(|a| !a["updated_at"].is_null()).count();
    assert_eq!(counts["actions"].as_u64().unwrap() as usize, persisted, "every stored action is in the CAN");

    // the inspector page
    let html = client.get(format!("http://{addr}/")).send().await.unwrap().text().await.unwrap();
    assert!(html.contains("Spoon Inspector"));
    assert!(!html.contains('\u{2014}'), "no em-dashes");
}

// ---- 7. CLI parsing --------------------------------------------------------

#[test]
fn cli_parses() {
    use clap::Parser;

    let cli = Cli::try_parse_from(["spoon", "--ephemeral", "--offline", "serve", "--port", "9999"])
        .expect("serve parse");
    assert!(cli.ephemeral);
    assert!(cli.offline);
    assert!(matches!(cli.command, Command::Serve { port: 9999, .. }));

    let cli2 = Cli::try_parse_from(["spoon", "bench", "ace"]).expect("bench parse");
    assert!(matches!(cli2.command, Command::Bench { ref corpus } if corpus == "ace"));

    let cli3 = Cli::try_parse_from(["spoon", "teach", "--lessons", "6", "--themes", "math,cooking", "--max-minutes", "8", "--dry-run"])
        .expect("teach parse");
    match cli3.command {
        Command::Teach { lessons, themes, max_minutes, dry_run } => {
            assert_eq!(lessons, 6);
            assert_eq!(themes, vec!["math".to_string(), "cooking".to_string()]);
            assert_eq!(max_minutes, 8);
            assert!(dry_run);
        }
        other => panic!("expected teach, got {other:?}"),
    }
    let cli4 = Cli::try_parse_from(["spoon", "teach"]).expect("teach defaults");
    assert!(matches!(cli4.command, Command::Teach { lessons: 10, max_minutes: 30, dry_run: false, .. }));
}
