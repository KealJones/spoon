use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use spoon_server::config::{ResolveInput, resolve};

fn scratch() -> PathBuf {
    let root = std::env::temp_dir().join(format!("spoon-config-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(root.join("cwd")).unwrap();
    root
}

fn write_json(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn resolve_at(root: &Path, env: BTreeMap<String, String>) -> spoon_server::config::ResolvedRuntime {
    resolve(ResolveInput {
        cwd: &root.join("cwd"),
        home: &root.join("home"),
        env: &env,
    })
    .expect("resolve")
}

#[test]
fn home_config_teacher_model_is_used_when_env_is_empty() {
    let root = scratch();
    write_json(
        &root.join("home/.spoon/config.json"),
        r#"{"version":1,"teacher":{"provider":"ollama","model":"qwen3.8:27b"}}"#,
    );

    let teacher = resolve_at(&root, BTreeMap::new()).teacher.expect("teacher");
    assert_eq!(teacher.model, "qwen3.8:27b");
    assert_eq!(teacher.base_url, "http://localhost:11434");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn env_teacher_model_overrides_home_config() {
    let root = scratch();
    write_json(
        &root.join("home/.spoon/config.json"),
        r#"{"version":1,"teacher":{"provider":"ollama","model":"qwen3.8:27b"}}"#,
    );
    let env = BTreeMap::from([("SPOON_TEACHER_MODEL".into(), "qwen2.5:1.5b".into())]);

    let teacher = resolve_at(&root, env).teacher.expect("teacher");
    assert_eq!(teacher.model, "qwen2.5:1.5b");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn home_config_database_path_is_used_when_env_is_empty() {
    let root = scratch();
    let db = root.join("shared/spoon.db");
    write_json(
        &root.join("home/.spoon/config.json"),
        &format!(
            r#"{{"version":1,"database":{{"path":"{}"}}}}"#,
            db.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    let resolved = resolve_at(&root, BTreeMap::new());
    assert_eq!(resolved.database_path, db);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn home_config_applies_permission_recall_and_interpreter() {
    let root = scratch();
    write_json(
        &root.join("home/.spoon/config.json"),
        r#"{
            "version": 1,
            "teacher": {"provider": "ollama", "model": "qwen3.8:27b"},
            "capabilities": {"permissionMode": "full-access"},
            "recall": {"mode": "session", "lookback": "7d", "maxEpisodes": 12},
            "language": {"interpreter": {"provider": "ollama", "model": "qwen3.8:27b", "baseUrl": "http://127.0.0.1:11434"}},
            "output": {"mode": "explain"}
        }"#,
    );

    let resolved = resolve_at(&root, BTreeMap::new());
    assert_eq!(resolved.permission_mode, "full-access");
    assert_eq!(resolved.recall_mode, "session");
    assert_eq!(resolved.recall_lookback.as_deref(), Some("7d"));
    assert_eq!(resolved.recall_max_episodes, 12);
    assert_eq!(resolved.output_mode, "explain");
    assert_eq!(resolved.teacher.as_ref().unwrap().model, "qwen3.8:27b");
    assert_eq!(resolved.interpreter.as_ref().unwrap().model, "qwen3.8:27b");
    assert_eq!(
        resolved.interpreter.as_ref().unwrap().base_url,
        "http://127.0.0.1:11434"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn env_permission_mode_overrides_home_config() {
    let root = scratch();
    write_json(
        &root.join("home/.spoon/config.json"),
        r#"{"version":1,"capabilities":{"permissionMode":"full-access"}}"#,
    );
    let env = BTreeMap::from([("SPOON_PERMISSION_MODE".into(), "ask".into())]);

    let resolved = resolve_at(&root, env);
    assert_eq!(resolved.permission_mode, "ask");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn interpreter_stays_off_unless_configured() {
    let root = scratch();
    write_json(
        &root.join("home/.spoon/config.json"),
        r#"{"version":1,"teacher":{"provider":"ollama","model":"qwen3.8:27b"}}"#,
    );

    let resolved = resolve_at(&root, BTreeMap::new());
    assert_eq!(resolved.interpreter_provider, "off");
    assert!(resolved.interpreter.is_none());

    let _ = fs::remove_dir_all(root);
}
