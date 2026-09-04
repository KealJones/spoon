//! HTTP: an OpenAI-compatible endpoint and the inspector.

use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use spoon_brain::Brain;
use tokio::sync::Mutex;

use crate::Cli;
use crate::build::assemble;

type Shared = Arc<Mutex<Brain>>;

pub async fn run(cli: &Cli, host: &str, port: u16) -> Result<()> {
    let brain: Shared = Arc::new(Mutex::new(assemble(cli).await?));
    let app = Router::new()
        .route("/", get(inspector))
        .route("/inspector", get(inspector))
        .route("/health", get(|| async { "ok" }))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat))
        .route("/debug/status", get(status))
        .route("/debug/concepts", get(concepts))
        .route("/debug/realizations", get(realizations))
        .route("/debug/episodes", get(episodes))
        .with_state(brain);

    let addr = format!("{host}:{port}");
    println!("spoon serving on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn models() -> impl IntoResponse {
    Json(json!({
        "object": "list",
        "data": [{ "id": "spoon", "object": "model", "owned_by": "spoon" }]
    }))
}

async fn chat(
    State(brain): State<Shared>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let session = body["user"].as_str().unwrap_or("http").to_string();
    let text = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_string();

    let mut guard = brain.lock().await;
    match guard.turn(&session, &text).await {
        Ok(result) => Json(json!({
            "object": "chat.completion",
            "model": "spoon",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": result.reply },
                "finish_reason": "stop"
            }],
            // Everything the interior did, so a caller can see the reasoning
            // rather than just the sentence.
            "spoon": {
                "episode_id": result.episode.id,
                "ears_path": format!("{:?}", result.episode.ears_path),
                "steps": result.episode.steps.iter().map(|s| guard.render(s)).collect::<Vec<_>>(),
                "gaps": result.episode.gaps.iter().map(|g| guard.render(g)).collect::<Vec<_>>(),
                "rules": result.episode.rules,
                "metrics": result.episode.metrics,
            }
        }))
        .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn status(State(brain): State<Shared>) -> impl IntoResponse {
    let guard = brain.lock().await;
    let store = guard.store();
    Json(json!({
        "schema": store.schema_version().unwrap_or(0),
        "concepts": store.count_concepts().unwrap_or(0),
        "realizations": store.all_realizations().map(|r| r.len()).unwrap_or(0),
        "episodes": store.count_episodes().unwrap_or(0),
        "symbols": store.all_symbols().map(|s| s.len()).unwrap_or(0),
    }))
}

#[derive(serde::Deserialize)]
struct Filter {
    q: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    200
}

async fn concepts(State(brain): State<Shared>, Query(f): Query<Filter>) -> impl IntoResponse {
    let guard = brain.lock().await;
    let all = guard.store().all_concepts().unwrap_or_default();
    let rendered: Vec<String> = all
        .iter()
        .map(|c| guard.render(c))
        .filter(|s| {
            f.q.as_ref()
                .is_none_or(|q| s.to_lowercase().contains(&q.to_lowercase()))
        })
        .take(f.limit)
        .collect();
    Json(json!({ "count": rendered.len(), "concepts": rendered }))
}

async fn realizations(State(brain): State<Shared>, Query(f): Query<Filter>) -> impl IntoResponse {
    let guard = brain.lock().await;
    let all = guard.store().all_realizations().unwrap_or_default();
    let rows: Vec<serde_json::Value> = all
        .iter()
        .filter(|r| {
            f.q.as_ref()
                .is_none_or(|q| r.name.to_lowercase().contains(&q.to_lowercase()))
        })
        .take(f.limit)
        .map(|r| {
            json!({
                "name": r.name,
                "target": guard.render(&r.target),
                "kind": r.spec.kind().as_str(),
                "effect": r.effect.as_str(),
                "tier": format!("{:?}", r.tier),
                "uses": r.activation.uses,
                "successes": r.activation.successes,
                "failures": r.activation.failures,
            })
        })
        .collect();
    Json(json!({ "count": rows.len(), "realizations": rows }))
}

async fn episodes(State(brain): State<Shared>, Query(f): Query<Filter>) -> impl IntoResponse {
    let guard = brain.lock().await;
    let raw = guard
        .store()
        .recent_episodes(f.limit, None)
        .unwrap_or_default();
    let parsed: Vec<serde_json::Value> = raw
        .iter()
        .filter_map(|r| serde_json::from_str(r).ok())
        .collect();
    Json(json!({ "count": parsed.len(), "episodes": parsed }))
}

/// The inspector, served from the binary so there is nothing to build or host.
async fn inspector() -> Html<&'static str> {
    Html(include_str!("inspector.html"))
}
