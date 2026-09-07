//! The config file at `~/.spoon/config.json`.
//!
//! It already existed and Spoon was ignoring it, so a teacher set to a 27b
//! model had been running on the 4b default the whole time and every
//! measurement of "the Teacher is unreliable" was really a measurement of the
//! wrong model.
//!
//! Precedence: an explicit command line flag beats the file, the file beats
//! the built-in default. Flags stay authoritative because a one-off run is
//! exactly when you want to override what you normally use.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub database: Option<Database>,
    pub ears: Option<Seat>,
    pub mouth: Option<Seat>,
    pub teacher: Option<Seat>,
    pub capabilities: Option<Capabilities>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Database {
    pub path: Option<PathBuf>,
}

/// One seat's model and how to reach it.
///
/// `provider` was read and thrown away for as long as the file existed, which
/// is why a teacher pointed at a frontier endpoint quietly ran against the
/// local default. It now selects the transport.
///
/// `base_url` for an `openai` provider has to include the version prefix, so
/// `https://api.openai.com/v1` rather than the bare host: the client appends
/// `/chat/completions` and nothing else.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Seat {
    /// `ollama` (the default) or `openai`, meaning any OpenAI-compatible
    /// endpoint rather than that vendor specifically.
    pub provider: Option<String>,
    pub model: Option<String>,
    #[serde(rename = "baseUrl")]
    pub base_url: Option<String>,
    /// Environment variable holding the key, not the key itself. A config file
    /// with a secret in it ends up in a git diff eventually.
    #[serde(rename = "apiKeyEnv")]
    pub api_key_env: Option<String>,
    /// Seconds. A 27b model on a laptop needs more than the 20s a 4b wants.
    #[serde(rename = "timeoutSecs")]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    #[serde(rename = "permissionMode")]
    pub permission_mode: Option<String>,
}

impl Config {
    /// Read the config, or a default if there is none.
    ///
    /// A malformed file is reported rather than swallowed. Silently falling
    /// back to defaults is how a 27b teacher setting goes unnoticed for a
    /// week.
    pub fn load() -> anyhow::Result<Self> {
        let Ok(home) = std::env::var("HOME") else {
            return Ok(Config::default());
        };
        let path = PathBuf::from(home).join(".spoon/config.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(Config::default());
        };
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    }

    /// The permission mode, translated from the file's spelling.
    ///
    /// `full-access` is what the v1 config called bypass, and the file on disk
    /// uses it, so it is accepted rather than quietly ignored.
    pub fn permission_mode(&self) -> Option<&str> {
        match self.capabilities.as_ref()?.permission_mode.as_deref()? {
            "full-access" => Some("bypass"),
            other => Some(other),
        }
    }
}
