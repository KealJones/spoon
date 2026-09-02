//! The one LLM client. Ollama exposes an OpenAI-compatible endpoint at
//! `http://localhost:11434/v1`, so one implementation covers local models and
//! frontier providers. Every call is tagged with a `Seat` so metrics can prove
//! the interior never calls it.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Seat {
    Ears,
    Mouth,
    Teacher,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// e.g. "http://localhost:11434/v1"
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub temperature: f32,
    /// Ollama-specific: disable chain-of-thought for codec work.
    #[serde(default = "default_true")]
    pub no_think: bool,
}

fn default_timeout() -> u64 {
    60
}
fn default_true() -> bool {
    true
}

impl LlmConfig {
    pub fn ollama(model: &str) -> LlmConfig {
        LlmConfig {
            base_url: std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".into())
                .trim_end_matches('/')
                .to_string()
                + "/v1",
            api_key: None,
            model: model.to_string(),
            timeout_secs: 60,
            temperature: 0.0,
            no_think: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(s: impl Into<String>) -> ChatMessage {
        ChatMessage { role: "system".into(), content: s.into() }
    }
    pub fn user(s: impl Into<String>) -> ChatMessage {
        ChatMessage { role: "user".into(), content: s.into() }
    }
    pub fn assistant(s: impl Into<String>) -> ChatMessage {
        ChatMessage { role: "assistant".into(), content: s.into() }
    }
}

#[derive(Debug, Default)]
pub struct LlmCounters {
    pub ears: AtomicU32,
    pub mouth: AtomicU32,
    pub teacher: AtomicU32,
    pub failures: AtomicU32,
}

impl LlmCounters {
    pub fn snapshot(&self) -> (u32, u32, u32, u32) {
        (
            self.ears.load(Ordering::Relaxed),
            self.mouth.load(Ordering::Relaxed),
            self.teacher.load(Ordering::Relaxed),
            self.failures.load(Ordering::Relaxed),
        )
    }
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    pub counters: Arc<LlmCounters>,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    temperature: f32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: ChatMessage,
}

impl Default for LlmClient {
    fn default() -> Self {
        LlmClient::new()
    }
}

impl LlmClient {
    pub fn new() -> LlmClient {
        LlmClient {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("http client"),
            counters: Arc::new(LlmCounters::default()),
        }
    }

    fn count(&self, seat: Seat) {
        let c = match seat {
            Seat::Ears => &self.counters.ears,
            Seat::Mouth => &self.counters.mouth,
            Seat::Teacher => &self.counters.teacher,
        };
        c.fetch_add(1, Ordering::Relaxed);
    }

    /// One chat completion. `json` requests a JSON object response.
    pub async fn chat(
        &self,
        seat: Seat,
        cfg: &LlmConfig,
        mut messages: Vec<ChatMessage>,
        json: bool,
        max_tokens: Option<u32>,
    ) -> anyhow::Result<String> {
        self.count(seat);
        if cfg.no_think && cfg.model.starts_with("qwen") {
            // Qwen3 honors a soft switch in the prompt when thinking is not
            // disabled at the API level.
            if let Some(last) = messages.last_mut() {
                if last.role == "user" && !last.content.contains("/no_think") {
                    last.content.push_str("\n/no_think");
                }
            }
        }
        let req = ChatRequest {
            model: &cfg.model,
            messages: &messages,
            temperature: cfg.temperature,
            stream: false,
            response_format: json.then(|| serde_json::json!({"type": "json_object"})),
            max_tokens,
        };
        let url = format!("{}/chat/completions", cfg.base_url);
        let mut r = self.http.post(&url).timeout(Duration::from_secs(cfg.timeout_secs)).json(&req);
        if let Some(k) = &cfg.api_key {
            r = r.bearer_auth(k);
        }
        let resp = r.send().await;
        let resp = match resp {
            Ok(x) => x,
            Err(e) => {
                self.counters.failures.fetch_add(1, Ordering::Relaxed);
                return Err(anyhow::anyhow!("llm request failed: {e}"));
            }
        };
        if !resp.status().is_success() {
            self.counters.failures.fetch_add(1, Ordering::Relaxed);
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("llm http {status}: {body}"));
        }
        let parsed: ChatResponse = resp.json().await?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        Ok(strip_think(&content))
    }

    /// Is the endpoint reachable? Used to decide offline mode at startup.
    pub async fn ping(&self, cfg: &LlmConfig) -> bool {
        let url = format!("{}/models", cfg.base_url);
        let mut r = self.http.get(&url).timeout(Duration::from_secs(3));
        if let Some(k) = &cfg.api_key {
            r = r.bearer_auth(k);
        }
        matches!(r.send().await, Ok(x) if x.status().is_success())
    }
}

/// Remove `<think>...</think>` blocks some models emit even when asked not to.
pub fn strip_think(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}
