use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use thiserror::Error;

const DEFAULT_DATABASE: &str = "spoon.db";
const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";
const DEFAULT_HTTP_MODEL: &str = "qwen2.5:1.5b";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{0}")]
    Message(String),
    #[error("current directory: {0}")]
    CurrentDir(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct TeacherConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedRuntime {
    pub cwd: PathBuf,
    pub database_path: PathBuf,
    pub teacher_enabled: bool,
    pub teacher_provider: String,
    pub teacher_command: Option<String>,
    pub teacher: Option<TeacherConfig>,
    pub interpreter_provider: String,
    pub interpreter: Option<TeacherConfig>,
    pub permission_mode: String,
    pub recall_mode: String,
    pub recall_lookback: Option<String>,
    pub recall_max_episodes: u32,
    pub output_mode: String,
}

pub struct ResolveInput<'a> {
    pub cwd: &'a Path,
    pub home: &'a Path,
    pub env: &'a BTreeMap<String, String>,
}

const ENV_KEYS: &[&str] = &[
    "SPOON_DB",
    "SPOON_TEACHER",
    "SPOON_TEACHER_MODEL",
    "SPOON_TEACHER_ENABLED",
    "SPOON_TEACHER_URL",
    "SPOON_TEACHER_API_KEY",
    "SPOON_OLLAMA_URL",
    "SPOON_INTERPRETER",
    "SPOON_INTERPRETER_MODEL",
    "SPOON_PERMISSION_MODE",
    "SPOON_RECALL_MODE",
    "SPOON_RECALL_MAX_EPISODES",
];

pub fn resolve_from_process() -> Result<ResolvedRuntime, ConfigError> {
    let cwd = std::env::current_dir()?;
    let home = home_dir().ok_or_else(|| {
        ConfigError::Message("HOME or USERPROFILE must be set to load Spoon config".into())
    })?;
    resolve(ResolveInput {
        cwd: &cwd,
        home: &home,
        env: &process_env(),
    })
}

pub fn resolve(input: ResolveInput<'_>) -> Result<ResolvedRuntime, ConfigError> {
    let mut merged = default_layer();

    for path in config_file_paths(input.cwd, input.home) {
        let Some(mut layer) = read_optional_json(&path)? else {
            continue;
        };
        if let Some(object) = layer.as_object_mut() {
            object.remove("$schema");
        }
        validate_layer(&layer, &path.display().to_string())?;
        normalize_database_path(&mut layer, path.parent().unwrap_or(input.cwd));
        merged = deep_merge(merged, layer);
    }

    if let Some(enabled) = input.env.get("SPOON_TEACHER_ENABLED")
        && enabled != "true"
        && enabled != "false"
    {
        return Err(ConfigError::Message(
            "SPOON_TEACHER_ENABLED must be true or false".into(),
        ));
    }
    merged = deep_merge(merged, environment_layer(input.env));
    validate_layer(&merged, "effective configuration")?;

    let mut database_path = string_at(&merged, &["database", "path"])
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DATABASE));
    if !database_path.is_absolute() {
        database_path = input.cwd.join(database_path);
    }

    let teacher_enabled = merged
        .pointer("/teacher/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let teacher_provider =
        string_at(&merged, &["teacher", "provider"]).unwrap_or_else(|| "claude".into());
    let interpreter_provider = string_at(&merged, &["language", "interpreter", "provider"])
        .unwrap_or_else(|| "off".into());
    let base_url = llm_base_url(&merged, input.env);
    let api_key = input.env.get("SPOON_TEACHER_API_KEY").cloned();
    let teacher_model = string_at(&merged, &["teacher", "model"]).or_else(|| {
        if teacher_provider.eq_ignore_ascii_case("ollama") {
            string_at(&merged, &["language", "interpreter", "model"])
        } else {
            None
        }
    });
    let interpreter_model =
        string_at(&merged, &["language", "interpreter", "model"]).or_else(|| teacher_model.clone());

    Ok(ResolvedRuntime {
        cwd: input.cwd.to_path_buf(),
        database_path,
        teacher: http_llm(
            teacher_enabled,
            &teacher_provider,
            teacher_model,
            &base_url,
            api_key.clone(),
            input.env.contains_key("SPOON_TEACHER_URL"),
        ),
        teacher_enabled,
        teacher_provider: teacher_provider.clone(),
        teacher_command: string_at(&merged, &["teacher", "command"]),
        interpreter: http_llm(
            interpreter_provider.eq_ignore_ascii_case("ollama"),
            &interpreter_provider,
            interpreter_model,
            &base_url,
            api_key,
            input.env.contains_key("SPOON_TEACHER_URL"),
        ),
        interpreter_provider,
        permission_mode: string_at(&merged, &["capabilities", "permissionMode"])
            .unwrap_or_else(|| "ask".into()),
        recall_mode: string_at(&merged, &["recall", "mode"]).unwrap_or_else(|| "global".into()),
        recall_lookback: string_at(&merged, &["recall", "lookback"]),
        recall_max_episodes: merged
            .pointer("/recall/maxEpisodes")
            .and_then(Value::as_u64)
            .unwrap_or(64) as u32,
        output_mode: string_at(&merged, &["output", "mode"]).unwrap_or_else(|| "normal".into()),
    })
}

fn default_layer() -> Value {
    json!({
        "version": 1,
        "database": { "path": DEFAULT_DATABASE },
        "teacher": { "enabled": true, "provider": "claude", "model": null, "command": null },
        "capabilities": { "permissionMode": "ask" },
        "recall": { "mode": "global", "lookback": "90d", "maxEpisodes": 64 },
        "language": {
            "interpreter": { "provider": "off", "model": null, "baseUrl": null }
        },
        "output": { "mode": "normal" }
    })
}

fn process_env() -> BTreeMap<String, String> {
    ENV_KEYS
        .iter()
        .filter_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| ((*key).to_string(), value))
        })
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn config_file_paths(cwd: &Path, home: &Path) -> Vec<PathBuf> {
    let mut ancestors = Vec::new();
    let mut current = cwd.to_path_buf();
    loop {
        ancestors.push(current.clone());
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    ancestors.reverse();
    let mut paths = vec![home.join(".spoon").join("config.json")];
    for directory in ancestors {
        let candidate = directory.join(".spoon").join("config.json");
        if !paths.contains(&candidate) {
            paths.push(candidate);
        }
    }
    paths.push(cwd.join(".spoon").join("config.local.json"));
    paths
}

fn read_optional_json(path: &Path) -> Result<Option<Value>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
            ConfigError::Message(format!("{}: invalid JSON ({error})", path.display()))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ConfigError::Message(format!("{}: {error}", path.display()))),
    }
}

fn normalize_database_path(layer: &mut Value, base: &Path) {
    let Some(path) = string_at(layer, &["database", "path"]) else {
        return;
    };
    let path = PathBuf::from(path);
    if path.is_absolute() {
        return;
    }
    if let Some(database) = layer.get_mut("database").and_then(Value::as_object_mut) {
        database.insert(
            "path".into(),
            Value::String(base.join(path).to_string_lossy().into_owned()),
        );
    }
}

fn environment_layer(env: &BTreeMap<String, String>) -> Value {
    let mut layer = json!({});
    if let Some(path) = env.get("SPOON_DB") {
        layer["database"] = json!({ "path": path });
    }
    let mut teacher = json!({});
    if let Some(provider) = env.get("SPOON_TEACHER") {
        teacher["provider"] = json!(provider);
    }
    if let Some(model) = env.get("SPOON_TEACHER_MODEL") {
        teacher["model"] = json!(model);
    }
    if let Some(enabled) = env.get("SPOON_TEACHER_ENABLED") {
        teacher["enabled"] = json!(enabled == "true");
    }
    if teacher.as_object().is_some_and(|object| !object.is_empty()) {
        layer["teacher"] = teacher;
    }
    if let Some(mode) = env.get("SPOON_PERMISSION_MODE") {
        layer["capabilities"] = json!({ "permissionMode": mode });
    }
    let mut recall = json!({});
    if let Some(mode) = env.get("SPOON_RECALL_MODE") {
        recall["mode"] = json!(mode);
    }
    if let Some(max_episodes) = env.get("SPOON_RECALL_MAX_EPISODES") {
        if let Ok(value) = max_episodes.parse::<u64>() {
            recall["maxEpisodes"] = json!(value);
        }
    }
    if recall.as_object().is_some_and(|object| !object.is_empty()) {
        layer["recall"] = recall;
    }
    let mut interpreter = json!({});
    if let Some(provider) = env.get("SPOON_INTERPRETER") {
        interpreter["provider"] = json!(provider);
    }
    if let Some(model) = env.get("SPOON_INTERPRETER_MODEL") {
        interpreter["model"] = json!(model);
    }
    if let Some(base_url) = env.get("SPOON_OLLAMA_URL") {
        interpreter["baseUrl"] = json!(base_url);
    }
    if interpreter
        .as_object()
        .is_some_and(|object| !object.is_empty())
    {
        layer["language"] = json!({ "interpreter": interpreter });
    }
    layer
}

fn llm_base_url(merged: &Value, env: &BTreeMap<String, String>) -> String {
    env.get("SPOON_TEACHER_URL")
        .cloned()
        .or_else(|| env.get("SPOON_OLLAMA_URL").cloned())
        .or_else(|| string_at(merged, &["language", "interpreter", "baseUrl"]))
        .unwrap_or_else(|| DEFAULT_OLLAMA_URL.into())
}

fn http_llm(
    enabled: bool,
    provider: &str,
    model: Option<String>,
    base_url: &str,
    api_key: Option<String>,
    teacher_url_override: bool,
) -> Option<TeacherConfig> {
    if !enabled {
        return None;
    }
    let http = provider.eq_ignore_ascii_case("ollama")
        || provider.eq_ignore_ascii_case("openai")
        || teacher_url_override;
    let unconfigured_claude = provider.eq_ignore_ascii_case("claude") && model.is_none();
    if !http && !unconfigured_claude {
        return None;
    }
    Some(TeacherConfig {
        provider: if unconfigured_claude {
            "ollama".into()
        } else {
            provider.to_string()
        },
        base_url: base_url.to_string(),
        model: model.unwrap_or_else(|| DEFAULT_HTTP_MODEL.into()),
        api_key,
    })
}

fn validate_layer(value: &Value, source: &str) -> Result<(), ConfigError> {
    let Some(object) = value.as_object() else {
        return Err(ConfigError::Message(format!(
            "{source}: configuration must be a JSON object"
        )));
    };
    if let Some(version) = object.get("version")
        && version.as_u64() != Some(1)
        && version.as_i64() != Some(1)
    {
        return Err(ConfigError::Message(format!("{source}: version must be 1")));
    }
    if let Some(mode) = string_at(value, &["capabilities", "permissionMode"])
        && !matches!(
            mode.as_str(),
            "ask" | "workspace" | "full-access" | "god-mode"
        )
    {
        return Err(ConfigError::Message(format!(
            "{source}: capabilities.permissionMode must be ask, workspace, full-access, or god-mode"
        )));
    }
    if let Some(mode) = string_at(value, &["recall", "mode"])
        && !matches!(mode.as_str(), "global" | "session" | "none")
    {
        return Err(ConfigError::Message(format!(
            "{source}: recall.mode must be global, session, or none"
        )));
    }
    if let Some(mode) = string_at(value, &["output", "mode"])
        && !matches!(mode.as_str(), "quiet" | "normal" | "explain")
    {
        return Err(ConfigError::Message(format!(
            "{source}: output.mode must be quiet, normal, or explain"
        )));
    }
    if let Some(provider) = string_at(value, &["language", "interpreter", "provider"])
        && !matches!(provider.as_str(), "off" | "ollama" | "cursor")
    {
        return Err(ConfigError::Message(format!(
            "{source}: language.interpreter.provider must be off, ollama, or cursor"
        )));
    }
    Ok(())
}

fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Object(mut base_map), Value::Object(overlay_map)) => {
            for (key, value) in overlay_map {
                let next = match base_map.remove(&key) {
                    Some(previous) if previous.is_object() && value.is_object() => {
                        deep_merge(previous, value)
                    }
                    _ => value,
                };
                base_map.insert(key, next);
            }
            Value::Object(base_map)
        }
        (_, overlay) => overlay,
    }
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    match current {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}
