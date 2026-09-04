//! One HTTP client, three seats, separate counters.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::seats::Seat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Message {
            role: Role::System,
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Message {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// Which wire protocol to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Ollama's own `/api/chat`.
    ///
    /// Preferred for local models. The OpenAI compatibility shim ignores the
    /// `think` flag, and a small reasoning model left to think will burn
    /// thousands of tokens and time out before it answers.
    Ollama,
    /// Any OpenAI-compatible `/v1/chat/completions`, for a frontier teacher.
    OpenAi,
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub transport: Transport,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
    /// Ask the model not to emit reasoning tokens, where the transport
    /// supports saying so.
    pub suppress_thinking: bool,
    pub temperature: f32,
}

impl LlmConfig {
    /// A local Ollama model. The default for ears and mouth.
    pub fn ollama(model: impl Into<String>) -> Self {
        LlmConfig {
            transport: Transport::Ollama,
            base_url: "http://localhost:11434".to_string(),
            model: model.into(),
            api_key: None,
            // Short on purpose. A seat that takes a minute has already failed:
            // the deterministic fallback would have answered instantly and the
            // user is still waiting.
            timeout: Duration::from_secs(20),
            suppress_thinking: true,
            temperature: 0.2,
        }
    }

    /// A frontier model behind an OpenAI-compatible endpoint, for the Teacher.
    pub fn openai(
        base_url: impl Into<String>,
        model: impl Into<String>,
        key: Option<String>,
    ) -> Self {
        LlmConfig {
            transport: Transport::OpenAi,
            base_url: base_url.into(),
            model: model.into(),
            api_key: key,
            timeout: Duration::from_secs(120),
            suppress_thinking: false,
            temperature: 0.2,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("no model is configured for the {0} seat")]
    NoSeat(&'static str),
    #[error("{seat} seat timed out after {millis}ms")]
    Timeout { seat: &'static str, millis: u128 },
    #[error("{seat} seat transport: {source}")]
    Transport {
        seat: &'static str,
        source: reqwest::Error,
    },
    #[error("{seat} seat returned unusable output: {detail}")]
    Unusable { seat: &'static str, detail: String },
    #[error("{seat} seat: {status} {body}")]
    Status {
        seat: &'static str,
        status: u16,
        body: String,
    },
}

/// How many times each seat has been used.
///
/// `interior` must stay zero. It exists so a test can assert the thing the
/// whole design rests on, rather than trusting that nobody wired a model into
/// the middle of the system.
#[derive(Debug, Default)]
pub struct SeatCounters {
    ears: AtomicU64,
    mouth: AtomicU64,
    teacher: AtomicU64,
    interior: AtomicU64,
}

impl SeatCounters {
    pub fn record(&self, seat: Seat) {
        match seat {
            Seat::Ears => &self.ears,
            Seat::Mouth => &self.mouth,
            Seat::Teacher => &self.teacher,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    /// Called by nothing. Any non-zero reading here means a model was consulted
    /// somewhere it must never be.
    pub fn record_interior(&self) {
        self.interior.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get(&self, seat: Seat) -> u64 {
        match seat {
            Seat::Ears => self.ears.load(Ordering::Relaxed),
            Seat::Mouth => self.mouth.load(Ordering::Relaxed),
            Seat::Teacher => self.teacher.load(Ordering::Relaxed),
        }
    }

    pub fn interior(&self) -> u64 {
        self.interior.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.ears.store(0, Ordering::Relaxed);
        self.mouth.store(0, Ordering::Relaxed);
        self.teacher.store(0, Ordering::Relaxed);
        self.interior.store(0, Ordering::Relaxed);
    }
}

/// A configured model, shared by whichever seats point at it.
#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    config: LlmConfig,
    counters: Arc<SeatCounters>,
}

impl LlmClient {
    pub fn new(config: LlmConfig, counters: Arc<SeatCounters>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .unwrap_or_default();
        LlmClient {
            http,
            config,
            counters,
        }
    }

    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    pub fn counters(&self) -> &Arc<SeatCounters> {
        &self.counters
    }

    /// Send a conversation and get the reply text.
    pub async fn chat(&self, seat: Seat, messages: &[Message]) -> Result<String, LlmError> {
        self.counters.record(seat);
        let started = std::time::Instant::now();
        let result = match self.config.transport {
            Transport::Ollama => self.chat_ollama(seat, messages).await,
            Transport::OpenAi => self.chat_openai(seat, messages).await,
        };
        match result {
            Err(LlmError::Transport { source, .. }) if source.is_timeout() => {
                Err(LlmError::Timeout {
                    seat: seat.as_str(),
                    millis: started.elapsed().as_millis(),
                })
            }
            other => other,
        }
    }

    async fn chat_ollama(&self, seat: Seat, messages: &[Message]) -> Result<String, LlmError> {
        let body = serde_json::json!({
            "model": self.config.model,
            "messages": messages,
            "stream": false,
            "think": !self.config.suppress_thinking,
            "options": { "temperature": self.config.temperature },
        });
        let url = format!("{}/api/chat", self.config.base_url.trim_end_matches('/'));
        let response = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|source| LlmError::Transport {
                seat: seat.as_str(),
                source,
            })?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|source| LlmError::Transport {
                seat: seat.as_str(),
                source,
            })?;
        if !status.is_success() {
            return Err(LlmError::Status {
                seat: seat.as_str(),
                status: status.as_u16(),
                body: text,
            });
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| LlmError::Unusable {
                seat: seat.as_str(),
                detail: e.to_string(),
            })?;
        parsed["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| LlmError::Unusable {
                seat: seat.as_str(),
                detail: format!("no message content in {text}"),
            })
    }

    async fn chat_openai(&self, seat: Seat, messages: &[Message]) -> Result<String, LlmError> {
        let body = serde_json::json!({
            "model": self.config.model,
            "messages": messages,
            "temperature": self.config.temperature,
        });
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut request = self.http.post(url).json(&body);
        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(|source| LlmError::Transport {
            seat: seat.as_str(),
            source,
        })?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|source| LlmError::Transport {
                seat: seat.as_str(),
                source,
            })?;
        if !status.is_success() {
            return Err(LlmError::Status {
                seat: seat.as_str(),
                status: status.as_u16(),
                body: text,
            });
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| LlmError::Unusable {
                seat: seat.as_str(),
                detail: e.to_string(),
            })?;
        parsed["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| LlmError::Unusable {
                seat: seat.as_str(),
                detail: format!("no choice content in {text}"),
            })
    }

    /// Whether the configured endpoint answers at all.
    pub async fn reachable(&self) -> bool {
        let url = match self.config.transport {
            Transport::Ollama => format!("{}/api/tags", self.config.base_url.trim_end_matches('/')),
            Transport::OpenAi => format!("{}/models", self.config.base_url.trim_end_matches('/')),
        };
        let mut request = self.http.get(url).timeout(Duration::from_secs(3));
        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key);
        }
        matches!(request.send().await, Ok(r) if r.status().is_success())
    }
}
