//! Behavioral tests against real macOS binaries. Nothing here is faked: every
//! case spawns (or refuses to spawn) an actual process.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use spoon_capability::{
    AuthorizedPrimitiveInvocation, CapabilityError, CapabilityInvocationAdapter, Effect,
    NativePrimitive, PrimitiveRequest, ResourceBounds,
};
use spoon_sandbox::{
    ArgumentRule, Confinement, SandboxAdapter, SandboxProfile, TIMEOUT_MESSAGE, digest_file,
    sandbox_input,
};

const ECHO: &str = "/bin/echo";
const SLEEP: &str = "/bin/sleep";
const CAT: &str = "/bin/cat";
const FALSE: &str = "/usr/bin/false";
const ENV: &str = "/usr/bin/env";
const NC: &str = "/usr/bin/nc";

/// Each test gets its own root so working-directory cases cannot collide.
fn root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("spoon-sandbox-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::canonicalize(&root).expect("canonical root")
}

fn bounds(max_bytes: u64, max_millis: u64) -> ResourceBounds {
    ResourceBounds {
        max_bytes,
        max_steps: 16,
        max_millis,
    }
}

fn profile(name: &str, executable: &str, arguments: Vec<ArgumentRule>) -> SandboxProfile {
    SandboxProfile {
        name: name.into(),
        executable: PathBuf::from(executable),
        executable_digest: digest_file(executable).expect("digest"),
        arguments,
        environment: BTreeSet::new(),
    }
}

fn text(max_bytes: usize) -> ArgumentRule {
    ArgumentRule::Text { max_bytes }
}

fn literal(value: &str) -> ArgumentRule {
    ArgumentRule::Literal {
        value: value.into(),
    }
}

fn invocation(
    profile: &str,
    input: Value,
    bounds: ResourceBounds,
) -> AuthorizedPrimitiveInvocation {
    AuthorizedPrimitiveInvocation {
        content_id: "content".into(),
        procedure_id: "procedure".into(),
        primitive: NativePrimitive::SandboxExecute,
        effect: Effect::SandboxedExecution,
        request: PrimitiveRequest::SandboxExecute {
            profile: profile.into(),
            steps: 1,
        },
        input,
        bounds,
    }
}

fn adapter(name: &str, profiles: Vec<SandboxProfile>) -> SandboxAdapter {
    SandboxAdapter::new(root(name), bounds(64 * 1024, 5_000), profiles).expect("adapter")
}

#[test]
fn captures_stdout_and_a_zero_exit_status() {
    let mut adapter = adapter("stdout", vec![profile("echo", ECHO, vec![text(32)])]);
    let execution = adapter
        .execute(&invocation(
            "echo",
            sandbox_input(&["hello"], "", ""),
            bounds(64 * 1024, 5_000),
        ))
        .expect("run");

    assert_eq!(execution.effect, Effect::SandboxedExecution);
    assert_eq!(execution.output["stdout"], "hello\n");
    assert_eq!(execution.output["exitCode"], 0);
    assert_eq!(execution.output["signal"], Value::Null);
    assert_eq!(execution.output["truncated"], false);
    assert_eq!(execution.output["osConfinement"], "sandbox-exec");
    assert_eq!(execution.output["confined"], true);
    assert_eq!(execution.usage.steps, 1);
    assert_eq!(execution.usage.bytes, 6);
}

#[test]
fn reports_a_non_zero_exit_status_rather_than_failing() {
    let mut adapter = adapter("exit", vec![profile("false", FALSE, Vec::new())]);
    let execution = adapter
        .execute(&invocation(
            "false",
            sandbox_input(&[], "", ""),
            bounds(64 * 1024, 5_000),
        ))
        .expect("run");

    assert_eq!(execution.output["exitCode"], 1);
    assert_eq!(execution.output["stdout"], "");
}

#[test]
fn delivers_stdin_to_the_child() {
    let mut adapter = adapter("stdin", vec![profile("cat", CAT, Vec::new())]);
    let execution = adapter
        .execute(&invocation(
            "cat",
            sandbox_input(&[], "piped payload", ""),
            bounds(64 * 1024, 5_000),
        ))
        .expect("run");

    assert_eq!(execution.output["stdout"], "piped payload");
    assert_eq!(execution.output["exitCode"], 0);
}

#[test]
fn refuses_an_executable_whose_digest_does_not_match() {
    let mut pinned = profile("echo", ECHO, vec![text(32)]);
    pinned.executable_digest = format!("sha256:{}", "0".repeat(64));
    let mut adapter = adapter("digest", vec![pinned]);

    let error = adapter
        .execute(&invocation(
            "echo",
            sandbox_input(&["hello"], "", ""),
            bounds(64 * 1024, 5_000),
        ))
        .expect_err("digest mismatch");

    assert!(
        matches!(&error, CapabilityError::PermissionRequired(message) if message.contains("digest mismatch")),
        "unexpected error: {error}"
    );
}

#[test]
fn refuses_a_relative_or_path_resolved_executable() {
    for candidate in ["echo", "./echo", "/usr/bin/../bin/echo"] {
        let error = SandboxAdapter::new(
            root("relative"),
            bounds(64 * 1024, 5_000),
            vec![SandboxProfile {
                name: "echo".into(),
                executable: PathBuf::from(candidate),
                executable_digest: digest_file(ECHO).expect("digest"),
                arguments: Vec::new(),
                environment: BTreeSet::new(),
            }],
        )
        .expect_err("relative executable");
        assert!(
            matches!(&error, CapabilityError::Invalid(message) if message.contains("absolute normalized")),
            "unexpected error for {candidate}: {error}"
        );
    }
}

#[test]
fn refuses_an_executable_reached_through_a_symlink() {
    let root = root("symlink-exe");
    let link = root.join("echo");
    std::os::unix::fs::symlink(ECHO, &link).expect("symlink");
    let mut adapter = SandboxAdapter::new(
        &root,
        bounds(64 * 1024, 5_000),
        vec![SandboxProfile {
            name: "echo".into(),
            executable: link,
            executable_digest: digest_file(ECHO).expect("digest"),
            arguments: Vec::new(),
            environment: BTreeSet::new(),
        }],
    )
    .expect("adapter");

    let error = adapter
        .execute(&invocation(
            "echo",
            sandbox_input(&[], "", ""),
            bounds(64 * 1024, 5_000),
        ))
        .expect_err("symlinked executable");
    assert!(
        matches!(&error, CapabilityError::Invalid(message) if message.contains("regular file")),
        "unexpected error: {error}"
    );
}

#[test]
fn refuses_arguments_outside_the_declared_schema() {
    let mut adapter = adapter(
        "arguments",
        vec![profile("echo", ECHO, vec![literal("--"), text(8)])],
    );

    // Wrong literal, an option smuggled into a text slot, an over-long value,
    // and the wrong argument count are all refusals.
    for arguments in [
        vec!["-n", "hello"],
        vec!["--", "-n"],
        vec!["--", "far-too-long-for-the-slot"],
        vec!["--"],
    ] {
        let error = adapter
            .execute(&invocation(
                "echo",
                sandbox_input(&arguments, "", ""),
                bounds(64 * 1024, 5_000),
            ))
            .expect_err("argument refusal");
        assert!(
            matches!(&error, CapabilityError::PermissionRequired(_)),
            "unexpected error for {arguments:?}: {error}"
        );
    }

    let execution = adapter
        .execute(&invocation(
            "echo",
            sandbox_input(&["--", "ok"], "", ""),
            bounds(64 * 1024, 5_000),
        ))
        .expect("permitted arguments");
    assert_eq!(execution.output["stdout"], "-- ok\n");
}

#[test]
fn refuses_a_profile_that_is_not_in_the_policy_allowlist() {
    let mut adapter = adapter("allowlist", vec![profile("echo", ECHO, Vec::new())]);
    assert_eq!(
        adapter.policy().sandbox_profiles,
        BTreeSet::from(["echo".to_string()])
    );

    let error = adapter
        .execute(&invocation(
            "unregistered",
            sandbox_input(&[], "", ""),
            bounds(64 * 1024, 5_000),
        ))
        .expect_err("profile refusal");
    assert!(
        matches!(&error, CapabilityError::PermissionRequired(message) if message.contains("sandbox profile unregistered")),
        "unexpected error: {error}"
    );
}

#[test]
fn kills_a_child_and_its_process_group_when_the_wall_clock_bound_is_exceeded() {
    let mut adapter = adapter("timeout", vec![profile("sleep", SLEEP, vec![text(4)])]);
    let before = sleeping_processes();

    let error = adapter
        .execute(&invocation(
            "sleep",
            sandbox_input(&["30"], "", ""),
            bounds(64 * 1024, 200),
        ))
        .expect_err("timeout");
    assert!(
        matches!(&error, CapabilityError::Invalid(message) if message.contains(TIMEOUT_MESSAGE)),
        "unexpected error: {error}"
    );

    // The adapter already proved the process group is gone before returning;
    // this independently confirms no `sleep 30` survived the cancellation.
    assert_eq!(sleeping_processes(), before);
}

/// Count live `sleep 30` processes owned by this user, via a real `ps`.
fn sleeping_processes() -> usize {
    let output = std::process::Command::new("/bin/ps")
        .args(["-x", "-o", "command"])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains("sleep 30"))
        .count()
}

#[test]
fn truncates_output_that_exceeds_the_byte_bound_and_flags_it() {
    let mut adapter = adapter("truncate", vec![profile("echo", ECHO, vec![text(4096)])]);
    let payload = "x".repeat(4000);

    let execution = adapter
        .execute(&invocation(
            "echo",
            sandbox_input(&[payload.as_str()], "", ""),
            bounds(64, 5_000),
        ))
        .expect("run");

    assert_eq!(execution.output["truncated"], true);
    assert_eq!(execution.output["stdoutBytes"], 4001);
    assert_eq!(
        execution.output["stdout"].as_str().expect("stdout").len(),
        64
    );
    assert_eq!(execution.usage.bytes, 64);
}

#[test]
fn starts_the_child_with_an_empty_environment_plus_the_allowlist() {
    // Safety: single-threaded setup before any child is spawned in this test.
    unsafe {
        std::env::set_var("SPOON_SANDBOX_LEAK", "leaked");
        std::env::set_var("SPOON_SANDBOX_ALLOWED", "carried");
    }
    let mut allowlisted = profile("env", ENV, Vec::new());
    allowlisted
        .environment
        .insert("SPOON_SANDBOX_ALLOWED".into());
    let mut adapter = adapter("environment", vec![allowlisted]);

    let execution = adapter
        .execute(&invocation(
            "env",
            sandbox_input(&[], "", ""),
            bounds(64 * 1024, 5_000),
        ))
        .expect("run");

    let stdout = execution.output["stdout"].as_str().expect("stdout");
    assert!(!stdout.contains("SPOON_SANDBOX_LEAK"), "stdout: {stdout}");
    assert!(
        stdout.contains("SPOON_SANDBOX_ALLOWED=carried"),
        "stdout: {stdout}"
    );
    assert!(!stdout.contains("HOME="), "stdout: {stdout}");
}

#[test]
fn refuses_a_working_directory_outside_the_configured_root() {
    let root = root("workdir");
    std::fs::create_dir_all(root.join("inside")).expect("inside");
    let outside = std::env::temp_dir().join("spoon-sandbox-workdir-outside");
    std::fs::create_dir_all(&outside).expect("outside");
    std::os::unix::fs::symlink(&outside, root.join("escape")).expect("symlink");

    let mut adapter = SandboxAdapter::new(
        &root,
        bounds(64 * 1024, 5_000),
        vec![profile("echo", ECHO, Vec::new())],
    )
    .expect("adapter");

    let permitted = adapter
        .execute(&invocation(
            "echo",
            sandbox_input(&[], "", "inside"),
            bounds(64 * 1024, 5_000),
        ))
        .expect("inside the root");
    assert_eq!(
        permitted.output["workingDirectory"],
        root.join("inside").to_string_lossy().as_ref()
    );

    // A traversal is rejected by shape; the symlink is only caught after the
    // resolved path is compared against the root.
    for requested in ["../elsewhere", "/etc", "escape"] {
        let error = adapter
            .execute(&invocation(
                "echo",
                sandbox_input(&[], "", requested),
                bounds(64 * 1024, 5_000),
            ))
            .expect_err("working directory refusal");
        assert!(
            matches!(
                &error,
                CapabilityError::Invalid(_) | CapabilityError::PermissionRequired(_)
            ),
            "unexpected error for {requested}: {error}"
        );
    }

    let error = adapter
        .execute(&invocation(
            "echo",
            sandbox_input(&[], "", "escape"),
            bounds(64 * 1024, 5_000),
        ))
        .expect_err("symlink escape");
    assert!(
        matches!(&error, CapabilityError::PermissionRequired(message) if message.contains("escaped the configured root")),
        "unexpected error: {error}"
    );
}

#[test]
fn sandbox_exec_confinement_denies_network_access() {
    if !Path::new(NC).is_file() {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("addr").port().to_string();
    let arguments = vec![
        literal("-z"),
        literal("-w"),
        literal("1"),
        literal("127.0.0.1"),
        text(6),
    ];
    let supplied = ["-z", "-w", "1", "127.0.0.1", port.as_str()];

    let mut confined = adapter("network-denied", vec![profile("nc", NC, arguments.clone())]);
    let denied = confined
        .execute(&invocation(
            "nc",
            sandbox_input(&supplied, "", ""),
            bounds(64 * 1024, 5_000),
        ))
        .expect("run");
    assert_eq!(denied.output["osConfinement"], "sandbox-exec");
    assert_ne!(denied.output["exitCode"], 0);

    // The same call without kernel confinement reaches the listener, which is
    // what makes the denial above attributable to `sandbox-exec`.
    let mut unconfined = adapter("network-allowed", vec![profile("nc", NC, arguments)])
        .with_confinement(Confinement::None);
    let allowed = unconfined
        .execute(&invocation(
            "nc",
            sandbox_input(&supplied, "", ""),
            bounds(64 * 1024, 5_000),
        ))
        .expect("run");
    assert_eq!(allowed.output["osConfinement"], "none");
    assert_eq!(allowed.output["confined"], false);
    assert_eq!(allowed.output["exitCode"], 0);
    drop(listener);
}
