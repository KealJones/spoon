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

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Seat {
    pub provider: Option<String>,
    pub model: Option<String>,
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
        serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    }

    pub fn model(seat: &Option<Seat>) -> Option<&str> {
        seat.as_ref()?.model.as_deref()
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
