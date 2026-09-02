use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Json, Response,
        sse::{Event, Sse},
    },
};
use futures::stream;
use serde::Deserialize;
use serde_json::json;
use spoon_mind::brain::Brain;

// ---- Request types ---------------------------------------------------------

#[derive(Deserialize)]
pub struct ChatRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub stream: Option<bool>,
    pub user: Option<String>,
}

#[derive(Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: MessageContent,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub type_: String,
    pub text: Option<String>,
}

// ---- Helpers ----------------------------------------------------------------

fn content_to_str(c: &MessageContent) -> String {
    match c {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter(|p| p.type_ == "text")
            .filter_map(|p| p.text.as_deref())
            .collect::<Vec<_>>()
            .join(""),
    }
}

fn content_into_string(c: MessageContent) -> String {
    match c {
        MessageContent::Text(s) => s,
        MessageContent::Parts(parts) => parts
            .into_iter()
            .filter(|p| p.type_ == "text")
            .filter_map(|p| p.text)
            .collect::<Vec<_>>()
            .join(""),
    }
}

fn error_resp(status: StatusCode, msg: &str) -> Response {
    (
        status,
        Json(json!({"error": {"message": msg, "type": "invalid_request_error"}})),
    )
        .into_response()
}

/// Stable session hash from a message string using std DefaultHasher.
fn session_hash(text: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Split reply into 1-3-word chunks for streaming, preserving spaces.
fn split_chunks(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return if text.is_empty() { vec![] } else { vec![text.to_string()] };
    }
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let n = (i % 3 + 1).min(words.len() - i);
        let joined = words[i..i + n].join(" ");
        let chunk = if i + n < words.len() {
            format!("{joined} ")
        } else {
            joined
        };
        chunks.push(chunk);
        i += n;
    }
    chunks
}

// ---- Handlers ---------------------------------------------------------------

pub async fn list_models() -> impl IntoResponse {
    Json(json!({
        "object": "list",
        "data": [{"id": "spoon", "object": "model", "owned_by": "spoon"}]
    }))
}

pub async fn chat_completions(
    State(brain): State<Arc<Brain>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let req: ChatRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_resp(StatusCode::BAD_REQUEST, &e.to_string()),
    };

    let is_stream = req.stream.unwrap_or(false);
    let user_field = req.user.clone();

    // Session ID: X-Spoon-Session header > user field > hash of first user msg
    let session_id = if let Some(h) = headers
        .get("x-spoon-session")
        .and_then(|v| v.to_str().ok())
    {
        h.to_string()
    } else if let Some(u) = user_field {
        u
    } else {
        req.messages
            .iter()
            .find(|m| m.role == "user")
            .map(|m| session_hash(&content_to_str(&m.content)))
            .unwrap_or_else(|| "default".to_string())
    };

    // Extract last user message text
    let text = match req.messages.into_iter().rev().find(|m| m.role == "user") {
        None => return error_resp(StatusCode::BAD_REQUEST, "no user message found"),
        Some(msg) => content_into_string(msg.content),
    };

    let result = match brain.turn(&session_id, &text).await {
        Ok(r) => r,
        Err(e) => return error_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let chat_id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
    let created = chrono::Utc::now().timestamp();

    if is_stream {
        let chunks = split_chunks(&result.text);
        let mut events: Vec<Result<Event, Infallible>> = Vec::new();

        // Role chunk
        events.push(Ok(Event::default().data(
            json!({
                "id": chat_id, "object": "chat.completion.chunk",
                "created": created, "model": "spoon",
                "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]
            })
            .to_string(),
        )));

        // Content chunks
        for chunk in chunks {
            events.push(Ok(Event::default().data(
                json!({
                    "id": chat_id, "object": "chat.completion.chunk",
                    "created": created, "model": "spoon",
                    "choices": [{"index": 0, "delta": {"content": chunk}, "finish_reason": null}]
                })
                .to_string(),
            )));
        }

        // Finish chunk
        events.push(Ok(Event::default().data(
            json!({
                "id": chat_id, "object": "chat.completion.chunk",
                "created": created, "model": "spoon",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            })
            .to_string(),
        )));

        // [DONE]
        events.push(Ok(Event::default().data("[DONE]")));

        Sse::new(stream::iter(events)).into_response()
    } else {
        Json(json!({
            "id": chat_id,
            "object": "chat.completion",
            "created": created,
            "model": "spoon",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": result.text},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
            "spoon": {
                "episode_id": result.episode.id,
                "mouth_path": result.mouth_path,
                "metrics": result.episode.metrics
            }
        }))
        .into_response()
    }
}
