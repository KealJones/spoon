//! HTTP: an OpenAI-compatible endpoint and the inspector.

use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
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
    // Every OpenAI client asks for streaming, and one that asks and gets a
    // silent socket followed by everything at once reads that as a hang.
    if body["stream"].as_bool().unwrap_or(false) {
        return stream_chat(brain, body).await.into_response();
    }
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

/// The streaming form of the same endpoint.
///
/// The interior does not stream: a turn is interpreted, reasoned about, and
/// answered as one act, and there is no half-formed answer to send. What
/// streams is the finished reply, chunked, because every OpenAI client expects
/// this shape and refusing to speak it would mean none of them work. The
/// alternative, holding the socket silent and then sending everything at once,
/// is what clients read as a hang.
async fn stream_chat(brain: Shared, body: serde_json::Value) -> impl IntoResponse {
    let session = body["user"].as_str().unwrap_or("http").to_string();
    let text = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_string();

    let reply = {
        let mut guard = brain.lock().await;
        match guard.turn(&session, &text).await {
            Ok(result) => result.reply,
            Err(err) => format!("error: {err}"),
        }
    };

    let id = format!("chatcmpl-{}", chrono::Utc::now().timestamp_millis());
    let mut events: Vec<Result<Event, std::convert::Infallible>> = Vec::new();
    let chunk = |id: &str, delta: serde_json::Value, finish: Option<&str>| {
        Event::default().data(
            serde_json::json!({
                "id": id,
                "object": "chat.completion.chunk",
                "model": "spoon",
                "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }]
            })
            .to_string(),
        )
    };

    events.push(Ok(chunk(
        &id,
        serde_json::json!({ "role": "assistant" }),
        None,
    )));
    // Word-sized pieces: small enough to render progressively, large enough not
    // to drown a client in single-character frames.
    for word in reply.split_inclusive(' ') {
        events.push(Ok(chunk(&id, serde_json::json!({ "content": word }), None)));
    }
    events.push(Ok(chunk(&id, serde_json::json!({}), Some("stop"))));
    events.push(Ok(Event::default().data("[DONE]")));

    Sse::new(futures::stream::iter(events))
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
    let now = chrono::Utc::now();
    let all = guard.store().all_realizations().unwrap_or_default();

    // Grouped by where it came from, because the interesting question is
    // whether what the Teacher wrote holds up against what search found, and
    // that is invisible in a flat list.
    let mut by_source: std::collections::BTreeMap<&str, (usize, u64, u64, f64)> =
        Default::default();
    for r in &all {
        let entry = by_source
            .entry(provenance_label(&r.provenance))
            .or_default();
        entry.0 += 1;
        entry.1 += r.activation.uses;
        entry.2 += r.activation.failures;
        entry.3 += r.activation.success_rate();
    }
    let summary: Vec<serde_json::Value> = by_source
        .into_iter()
        .map(|(source, (count, uses, failures, rate_sum))| {
            json!({
                "source": source,
                "count": count,
                "uses": uses,
                "failures": failures,
                "mean_success_rate": rate_sum / count as f64,
            })
        })
        .collect();
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
                // Where it came from. Two realizations of one concept are
                // routinely a Teacher's guess sitting beside a synthesized
                // body, and which is which is the first thing anyone asks.
                "source": provenance_label(&r.provenance),
                "effect": r.effect.as_str(),
                "tier": format!("{:?}", r.tier),
                "uses": r.activation.uses,
                "successes": r.activation.successes,
                "failures": r.activation.failures,
                // The numbers selection actually runs on, computed the same
                // way here as there. Reading a rank off raw counts is
                // misleading: a realization used twice today outranks one used
                // twenty times last month, and only the activation shows that.
                "success_rate": r.activation.success_rate(),
                "activation": r.activation.base_level(now),
                "score": // Reported as the posterior mean, not a draw: a number in a table
                // that changed on every refresh would be unreadable.
                spoon_eval::score(std::sync::Arc::new(r.clone()), 0.5, now, None).score,
                "last_used": r.activation.last_used_at,
            })
        })
        .collect();
    Json(json!({ "count": rows.len(), "by_source": summary, "realizations": rows }))
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

/// A short, readable name for where something came from.
fn provenance_label(p: &spoon_concept::Provenance) -> &'static str {
    match p {
        spoon_concept::Provenance::Bootstrap => "bootstrap",
        spoon_concept::Provenance::User { .. } => "user",
        spoon_concept::Provenance::Teacher { .. } => "taught",
        spoon_concept::Provenance::Synthesized { .. } => "synthesized",
        spoon_concept::Provenance::Consolidated => "consolidated",
        spoon_concept::Provenance::Inferred => "inferred",
        spoon_concept::Provenance::Imported { .. } => "imported",
        spoon_concept::Provenance::External { .. } => "external",
    }
}

/// The inspector, served from the binary so there is nothing to build or host.
async fn inspector() -> Html<&'static str> {
    Html(include_str!("inspector.html"))
}
