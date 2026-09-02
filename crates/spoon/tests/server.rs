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

// ---- 6. CLI parsing --------------------------------------------------------

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
