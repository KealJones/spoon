//! The one LLM client. Two transports: Ollama's native `/api/chat` (needed to
//! hard-disable thinking on Qwen3 models; the OpenAI-compatible shim ignores
//! `think`) and OpenAI-compatible `/v1/chat/completions` for frontier
//! providers acting as a teacher. Every call is tagged with a `Seat` so
//! metrics can prove the interior never calls it.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    #[default]
    Ollama,
    OpenAi,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Ollama: "http://localhost:11434". OpenAI-compatible: ".../v1".
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Per-attempt timeout for the Mouth seat. Shorter than the general timeout
    /// so a stalled surface-realizer falls back to templates quickly.
    #[serde(default = "default_mouth_timeout")]
    pub mouth_timeout_secs: u64,
    #[serde(default)]
    pub temperature: f32,
    /// Disable chain-of-thought (Qwen3 `think`). Codec seats always want this.
    #[serde(default = "default_true")]
    pub no_think: bool,
}

fn default_timeout() -> u64 {
    60
}
fn default_mouth_timeout() -> u64 {
    8
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
                .trim_end_matches("/v1")
                .to_string(),
            api_key: None,
            model: model.to_string(),
            transport: Transport::Ollama,
            timeout_secs: 60,
            mouth_timeout_secs: 8,
            temperature: 0.0,
            no_think: true,
        }
    }
    pub fn openai(base_url: &str, api_key: Option<String>, model: &str) -> LlmConfig {
        LlmConfig {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model: model.to_string(),
            transport: Transport::OpenAi,
            timeout_secs: 120,
            mouth_timeout_secs: 8,
            temperature: 0.0,
            no_think: true,
        }
    }
    /// Build from env: SPOON_TEACHER_URL / SPOON_TEACHER_KEY / SPOON_TEACHER_MODEL
    /// (OpenAI-compatible) else Ollama with the given default model.
    pub fn teacher_from_env(default_model: &str) -> LlmConfig {
        match std::env::var("SPOON_TEACHER_URL") {
            Ok(url) => LlmConfig::openai(
                &url,
                std::env::var("SPOON_TEACHER_KEY").ok(),
                &std::env::var("SPOON_TEACHER_MODEL").unwrap_or_else(|_| default_model.into()),
            ),
            Err(_) => LlmConfig::ollama(
                &std::env::var("SPOON_TEACHER_MODEL").unwrap_or_else(|_| default_model.into()),
            ),
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

// --- OpenAI-compatible wire types ---
#[derive(Serialize)]
struct OaRequest<'a> {
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
struct OaResponse {
    choices: Vec<OaChoice>,
}
#[derive(Deserialize)]
struct OaChoice {
    message: ChatMessage,
}

// --- Ollama native wire types ---
#[derive(Serialize)]
struct OlRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    think: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'a str>,
    options: OlOptions,
}
#[derive(Serialize)]
struct OlOptions {
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}
#[derive(Deserialize)]
struct OlResponse {
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
                .timeout(Duration::from_secs(180))
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
        messages: Vec<ChatMessage>,
        json: bool,
        max_tokens: Option<u32>,
    ) -> anyhow::Result<String> {
        self.count(seat);
        let result = match cfg.transport {
            Transport::Ollama => self.chat_ollama(cfg, &messages, json, max_tokens).await,
            Transport::OpenAi => self.chat_openai(cfg, messages, json, max_tokens).await,
        };
        if result.is_err() {
            self.counters.failures.fetch_add(1, Ordering::Relaxed);
        }
        result.map(|s| strip_think(&s))
    }

    async fn chat_ollama(
        &self,
        cfg: &LlmConfig,
        messages: &[ChatMessage],
        json: bool,
        max_tokens: Option<u32>,
    ) -> anyhow::Result<String> {
        let req = OlRequest {
            model: &cfg.model,
            messages,
            stream: false,
            think: !cfg.no_think,
            format: json.then_some("json"),
            options: OlOptions { temperature: cfg.temperature, num_predict: max_tokens },
        };
        let url = format!("{}/api/chat", cfg.base_url);
        let resp = self
            .http
            .post(&url)
            .timeout(Duration::from_secs(cfg.timeout_secs))
            .json(&req)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ollama request failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("ollama http {status}: {body}");
        }
        let parsed: OlResponse = resp.json().await?;
        Ok(parsed.message.content)
    }

    async fn chat_openai(
        &self,
        cfg: &LlmConfig,
        mut messages: Vec<ChatMessage>,
        json: bool,
        max_tokens: Option<u32>,
    ) -> anyhow::Result<String> {
        if cfg.no_think && cfg.model.to_lowercase().contains("qwen") {
            if let Some(last) = messages.last_mut() {
                if last.role == "user" && !last.content.contains("/no_think") {
                    last.content.push_str("\n/no_think");
                }
            }
        }
        let req = OaRequest {
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
        let resp = r.send().await.map_err(|e| anyhow::anyhow!("llm request failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("llm http {status}: {body}");
        }
        let parsed: OaResponse = resp.json().await?;
        Ok(parsed.choices.into_iter().next().map(|c| c.message.content).unwrap_or_default())
    }

    /// Is the endpoint reachable? Used to decide offline mode at startup.
    pub async fn ping(&self, cfg: &LlmConfig) -> bool {
        let url = match cfg.transport {
            Transport::Ollama => format!("{}/api/tags", cfg.base_url),
            Transport::OpenAi => format!("{}/models", cfg.base_url),
        };
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
