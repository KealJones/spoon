//! A real operating-system sandbox runner for the `SandboxExecute` primitive.
//!
//! Unlike the deterministic fixture executor in `spoon-capability`, this
//! adapter spawns an actual process. Every degree of freedom a caller has is
//! bounded by a host-owned profile: the executable is pinned by absolute path
//! and SHA-256 digest, arguments must match a declared positional schema, the
//! environment starts empty, and the working directory must resolve inside one
//! configured root. On macOS the child is additionally wrapped in
//! `sandbox-exec` so the kernel denies network access and writes outside the
//! working directory.
//!
//! The implementation is Unix-only; it relies on process groups for
//! cancellation.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use spoon_capability::{
    AdapterExecution, AuthorizedPrimitiveInvocation, CapabilityError, CapabilityInvocationAdapter,
    MAX_RESOURCE_BYTES, MAX_RESOURCE_MILLIS, MAX_RESOURCE_STEPS, NativePrimitive, PrimitivePolicy,
    PrimitiveRequest, ResourceBounds, ResourceUsage,
};

// `setsid` moves the child into a fresh session and process group so a timeout
// can signal the whole tree instead of just the direct child. Without it,
// anything the child forked before cancellation would be orphaned.
unsafe extern "C" {
    fn setsid() -> i32;
    fn killpg(process_group: i32, signal: i32) -> i32;
}

const SIGKILL: i32 = 9;
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Stable prefix of the wall-clock cancellation error, so callers can classify
/// a timeout without parsing a whole message.
pub const TIMEOUT_MESSAGE: &str = "sandbox wall-clock bound exceeded";

/// Whether the host kernel confinement layer was actually applied. Reported in
/// the structured output so a caller never has to assume confinement it did
/// not get.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confinement {
    /// The child ran under `sandbox-exec` with a generated SBPL profile that
    /// denies network access and all writes outside the working directory.
    SandboxExec,
    /// No kernel confinement was applied. Every in-process limit still holds,
    /// but the child could reach the network and write anywhere its user can.
    None,
}

impl Confinement {
    fn label(self) -> &'static str {
        match self {
            Self::SandboxExec => "sandbox-exec",
            Self::None => "none",
        }
    }
}

/// One positional argument slot. A profile declares a rule per slot, and the
/// supplied argument list must match slot for slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArgumentRule {
    /// Exactly this token, chosen by the host.
    Literal { value: String },
    /// One token drawn from a host-chosen set.
    OneOf { values: BTreeSet<String> },
    /// Caller-supplied text, bounded in length. A leading `-` is refused so a
    /// free-text slot can never become an option the profile did not declare.
    Text { max_bytes: usize },
}

/// A host-owned execution profile. Nothing here is selectable by a capability
/// bundle or by invocation input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxProfile {
    pub name: String,
    /// Absolute, normalized, non-symlinked path. `PATH` is never consulted.
    pub executable: PathBuf,
    /// `sha256:<64 hex>` of the executable, verified immediately before spawn.
    pub executable_digest: String,
    pub arguments: Vec<ArgumentRule>,
    /// Names copied from the host environment. Everything else is dropped.
    #[serde(default)]
    pub environment: BTreeSet<String>,
}

/// Caller-supplied invocation input. The profile decides what any of it is
/// allowed to be.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SandboxInput {
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    stdin: String,
    /// Relative path under the adapter root. Empty means the root itself.
    #[serde(default)]
    working_directory: String,
}

/// A concrete host adapter that runs pinned executables under an OS sandbox.
///
/// Constructing it is an explicit host action. The authorization policy is
/// derived from the registered profiles, so the profile registry is the single
/// source of truth for what may run.
#[derive(Debug, Clone)]
pub struct SandboxAdapter {
    root: PathBuf,
    profiles: BTreeMap<String, SandboxProfile>,
    policy: PrimitivePolicy,
    confinement: Confinement,
}

impl SandboxAdapter {
    pub fn new(
        root: impl AsRef<Path>,
        bounds: ResourceBounds,
        profiles: Vec<SandboxProfile>,
    ) -> Result<Self, CapabilityError> {
        validate_bounds(&bounds)?;
        let root = std::fs::canonicalize(root.as_ref()).map_err(|error| {
            CapabilityError::Invalid(format!("sandbox root is unavailable: {error}"))
        })?;
        if !root.is_dir() || root.parent().is_none() {
            return Err(CapabilityError::Invalid(
                "sandbox root must be a scoped directory".into(),
            ));
        }
        sbpl_literal(&root)?;

        let mut registry = BTreeMap::new();
        let mut names = BTreeSet::new();
        for profile in profiles {
            validate_profile(&profile)?;
            names.insert(profile.name.clone());
            if registry.insert(profile.name.clone(), profile).is_some() {
                return Err(CapabilityError::Invalid(
                    "sandbox profile names must be unique".into(),
                ));
            }
        }
        Ok(Self {
            root,
            profiles: registry,
            policy: PrimitivePolicy {
                sandbox_profiles: names,
                bounds,
                ..PrimitivePolicy::default()
            },
            confinement: detect_confinement(),
        })
    }

    /// Override the detected confinement layer. Host-owned, and only ever able
    /// to weaken confinement below what the machine supports, never above it.
    pub fn with_confinement(mut self, confinement: Confinement) -> Self {
        if confinement == Confinement::SandboxExec {
            self.confinement = detect_confinement();
        } else {
            self.confinement = confinement;
        }
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn confinement(&self) -> Confinement {
        self.confinement
    }

    /// The immutable server-local policy envelope to pass to capability
    /// authorization. Callers clone this per invocation rather than accepting
    /// a policy from a request.
    pub fn policy(&self) -> &PrimitivePolicy {
        &self.policy
    }

    /// Resolve a caller-declared working directory inside the configured root.
    /// Canonicalization is what defeats a symlink escape: the resolved path,
    /// not the requested one, has to stay under the root.
    fn resolve_working_directory(&self, requested: &str) -> Result<PathBuf, CapabilityError> {
        let path = Path::new(requested);
        if requested.is_empty() {
            return Ok(self.root.clone());
        }
        if path.is_absolute()
            || requested.contains('\0')
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(CapabilityError::Invalid(
                "sandbox working directory must be a relative normalized path".into(),
            ));
        }
        let resolved = std::fs::canonicalize(self.root.join(path)).map_err(|error| {
            CapabilityError::Invalid(format!("sandbox working directory is unavailable: {error}"))
        })?;
        if !resolved.starts_with(&self.root) {
            return Err(CapabilityError::PermissionRequired(
                "sandbox working directory escaped the configured root".into(),
            ));
        }
        if !resolved.is_dir() {
            return Err(CapabilityError::Invalid(
                "sandbox working directory must be a directory".into(),
            ));
        }
        Ok(resolved)
    }
}

impl CapabilityInvocationAdapter for SandboxAdapter {
    fn policy(&self, primitive: &NativePrimitive) -> Option<PrimitivePolicy> {
        matches!(primitive, NativePrimitive::SandboxExecute).then(|| self.policy.clone())
    }

    fn execute(
        &mut self,
        invocation: &AuthorizedPrimitiveInvocation,
    ) -> Result<AdapterExecution, CapabilityError> {
        if invocation.primitive != NativePrimitive::SandboxExecute {
            return Err(CapabilityError::AdapterViolation(
                "sandbox adapter does not support the requested primitive".into(),
            ));
        }
        // Authorizing against the host policy is what enforces the profile
        // allowlist and the declared step bound.
        let receipt = self.policy.authorize(&invocation.request)?;
        let PrimitiveRequest::SandboxExecute { profile, steps } = &invocation.request else {
            return Err(CapabilityError::AdapterViolation(
                "sandbox adapter requires a SandboxExecute request".into(),
            ));
        };
        let profile = self.profiles.get(profile).ok_or_else(|| {
            CapabilityError::PermissionRequired(format!("sandbox profile {profile}"))
        })?;

        let input: SandboxInput = serde_json::from_value(invocation.input.clone())
            .map_err(|error| CapabilityError::Invalid(format!("sandbox input: {error}")))?;
        validate_arguments(&profile.arguments, &input.arguments)?;
        let working_directory = self.resolve_working_directory(&input.working_directory)?;

        // The caller's bounds may only narrow the host envelope.
        let max_bytes = invocation
            .bounds
            .max_bytes
            .min(self.policy.bounds.max_bytes);
        let max_millis = invocation
            .bounds
            .max_millis
            .min(self.policy.bounds.max_millis);
        if max_millis == 0 {
            return Err(CapabilityError::Invalid(
                "sandbox wall-clock bound must be positive".into(),
            ));
        }

        verify_executable(profile)?;
        let outcome = run(
            profile,
            &input,
            &working_directory,
            self.confinement,
            usize::try_from(max_bytes).unwrap_or(usize::MAX),
            max_millis,
        )?;

        let kept = outcome.stdout.len().saturating_add(outcome.stderr.len());
        Ok(AdapterExecution {
            effect: receipt.effect,
            output: serde_json::json!({
                "profile": profile.name,
                "executable": profile.executable.to_string_lossy(),
                "executableDigest": profile.executable_digest,
                "arguments": input.arguments,
                "workingDirectory": working_directory.to_string_lossy(),
                "exitCode": outcome.exit_code,
                "signal": outcome.signal,
                "stdout": String::from_utf8_lossy(&outcome.stdout),
                "stderr": String::from_utf8_lossy(&outcome.stderr),
                "stdoutBytes": outcome.stdout_bytes,
                "stderrBytes": outcome.stderr_bytes,
                "truncated": outcome.truncated,
                "osConfinement": self.confinement.label(),
                "confined": self.confinement == Confinement::SandboxExec,
                "millis": outcome.millis,
            }),
            usage: ResourceUsage {
                bytes: u64::try_from(kept).unwrap_or(u64::MAX),
                steps: *steps,
                millis: outcome.millis,
            },
        })
    }
}

struct Outcome {
    exit_code: Option<i32>,
    signal: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_bytes: u64,
    stderr_bytes: u64,
    truncated: bool,
    millis: u64,
}

fn run(
    profile: &SandboxProfile,
    input: &SandboxInput,
    working_directory: &Path,
    confinement: Confinement,
    max_bytes: usize,
    max_millis: u64,
) -> Result<Outcome, CapabilityError> {
    let mut command = match confinement {
        Confinement::SandboxExec => {
            let mut command = Command::new(SANDBOX_EXEC);
            command
                .arg("-p")
                .arg(sbpl_profile(working_directory)?)
                .arg(&profile.executable);
            command
        }
        Confinement::None => Command::new(&profile.executable),
    };
    command
        .args(&input.arguments)
        .current_dir(working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // An empty environment is the default; the profile opts specific names
    // back in so nothing leaks in by accident.
    command.env_clear();
    for name in &profile.environment {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    // Safety: `setsid` is async-signal-safe and touches no allocator state.
    unsafe {
        command.pre_exec(|| match setsid() {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }

    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|error| CapabilityError::Invalid(format!("sandbox spawn failed: {error}")))?;
    // `setsid` makes the child a group leader, so its pid is its group id.
    let process_group = i32::try_from(child.id()).unwrap_or(-1);

    let stdin_payload = input.stdin.clone().into_bytes();
    let mut handle = child.stdin.take();
    let writer = thread::spawn(move || {
        if let Some(mut handle) = handle.take() {
            // A child that never reads stdin closes the pipe; that is not an
            // error for the run, only for this write.
            let _ = handle.write_all(&stdin_payload);
        }
    });
    let stdout = drain(child.stdout.take(), max_bytes);
    let stderr = drain(child.stderr.take(), max_bytes);

    let deadline = started + Duration::from_millis(max_millis);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => {
                return Err(CapabilityError::Invalid(format!(
                    "sandbox wait failed: {error}"
                )));
            }
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::sleep(POLL_INTERVAL);
    };

    let status = match status {
        Some(status) => status,
        None => {
            // Signal the whole group, then reap, then prove the group is gone.
            // Killing only the child would leave its descendants running.
            unsafe { killpg(process_group, SIGKILL) };
            child.wait().map_err(|error| {
                CapabilityError::Invalid(format!("sandbox reap after cancellation failed: {error}"))
            })?;
            let _ = writer.join();
            let _ = stdout.join();
            let _ = stderr.join();
            // Signal 0 probes for existence without delivering anything.
            if unsafe { killpg(process_group, 0) } == 0 {
                return Err(CapabilityError::AdapterViolation(
                    "sandbox process group survived cancellation".into(),
                ));
            }
            return Err(CapabilityError::Invalid(format!(
                "{TIMEOUT_MESSAGE} after {max_millis}ms"
            )));
        }
    };

    let _ = writer.join();
    let (stdout, stdout_bytes) = stdout.join().unwrap_or_default();
    let (stderr, stderr_bytes) = stderr.join().unwrap_or_default();
    // Deterministic truncation: stdout takes the budget first, stderr gets
    // whatever is left, regardless of how the two streams interleaved.
    let stdout = truncate(stdout, max_bytes);
    let stderr = truncate(stderr, max_bytes - stdout.len());
    let truncated = stdout_bytes > stdout.len() as u64 || stderr_bytes > stderr.len() as u64;

    Ok(Outcome {
        exit_code: status.code(),
        signal: status.signal(),
        stdout,
        stderr,
        stdout_bytes,
        stderr_bytes,
        truncated,
        millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

/// Read a stream to EOF, keeping at most `cap` bytes but counting all of them.
/// Draining past the cap is what keeps the child from blocking on a full pipe
/// while the wall-clock bound is still being measured.
fn drain<R: Read + Send + 'static>(
    source: Option<R>,
    cap: usize,
) -> thread::JoinHandle<(Vec<u8>, u64)> {
    thread::spawn(move || {
        let Some(mut source) = source else {
            return (Vec::new(), 0);
        };
        let mut kept = Vec::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 8192];
        while let Ok(read) = source.read(&mut buffer) {
            if read == 0 {
                break;
            }
            total = total.saturating_add(read as u64);
            if kept.len() < cap {
                let room = cap - kept.len();
                kept.extend_from_slice(&buffer[..read.min(room)]);
            }
        }
        (kept, total)
    })
}

fn truncate(mut bytes: Vec<u8>, limit: usize) -> Vec<u8> {
    bytes.truncate(limit);
    bytes
}

/// `(allow default)` with targeted denials rather than `(deny default)`: a
/// deny-default profile has to enumerate every dyld and system path an
/// arbitrary pinned binary needs, and a profile that broad is easy to get
/// quietly wrong. This confines exactly what is claimed, and nothing more.
fn sbpl_profile(working_directory: &Path) -> Result<String, CapabilityError> {
    let literal = sbpl_literal(working_directory)?;
    Ok(format!(
        "(version 1)\n(allow default)\n(deny network*)\n(deny file-write*)\n(allow file-write* (subpath \"{literal}\"))\n"
    ))
}

fn sbpl_literal(path: &Path) -> Result<&str, CapabilityError> {
    let text = path
        .to_str()
        .ok_or_else(|| CapabilityError::Invalid("sandbox path must be valid UTF-8".into()))?;
    if text.contains('"') || text.contains('\\') || text.chars().any(char::is_control) {
        return Err(CapabilityError::Invalid(
            "sandbox path is not representable in a sandbox profile".into(),
        ));
    }
    Ok(text)
}

fn detect_confinement() -> Confinement {
    match std::fs::metadata(SANDBOX_EXEC) {
        Ok(metadata) if metadata.is_file() => Confinement::SandboxExec,
        _ => Confinement::None,
    }
}

fn validate_arguments(rules: &[ArgumentRule], arguments: &[String]) -> Result<(), CapabilityError> {
    if rules.len() != arguments.len() {
        return Err(CapabilityError::PermissionRequired(format!(
            "sandbox profile declares {} arguments and {} were supplied",
            rules.len(),
            arguments.len()
        )));
    }
    // Errors name the slot but never the value: an argument can carry caller
    // data that must not reach a log or a receipt.
    for (index, (rule, argument)) in rules.iter().zip(arguments).enumerate() {
        let permitted = !argument.contains('\0')
            && match rule {
                ArgumentRule::Literal { value } => argument == value,
                ArgumentRule::OneOf { values } => values.contains(argument),
                ArgumentRule::Text { max_bytes } => {
                    !argument.is_empty()
                        && argument.len() <= *max_bytes
                        && !argument.starts_with('-')
                        && !argument.chars().any(char::is_control)
                }
            };
        if !permitted {
            return Err(CapabilityError::PermissionRequired(format!(
                "sandbox argument {index} is outside the declared schema"
            )));
        }
    }
    Ok(())
}

fn validate_profile(profile: &SandboxProfile) -> Result<(), CapabilityError> {
    if profile.name.is_empty()
        || !profile
            .name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(CapabilityError::Invalid(
            "sandbox profile name must be a portable identifier".into(),
        ));
    }
    if !profile.executable.is_absolute()
        || profile
            .executable
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(CapabilityError::Invalid(
            "sandbox executable must be an absolute normalized path".into(),
        ));
    }
    let hex = profile
        .executable_digest
        .strip_prefix("sha256:")
        .ok_or_else(|| {
            CapabilityError::Invalid("sandbox executable digest must be sha256".into())
        })?;
    if hex.len() != 64 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(CapabilityError::Invalid(
            "sandbox executable digest must be 64 hex characters".into(),
        ));
    }
    for name in &profile.environment {
        if name.is_empty() || name.contains('=') || name.contains('\0') {
            return Err(CapabilityError::Invalid(
                "sandbox environment allowlist entry is not a variable name".into(),
            ));
        }
    }
    Ok(())
}

/// Verify identity immediately before spawn. A symlinked or relative path is
/// refused outright so the digest describes the exact file that will run. A
/// replace between this check and `exec` remains possible; closing that fully
/// needs an fd-based exec the standard library does not expose.
fn verify_executable(profile: &SandboxProfile) -> Result<(), CapabilityError> {
    let metadata = std::fs::symlink_metadata(&profile.executable).map_err(|error| {
        CapabilityError::Invalid(format!("sandbox executable is unavailable: {error}"))
    })?;
    if !metadata.is_file() {
        return Err(CapabilityError::Invalid(
            "sandbox executable must be a regular file".into(),
        ));
    }
    let canonical = std::fs::canonicalize(&profile.executable).map_err(|error| {
        CapabilityError::Invalid(format!(
            "sandbox executable canonicalization failed: {error}"
        ))
    })?;
    if canonical != profile.executable {
        return Err(CapabilityError::Invalid(
            "sandbox executable path must already be canonical".into(),
        ));
    }
    let actual = digest_file(&profile.executable)?;
    if actual != profile.executable_digest {
        return Err(CapabilityError::PermissionRequired(format!(
            "sandbox executable digest mismatch for {}",
            profile.executable.display()
        )));
    }
    Ok(())
}

/// SHA-256 of a file as `sha256:<hex>`, matching the digest form profiles use.
pub fn digest_file(path: impl AsRef<Path>) -> Result<String, CapabilityError> {
    let mut file = std::fs::File::open(path.as_ref()).map_err(|error| {
        CapabilityError::Invalid(format!("sandbox digest read failed: {error}"))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            CapabilityError::Invalid(format!("sandbox digest read failed: {error}"))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut hex = String::from("sha256:");
    for byte in hasher.finalize() {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

fn validate_bounds(bounds: &ResourceBounds) -> Result<(), CapabilityError> {
    if bounds.max_bytes == 0
        || bounds.max_millis == 0
        || bounds.max_bytes > MAX_RESOURCE_BYTES
        || bounds.max_steps > MAX_RESOURCE_STEPS
        || bounds.max_millis > MAX_RESOURCE_MILLIS
    {
        return Err(CapabilityError::Invalid(
            "sandbox resource bounds are outside the supported envelope".into(),
        ));
    }
    Ok(())
}

/// Build the invocation input a caller passes to the adapter.
pub fn sandbox_input(arguments: &[&str], stdin: &str, working_directory: &str) -> Value {
    serde_json::json!({
        "arguments": arguments,
        "stdin": stdin,
        "workingDirectory": working_directory,
    })
}
