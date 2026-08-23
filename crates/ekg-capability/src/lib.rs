//! Policy-enforced capability acquisition and portable bundle exchange.
//!
//! This crate intentionally models the native substrate without performing
//! ambient I/O. Discovery produces typed candidates, sandbox validation uses
//! supplied fixtures, and invocation requires an explicit local grant.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const BUNDLE_FORMAT_VERSION: u16 = 1;
pub const MAX_PROCEDURES: usize = 64;
pub const MAX_DEPENDENCIES: usize = 128;
pub const MAX_DEPENDENCY_EDGES: usize = 512;
pub const MAX_TESTS: usize = 256;
pub const MAX_COMPATIBILITY_CONSTRAINTS: usize = 32;
pub const MAX_SCHEMA_BYTES: usize = 32 * 1024;
pub const MAX_BUNDLE_BYTES: usize = 512 * 1024;
pub const MAX_RESOURCE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RESOURCE_STEPS: u64 = 1_000_000;
pub const MAX_RESOURCE_MILLIS: u64 = 60_000;

#[derive(Debug, Error)]
pub enum CapabilityError {
    #[error("invalid capability: {0}")]
    Invalid(String),
    #[error("bundle is not reconstructible: {0}")]
    InvalidBundle(String),
    #[error("capability is not locally revalidated")]
    NotRevalidated,
    #[error("required local permission is not granted: {0}")]
    PermissionRequired(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Permission {
    NetworkHost { host: String },
    FileReadPrefix { path_prefix: String },
    FileWritePrefix { path_prefix: String },
    ObserveTarget { target: String },
    SandboxProfile { profile: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Network,
    FileRead,
    FileWrite,
    Observation,
    SandboxedExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePrimitive {
    NetworkRequest,
    FileRead,
    FileWrite,
    Observe,
    SandboxExecute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceBounds {
    pub max_bytes: u64,
    pub max_steps: u64,
    pub max_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrimitiveRequest {
    Network {
        host: String,
        method: String,
        body_bytes: u64,
    },
    FileRead {
        path: String,
        bytes: u64,
    },
    FileWrite {
        path: String,
        bytes: u64,
    },
    Observe {
        target: String,
    },
    SandboxExecute {
        profile: String,
        steps: u64,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimitivePolicy {
    pub network_hosts: BTreeSet<String>,
    pub file_read_prefixes: BTreeSet<String>,
    pub file_write_prefixes: BTreeSet<String>,
    pub observe_targets: BTreeSet<String>,
    pub sandbox_profiles: BTreeSet<String>,
    pub bounds: ResourceBounds,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvocationReceipt {
    pub primitive: NativePrimitive,
    pub effect: Effect,
    pub target: String,
    pub permission: Permission,
    pub payload_digest: String,
    pub bounds: ResourceBounds,
    /// The receipt stores a digest rather than the request payload.
    pub redacted: bool,
    /// Whether the primitive contract permits deterministic replay of the
    /// invocation without relying on ambient external state.
    pub replayable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimitiveExecution {
    pub receipt: InvocationReceipt,
    pub output: Value,
}

/// Safe native observations. Every target must be present in the local
/// policy, and only explicitly supported targets are implemented.
pub struct NativePrimitiveExecutor {
    policy: PrimitivePolicy,
}

impl NativePrimitiveExecutor {
    pub fn new(policy: PrimitivePolicy) -> Self {
        Self { policy }
    }

    pub fn observe(
        &self,
        request: &PrimitiveRequest,
    ) -> Result<PrimitiveExecution, CapabilityError> {
        let receipt = self.policy.authorize(request)?;
        let PrimitiveRequest::Observe { target } = request else {
            return Err(CapabilityError::Invalid(
                "native observation executor requires an Observe request".into(),
            ));
        };
        let output = match target.as_str() {
            "clock" => serde_json::json!({
                "unixSeconds": unix_time(),
                "source": "native:clock",
            }),
            _ => {
                return Err(CapabilityError::Invalid(format!(
                    "observation target {target} has no local adapter"
                )));
            }
        };
        Ok(PrimitiveExecution { receipt, output })
    }

    /// Execute a policy-authorized network request through an explicitly
    /// injected host adapter. The capability layer never opens an ambient
    /// socket; the host owns transport, TLS, credential handling, and any
    /// additional egress policy. The adapter receives only the authorized
    /// host/method and bounded request value.
    pub fn network_request<F>(
        &self,
        request: &PrimitiveRequest,
        body: &Value,
        adapter: F,
    ) -> Result<PrimitiveExecution, CapabilityError>
    where
        F: FnOnce(&str, &str, &Value) -> Result<Value, CapabilityError>,
    {
        let receipt = self.policy.authorize(request)?;
        let PrimitiveRequest::Network {
            host,
            method,
            body_bytes,
        } = request
        else {
            return Err(CapabilityError::Invalid(
                "network executor requires a Network request".into(),
            ));
        };
        let body_bytes_actual = canonical_json(body)?.len() as u64;
        enforce_bytes(body_bytes_actual, *body_bytes, &self.policy.bounds)?;
        let output = adapter(host, method, body)?;
        let output_bytes = canonical_json(&output)?.len() as u64;
        if output_bytes > self.policy.bounds.max_bytes {
            return Err(CapabilityError::Invalid(
                "network response exceeds resource byte bound".into(),
            ));
        }
        Ok(PrimitiveExecution { receipt, output })
    }

    pub fn read_file(
        &self,
        request: &PrimitiveRequest,
    ) -> Result<PrimitiveExecution, CapabilityError> {
        let receipt = self.policy.authorize(request)?;
        let PrimitiveRequest::FileRead { path, bytes } = request else {
            return Err(CapabilityError::Invalid(
                "file reader requires a FileRead request".into(),
            ));
        };
        let file = std::fs::symlink_metadata(path)
            .map_err(|error| CapabilityError::Invalid(format!("file metadata failed: {error}")))?;
        if file.file_type().is_symlink() {
            return Err(CapabilityError::Invalid(
                "symlink file targets are not allowed".into(),
            ));
        }
        let resolved = std::fs::canonicalize(path).map_err(|error| {
            CapabilityError::Invalid(format!("file canonicalization failed: {error}"))
        })?;
        let prefix = receipt_permission_path(&receipt.permission)?;
        let resolved_prefix = std::fs::canonicalize(prefix).map_err(|error| {
            CapabilityError::Invalid(format!("file permission scope is unavailable: {error}"))
        })?;
        if !resolved.starts_with(resolved_prefix) {
            return Err(CapabilityError::PermissionRequired(
                "resolved file path escaped its permission scope".into(),
            ));
        }
        let bytes_out = std::fs::read(&resolved)
            .map_err(|error| CapabilityError::Invalid(format!("file read failed: {error}")))?;
        enforce_bytes(bytes_out.len() as u64, *bytes, &self.policy.bounds)?;
        Ok(PrimitiveExecution {
            receipt,
            output: serde_json::json!({"bytes": bytes_out}),
        })
    }

    pub fn write_file(
        &self,
        request: &PrimitiveRequest,
        payload: &Value,
    ) -> Result<PrimitiveExecution, CapabilityError> {
        let receipt = self.policy.authorize(request)?;
        let PrimitiveRequest::FileWrite { path, bytes } = request else {
            return Err(CapabilityError::Invalid(
                "file writer requires a FileWrite request".into(),
            ));
        };
        let payload = payload_bytes(payload)?;
        enforce_bytes(payload.len() as u64, *bytes, &self.policy.bounds)?;
        if let Ok(file) = std::fs::symlink_metadata(path)
            && file.file_type().is_symlink()
        {
            return Err(CapabilityError::Invalid(
                "symlink file targets are not allowed".into(),
            ));
        }
        let parent = Path::new(path).parent().ok_or_else(|| {
            CapabilityError::Invalid("file write target has no parent directory".into())
        })?;
        let resolved_parent = std::fs::canonicalize(parent).map_err(|error| {
            CapabilityError::Invalid(format!("file parent canonicalization failed: {error}"))
        })?;
        let prefix = receipt_permission_path(&receipt.permission)?;
        let resolved_prefix = std::fs::canonicalize(prefix).map_err(|error| {
            CapabilityError::Invalid(format!("file permission scope is unavailable: {error}"))
        })?;
        if !resolved_parent.starts_with(resolved_prefix) {
            return Err(CapabilityError::PermissionRequired(
                "resolved file parent escaped its permission scope".into(),
            ));
        }
        std::fs::write(path, &payload)
            .map_err(|error| CapabilityError::Invalid(format!("file write failed: {error}")))?;
        Ok(PrimitiveExecution {
            receipt,
            output: serde_json::json!({"bytesWritten": payload.len()}),
        })
    }

    /// Bounded fixture execution. It deliberately does not spawn a process or
    /// interpret bundle-controlled code; a real sandbox adapter must be
    /// injected by the host and can consume this receipt boundary.
    pub fn sandbox_fixture(
        &self,
        request: &PrimitiveRequest,
        input: &Value,
    ) -> Result<PrimitiveExecution, CapabilityError> {
        let receipt = self.policy.authorize(request)?;
        let PrimitiveRequest::SandboxExecute { profile, steps } = request else {
            return Err(CapabilityError::Invalid(
                "sandbox executor requires a SandboxExecute request".into(),
            ));
        };
        let input_bytes = canonical_json(input)?;
        enforce_bytes(
            input_bytes.len() as u64,
            self.policy.bounds.max_bytes,
            &self.policy.bounds,
        )?;
        Ok(PrimitiveExecution {
            receipt,
            output: serde_json::json!({
                "sandboxed": true,
                "profile": profile,
                "steps": steps,
                "input": input,
                "inputDigest": digest_json(input)?
            }),
        })
    }
}

fn receipt_permission_path(permission: &Permission) -> Result<&str, CapabilityError> {
    match permission {
        Permission::FileReadPrefix { path_prefix }
        | Permission::FileWritePrefix { path_prefix } => Ok(path_prefix),
        _ => Err(CapabilityError::Invalid(
            "receipt is not bound to a file permission".into(),
        )),
    }
}

fn payload_bytes(payload: &Value) -> Result<Vec<u8>, CapabilityError> {
    match payload {
        Value::String(value) => Ok(value.as_bytes().to_vec()),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or_else(|| {
                        CapabilityError::Invalid("file payload must contain bytes".into())
                    })
            })
            .collect(),
        _ => Err(CapabilityError::Invalid(
            "file payload must be a string or byte array".into(),
        )),
    }
}

fn enforce_bytes(
    actual: u64,
    declared: u64,
    bounds: &ResourceBounds,
) -> Result<(), CapabilityError> {
    if actual > declared || actual > bounds.max_bytes {
        return Err(CapabilityError::Invalid(
            "resource byte bound exceeded".into(),
        ));
    }
    Ok(())
}

impl PrimitivePolicy {
    pub fn authorize(
        &self,
        request: &PrimitiveRequest,
    ) -> Result<InvocationReceipt, CapabilityError> {
        let (primitive, effect, target, permission, amount) = match request {
            PrimitiveRequest::Network {
                host,
                method,
                body_bytes,
            } => {
                if method.trim().is_empty() || !self.network_hosts.contains(host) {
                    return Err(CapabilityError::PermissionRequired(format!(
                        "network host {host}"
                    )));
                }
                (
                    NativePrimitive::NetworkRequest,
                    Effect::Network,
                    host.clone(),
                    Permission::NetworkHost { host: host.clone() },
                    *body_bytes,
                )
            }
            PrimitiveRequest::FileRead { path, bytes } => {
                let path_prefix = matching_path_prefix(path, &self.file_read_prefixes)?
                    .ok_or_else(|| {
                        CapabilityError::PermissionRequired(format!("file read {path}"))
                    })?;
                (
                    NativePrimitive::FileRead,
                    Effect::FileRead,
                    path.clone(),
                    Permission::FileReadPrefix { path_prefix },
                    *bytes,
                )
            }
            PrimitiveRequest::FileWrite { path, bytes } => {
                let path_prefix = matching_path_prefix(path, &self.file_write_prefixes)?
                    .ok_or_else(|| {
                        CapabilityError::PermissionRequired(format!("file write {path}"))
                    })?;
                (
                    NativePrimitive::FileWrite,
                    Effect::FileWrite,
                    path.clone(),
                    Permission::FileWritePrefix { path_prefix },
                    *bytes,
                )
            }
            PrimitiveRequest::Observe { target } => {
                if !self.observe_targets.contains(target) {
                    return Err(CapabilityError::PermissionRequired(format!(
                        "observation target {target}"
                    )));
                }
                (
                    NativePrimitive::Observe,
                    Effect::Observation,
                    target.clone(),
                    Permission::ObserveTarget {
                        target: target.clone(),
                    },
                    0,
                )
            }
            PrimitiveRequest::SandboxExecute { profile, steps } => {
                if !self.sandbox_profiles.contains(profile) {
                    return Err(CapabilityError::PermissionRequired(format!(
                        "sandbox profile {profile}"
                    )));
                }
                if *steps > self.bounds.max_steps {
                    return Err(CapabilityError::Invalid(
                        "sandbox step bound exceeded".into(),
                    ));
                }
                (
                    NativePrimitive::SandboxExecute,
                    Effect::SandboxedExecution,
                    profile.clone(),
                    Permission::SandboxProfile {
                        profile: profile.clone(),
                    },
                    *steps,
                )
            }
        };
        if amount > self.bounds.max_bytes {
            return Err(CapabilityError::Invalid(
                "primitive byte bound exceeded".into(),
            ));
        }
        let payload_digest = digest_json(request)?;
        let replayable = matches!(
            primitive,
            NativePrimitive::FileRead | NativePrimitive::SandboxExecute
        );
        Ok(InvocationReceipt {
            primitive,
            effect,
            target,
            permission,
            payload_digest,
            bounds: self.bounds.clone(),
            redacted: true,
            replayable,
        })
    }
}

fn matching_path_prefix(
    path: &str,
    prefixes: &BTreeSet<String>,
) -> Result<Option<String>, CapabilityError> {
    let candidate = Path::new(path);
    validate_absolute_path(candidate, "requested file path")?;
    let mut matches = Vec::new();
    for prefix in prefixes {
        let prefix_path = Path::new(prefix);
        validate_absolute_path(prefix_path, "file permission prefix")?;
        if prefix_path.parent().is_none() {
            return Err(CapabilityError::Invalid(
                "filesystem root cannot be used as a scoped file prefix".into(),
            ));
        }
        if candidate.starts_with(prefix_path) {
            matches.push(prefix.clone());
        }
    }
    Ok(matches.into_iter().max_by_key(|prefix| prefix.len()))
}

fn validate_absolute_path(path: &Path, label: &str) -> Result<(), CapabilityError> {
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || path.to_string_lossy().contains('\0')
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(CapabilityError::Invalid(format!(
            "{label} must be an absolute normalized path"
        )));
    }
    Ok(())
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, CapabilityError> {
    let bytes = canonical_json(value)?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("sha256:{}", hex_bytes(&digest.finalize())))
}

impl Default for ResourceBounds {
    fn default() -> Self {
        Self {
            max_bytes: 1_048_576,
            max_steps: 100_000,
            max_millis: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityTest {
    pub name: String,
    pub input: Value,
    pub expected_output: Value,
    pub fixture_output: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    pub source: String,
    pub discovered_at: i64,
    pub interface_fingerprint: String,
    pub validation_episodes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    pub name: String,
    pub version: String,
    pub content_hash: String,
    /// Direct prerequisite nodes, pinned by the same identity used in the
    /// bundle closure. These references form a finite acyclic graph.
    pub dependencies: Vec<DependencyReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd)]
#[serde(rename_all = "camelCase")]
pub struct DependencyReference {
    pub name: String,
    pub version: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeutralProcedureKind {
    NativePrimitive,
}

/// Portable metadata for a procedure. This is deliberately not executable
/// code: the receiving runtime maps this neutral IR to its own native adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NeutralProcedureMetadata {
    pub kind: NeutralProcedureKind,
    pub ir_version: u16,
    pub fixture_format: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconstructionRecipe {
    pub kind: String,
    pub recipe_version: u16,
    /// Portable platform/runtime constraints, never host-local settings.
    pub compatibility: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Quarantined,
    Provisional,
    Active,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityProcedure {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub primitive: NativePrimitive,
    pub input_schema: Value,
    pub output_schema: Value,
    pub contract: Value,
    pub neutral_metadata: NeutralProcedureMetadata,
    pub permissions: Vec<Permission>,
    pub effects: Vec<Effect>,
    pub bounds: ResourceBounds,
    pub dependencies: Vec<DependencyReference>,
    pub tests: Vec<CapabilityTest>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityBundle {
    pub format_version: u16,
    pub name: String,
    pub version: String,
    pub content_id: String,
    pub procedures: Vec<CapabilityProcedure>,
    pub dependencies: Vec<Dependency>,
    pub provenance: Provenance,
    pub reconstruction: ReconstructionRecipe,
}

/// The locally reconstructed, still-untrusted capability shape. It contains
/// only neutral procedure data, fixtures, and a deterministic dependency order;
/// it never contains local grants or executable foreign code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconstructedCapability {
    pub content_id: String,
    pub name: String,
    pub dependency_order: Vec<Dependency>,
    pub procedures: Vec<CapabilityProcedure>,
    pub reconstruction: ReconstructionRecipe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedCapability {
    pub content_id: String,
    pub name: String,
    pub status: CapabilityStatus,
    pub locally_validated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalValidation {
    pub passed: bool,
    pub validation_episodes: Vec<String>,
    pub environment_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredOperation {
    pub name: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub host: String,
    pub method: String,
    pub response_fixture: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceDescription {
    pub source: String,
    pub fingerprint: String,
    pub operations: Vec<DiscoveredOperation>,
}

pub fn discover_interface(
    description: &InterfaceDescription,
) -> Result<CapabilityBundle, CapabilityError> {
    if description.source.trim().is_empty() || description.fingerprint.trim().is_empty() {
        return Err(CapabilityError::Invalid(
            "interface provenance is required".into(),
        ));
    }
    if description.operations.is_empty() || description.operations.len() > MAX_PROCEDURES {
        return Err(CapabilityError::Invalid(
            "interface operation count is out of bounds".into(),
        ));
    }
    let procedures = description
        .operations
        .iter()
        .map(|operation| {
            validate_schema(&operation.input_schema)?;
            validate_schema(&operation.output_schema)?;
            let primitive = NativePrimitive::NetworkRequest;
            Ok(CapabilityProcedure {
                id: format!("{}:{}", description.fingerprint, operation.name),
                name: operation.name.clone(),
                version: 1,
                primitive,
                input_schema: operation.input_schema.clone(),
                output_schema: operation.output_schema.clone(),
                contract: serde_json::json!({
                    "method": operation.method,
                    "host": operation.host,
                    "replayable": true
                }),
                neutral_metadata: NeutralProcedureMetadata {
                    kind: NeutralProcedureKind::NativePrimitive,
                    ir_version: 1,
                    fixture_format: "json".into(),
                },
                permissions: vec![Permission::NetworkHost {
                    host: operation.host.clone(),
                }],
                effects: vec![Effect::Network],
                bounds: ResourceBounds::default(),
                dependencies: Vec::new(),
                tests: vec![CapabilityTest {
                    name: format!("{} fixture", operation.name),
                    input: serde_json::json!({}),
                    expected_output: operation.response_fixture.clone(),
                    fixture_output: operation.response_fixture.clone(),
                }],
                provenance: Provenance {
                    source: description.source.clone(),
                    discovered_at: unix_time(),
                    interface_fingerprint: description.fingerprint.clone(),
                    validation_episodes: Vec::new(),
                },
            })
        })
        .collect::<Result<Vec<_>, CapabilityError>>()?;
    let mut bundle = CapabilityBundle {
        format_version: BUNDLE_FORMAT_VERSION,
        name: description.source.clone(),
        version: "0.1.0".into(),
        content_id: String::new(),
        procedures,
        dependencies: Vec::new(),
        provenance: Provenance {
            source: description.source.clone(),
            discovered_at: unix_time(),
            interface_fingerprint: description.fingerprint.clone(),
            validation_episodes: Vec::new(),
        },
        reconstruction: ReconstructionRecipe {
            kind: "native_primitive_procedure".into(),
            recipe_version: 1,
            compatibility: vec!["ekg-capability-neutral-ir-v1".into()],
        },
    };
    bundle.content_id = bundle_content_id(&bundle)?;
    validate_bundle(&bundle)?;
    Ok(bundle)
}

pub fn run_sandbox_tests(procedure: &CapabilityProcedure) -> Result<(), CapabilityError> {
    if procedure.tests.is_empty() {
        return Err(CapabilityError::Invalid(
            "capability needs at least one sandbox test".into(),
        ));
    }
    for test in &procedure.tests {
        if test.fixture_output != test.expected_output {
            return Err(CapabilityError::Invalid(format!(
                "sandbox test '{}' failed",
                test.name
            )));
        }
    }
    Ok(())
}

pub fn bundle_content_id(bundle: &CapabilityBundle) -> Result<String, CapabilityError> {
    let mut canonical = bundle.clone();
    canonical.content_id.clear();
    let bytes = canonical_json(&canonical)?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("sha256:{}", hex_bytes(&digest.finalize())))
}

pub fn export_bundle(bundle: &CapabilityBundle) -> Result<Vec<u8>, CapabilityError> {
    validate_bundle(bundle)?;
    if bundle.content_id != bundle_content_id(bundle)? {
        return Err(CapabilityError::InvalidBundle(
            "content identity mismatch".into(),
        ));
    }
    let bytes = canonical_json(bundle)?;
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(CapabilityError::InvalidBundle(
            "bundle exceeds byte bound".into(),
        ));
    }
    Ok(bytes)
}

pub fn import_bundle(bytes: &[u8]) -> Result<CapabilityBundle, CapabilityError> {
    if bytes.is_empty() || bytes.len() > MAX_BUNDLE_BYTES {
        return Err(CapabilityError::InvalidBundle(
            "bundle bytes are out of bounds".into(),
        ));
    }
    let bundle: CapabilityBundle = serde_json::from_slice(bytes)?;
    validate_bundle(&bundle)?;
    if bundle.content_id != bundle_content_id(&bundle)? {
        return Err(CapabilityError::InvalidBundle(
            "content identity mismatch".into(),
        ));
    }
    if bytes != export_bundle(&bundle)? {
        return Err(CapabilityError::InvalidBundle(
            "bundle encoding is not canonical".into(),
        ));
    }
    Ok(bundle)
}

/// Verify that a portable bundle can be rebuilt by this runtime without
/// interpreting foreign code. The returned dependency order has every direct
/// prerequisite before its dependents, so a clean store can acquire adapters
/// and fixtures deterministically before exercising procedures.
pub fn reconstruct_bundle(
    bundle: &CapabilityBundle,
) -> Result<ReconstructedCapability, CapabilityError> {
    validate_bundle(bundle)?;
    if bundle.content_id != bundle_content_id(bundle)? {
        return Err(CapabilityError::InvalidBundle(
            "content identity mismatch".into(),
        ));
    }
    Ok(ReconstructedCapability {
        content_id: bundle.content_id.clone(),
        name: bundle.name.clone(),
        dependency_order: dependency_reconstruction_order(&bundle.dependencies)?,
        procedures: bundle.procedures.clone(),
        reconstruction: bundle.reconstruction.clone(),
    })
}

pub fn validate_bundle(bundle: &CapabilityBundle) -> Result<(), CapabilityError> {
    if bundle.format_version != BUNDLE_FORMAT_VERSION
        || bundle.name.trim().is_empty()
        || bundle.version.trim().is_empty()
        || bundle.procedures.is_empty()
        || bundle.procedures.len() > MAX_PROCEDURES
        || bundle.dependencies.len() > MAX_DEPENDENCIES
    {
        return Err(CapabilityError::InvalidBundle(
            "manifest or count bounds failed".into(),
        ));
    }
    if !is_sha256_digest(&bundle.content_id)
        || bundle.provenance.source.trim().is_empty()
        || bundle.provenance.interface_fingerprint.trim().is_empty()
    {
        return Err(CapabilityError::InvalidBundle(
            "bundle identity or provenance is invalid".into(),
        ));
    }
    reject_secrets(bundle)?;
    reject_local_authority(bundle)?;
    let mut dependency_names = BTreeSet::new();
    let mut dependency_edges = 0usize;
    for dependency in &bundle.dependencies {
        if dependency.name.trim().is_empty()
            || dependency.version.trim().is_empty()
            || !is_sha256_digest(&dependency.content_hash)
            || !dependency_names.insert(dependency.name.clone())
        {
            return Err(CapabilityError::InvalidBundle(
                "dependency identity or closure is invalid".into(),
            ));
        }
        dependency_edges = dependency_edges.saturating_add(dependency.dependencies.len());
        if dependency_edges > MAX_DEPENDENCY_EDGES {
            return Err(CapabilityError::InvalidBundle(
                "dependency graph exceeds edge bound".into(),
            ));
        }
    }
    for dependency in &bundle.dependencies {
        let mut direct_dependencies = BTreeSet::new();
        for reference in &dependency.dependencies {
            let identity = dependency_identity(reference);
            if !direct_dependencies.insert(identity)
                || dependency.name == reference.name
                || !bundle
                    .dependencies
                    .iter()
                    .any(|candidate| dependency_matches_reference(candidate, reference))
            {
                return Err(CapabilityError::InvalidBundle(
                    "dependency graph closure is invalid".into(),
                ));
            }
        }
    }
    dependency_reconstruction_order(&bundle.dependencies)?;
    let mut ids = BTreeSet::new();
    for procedure in &bundle.procedures {
        if procedure.id.trim().is_empty()
            || procedure.name.trim().is_empty()
            || procedure.version == 0
            || !ids.insert(procedure.id.clone())
            || procedure.tests.is_empty()
            || procedure.tests.len() > MAX_TESTS
            || procedure.dependencies.len() > MAX_DEPENDENCIES
        {
            return Err(CapabilityError::InvalidBundle(
                "procedure identity or count bounds failed".into(),
            ));
        }
        validate_schema(&procedure.input_schema)?;
        validate_schema(&procedure.output_schema)?;
        if procedure.permissions.is_empty() || procedure.effects.is_empty() {
            return Err(CapabilityError::InvalidBundle(
                "procedures must declare permissions and effects".into(),
            ));
        }
        validate_resource_bounds(&procedure.bounds)?;
        validate_neutral_metadata(&procedure.neutral_metadata)?;
        validate_primitive_declarations(procedure)?;
        for test in &procedure.tests {
            if test.name.trim().is_empty() {
                return Err(CapabilityError::InvalidBundle(
                    "sandbox test name is required".into(),
                ));
            }
        }
        let mut procedure_dependencies = BTreeSet::new();
        for dependency in &procedure.dependencies {
            if !procedure_dependencies.insert(dependency_identity(dependency))
                || !bundle
                    .dependencies
                    .iter()
                    .any(|candidate| dependency_matches_reference(candidate, dependency))
            {
                return Err(CapabilityError::InvalidBundle(
                    "procedure dependency is missing from bundle closure".into(),
                ));
            }
        }
    }
    validate_reconstruction_recipe(&bundle.reconstruction)?;
    Ok(())
}

fn dependency_identity(reference: &DependencyReference) -> (String, String, String) {
    (
        reference.name.clone(),
        reference.version.clone(),
        reference.content_hash.clone(),
    )
}

fn dependency_matches_reference(dependency: &Dependency, reference: &DependencyReference) -> bool {
    dependency.name == reference.name
        && dependency.version == reference.version
        && dependency.content_hash == reference.content_hash
}

fn dependency_reconstruction_order(
    dependencies: &[Dependency],
) -> Result<Vec<Dependency>, CapabilityError> {
    fn visit(
        name: &str,
        dependencies: &[Dependency],
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
        ordered: &mut Vec<Dependency>,
    ) -> Result<(), CapabilityError> {
        if visited.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name.into()) {
            return Err(CapabilityError::InvalidBundle(
                "dependency graph contains a cycle".into(),
            ));
        }
        let dependency = dependencies
            .iter()
            .find(|dependency| dependency.name == name)
            .ok_or_else(|| {
                CapabilityError::InvalidBundle("dependency graph is incomplete".into())
            })?;
        let mut children = dependency.dependencies.clone();
        children.sort();
        for child in children {
            visit(&child.name, dependencies, visiting, visited, ordered)?;
        }
        visiting.remove(name);
        visited.insert(name.into());
        ordered.push(dependency.clone());
        Ok(())
    }

    let mut names = dependencies
        .iter()
        .map(|dependency| dependency.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut ordered = Vec::with_capacity(dependencies.len());
    for name in names {
        visit(
            &name,
            dependencies,
            &mut visiting,
            &mut visited,
            &mut ordered,
        )?;
    }
    Ok(ordered)
}

fn validate_neutral_metadata(metadata: &NeutralProcedureMetadata) -> Result<(), CapabilityError> {
    if metadata.kind != NeutralProcedureKind::NativePrimitive
        || metadata.ir_version != 1
        || metadata.fixture_format != "json"
    {
        return Err(CapabilityError::InvalidBundle(
            "procedure neutral metadata is unsupported".into(),
        ));
    }
    Ok(())
}

fn validate_reconstruction_recipe(recipe: &ReconstructionRecipe) -> Result<(), CapabilityError> {
    if recipe.kind != "native_primitive_procedure"
        || recipe.recipe_version != 1
        || recipe.compatibility.is_empty()
        || recipe.compatibility.len() > MAX_COMPATIBILITY_CONSTRAINTS
        || recipe.compatibility.iter().any(|constraint| {
            constraint.trim().is_empty() || constraint.chars().any(char::is_control)
        })
    {
        return Err(CapabilityError::InvalidBundle(
            "reconstruction recipe or compatibility constraints are invalid".into(),
        ));
    }
    Ok(())
}

fn validate_resource_bounds(bounds: &ResourceBounds) -> Result<(), CapabilityError> {
    if bounds.max_bytes == 0
        || bounds.max_bytes > MAX_RESOURCE_BYTES
        || bounds.max_steps == 0
        || bounds.max_steps > MAX_RESOURCE_STEPS
        || bounds.max_millis == 0
        || bounds.max_millis > MAX_RESOURCE_MILLIS
    {
        return Err(CapabilityError::InvalidBundle(
            "procedure resource bounds are invalid".into(),
        ));
    }
    Ok(())
}

fn validate_primitive_declarations(procedure: &CapabilityProcedure) -> Result<(), CapabilityError> {
    let expected_effect = match procedure.primitive {
        NativePrimitive::NetworkRequest => Effect::Network,
        NativePrimitive::FileRead => Effect::FileRead,
        NativePrimitive::FileWrite => Effect::FileWrite,
        NativePrimitive::Observe => Effect::Observation,
        NativePrimitive::SandboxExecute => Effect::SandboxedExecution,
    };
    if procedure.effects.as_slice() != [expected_effect] {
        return Err(CapabilityError::InvalidBundle(
            "primitive effect declaration is inconsistent".into(),
        ));
    }
    let permissions_match = procedure.permissions.iter().all(|permission| {
        matches!(
            (&procedure.primitive, permission),
            (
                NativePrimitive::NetworkRequest,
                Permission::NetworkHost { .. }
            ) | (NativePrimitive::FileRead, Permission::FileReadPrefix { .. })
                | (
                    NativePrimitive::FileWrite,
                    Permission::FileWritePrefix { .. }
                )
                | (NativePrimitive::Observe, Permission::ObserveTarget { .. })
                | (
                    NativePrimitive::SandboxExecute,
                    Permission::SandboxProfile { .. }
                )
        )
    });
    if !permissions_match {
        return Err(CapabilityError::InvalidBundle(
            "primitive permission declaration is inconsistent".into(),
        ));
    }
    for permission in &procedure.permissions {
        let value = match permission {
            Permission::NetworkHost { host } => host,
            Permission::FileReadPrefix { path_prefix }
            | Permission::FileWritePrefix { path_prefix } => path_prefix,
            Permission::ObserveTarget { target } => target,
            Permission::SandboxProfile { profile } => profile,
        };
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(CapabilityError::InvalidBundle(
                "permission scope is invalid".into(),
            ));
        }
    }
    Ok(())
}

fn is_sha256_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn validate_schema(schema: &Value) -> Result<(), CapabilityError> {
    let bytes = serde_json::to_vec(schema)?;
    if bytes.len() > MAX_SCHEMA_BYTES {
        return Err(CapabilityError::InvalidBundle(
            "schema exceeds byte bound".into(),
        ));
    }
    Ok(())
}

fn reject_secrets(value: &impl Serialize) -> Result<(), CapabilityError> {
    let json = serde_json::to_value(value)?;
    let mut keys = Vec::new();
    let mut strings = Vec::new();
    collect_material(&json, &mut keys, &mut strings);
    if keys.iter().any(|key| {
        [
            "secret",
            "token",
            "password",
            "cookie",
            "api_key",
            "authorization",
        ]
        .iter()
        .any(|needle| key.contains(needle))
    }) || strings.iter().any(|value| contains_secret_value(value))
    {
        return Err(CapabilityError::InvalidBundle(
            "secret-bearing field or value is prohibited".into(),
        ));
    }
    Ok(())
}

fn reject_local_authority(value: &impl Serialize) -> Result<(), CapabilityError> {
    let json = serde_json::to_value(value)?;
    let mut keys = Vec::new();
    let mut strings = Vec::new();
    collect_material(&json, &mut keys, &mut strings);
    if keys.iter().any(|key| {
        [
            "trust_receipt",
            "grant",
            "promoted",
            "ambient_secret",
            "local_lifecycle",
        ]
        .iter()
        .any(|needle| key.contains(needle))
    }) || strings
        .iter()
        .any(|value| contains_machine_local_material(value))
    {
        return Err(CapabilityError::InvalidBundle(
            "local authority or environment-specific path is prohibited".into(),
        ));
    }
    Ok(())
}

fn contains_secret_value(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let credential_header = lower.starts_with("bearer ") || lower.starts_with("basic ");
    credential_header
        || lower.contains("-----begin private key-----")
        || lower.contains("-----begin rsa private key-----")
        || lower.contains("-----begin ec private key-----")
}

fn contains_machine_local_material(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let bytes = trimmed.as_bytes();
    let windows_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    trimmed.starts_with('/')
        || trimmed.starts_with("~/")
        || trimmed.starts_with("\\\\")
        || windows_absolute
        || lower.starts_with("file://")
        || contains_environment_reference(trimmed)
}

fn contains_environment_reference(value: &str) -> bool {
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'$' {
            let tail = &bytes[index + 1..];
            if tail.first() == Some(&b'{')
                || tail
                    .first()
                    .is_some_and(|next| next.is_ascii_uppercase() || *next == b'_')
            {
                return true;
            }
        }
        if *byte == b'%' {
            let tail = &bytes[index + 1..];
            if let Some(end) = tail.iter().position(|candidate| *candidate == b'%')
                && end > 0
                && tail[..end]
                    .iter()
                    .all(|candidate| candidate.is_ascii_uppercase() || *candidate == b'_')
            {
                return true;
            }
        }
    }
    false
}

fn collect_material(value: &Value, keys: &mut Vec<String>, strings: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                keys.push(key.to_lowercase());
                collect_material(child, keys, strings);
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|item| collect_material(item, keys, strings)),
        Value::String(value) => strings.push(value.clone()),
        _ => {}
    }
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, CapabilityError> {
    let value = serde_json::to_value(value)?;
    let normalized = canonical_value(value);
    Ok(serde_json::to_vec(&normalized)?)
}

fn canonical_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted = map
                .into_iter()
                .map(|(key, value)| (key, canonical_value(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical_value).collect()),
        other => other,
    }
}

pub struct CapabilityStore {
    conn: Connection,
}

impl CapabilityStore {
    pub fn open(path: &str) -> Result<Self, CapabilityError> {
        let store = Self {
            conn: Connection::open(path)?,
        };
        store.create_schema()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, CapabilityError> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.create_schema()?;
        Ok(store)
    }

    fn create_schema(&self) -> Result<(), CapabilityError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS capability_bundles (
                content_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                status TEXT NOT NULL,
                bundle_json TEXT NOT NULL,
                locally_validated INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );
             CREATE TABLE IF NOT EXISTS capability_grants (
                content_id TEXT NOT NULL,
                permission_json TEXT NOT NULL,
                revoked INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(content_id, permission_json),
                FOREIGN KEY(content_id) REFERENCES capability_bundles(content_id) ON DELETE CASCADE
             );",
        )?;
        Ok(())
    }

    pub fn import(&self, bytes: &[u8]) -> Result<ImportedCapability, CapabilityError> {
        let bundle = import_bundle(bytes)?;
        reconstruct_bundle(&bundle)?;
        let json = serde_json::to_string(&bundle)?;
        self.conn.execute(
            "INSERT INTO capability_bundles(content_id, name, status, bundle_json, locally_validated, created_at)
             VALUES (?1, ?2, 'quarantined', ?3, 0, ?4)
             ON CONFLICT(content_id) DO NOTHING",
            params![bundle.content_id, bundle.name, json, unix_time()],
        )?;
        self.imported_status(&bundle.content_id)
    }

    /// Import and locally revalidate in one durable state transition. Parsing,
    /// content verification, dependency reconstruction, and fixture execution
    /// finish before the row is written, so a clean store never observes an
    /// intermediate executable state. A fixture failure is retained as a
    /// rejected quarantined record for inspection.
    pub fn import_and_revalidate(
        &self,
        bytes: &[u8],
        validation: &LocalValidation,
    ) -> Result<ImportedCapability, CapabilityError> {
        let bundle = import_bundle(bytes)?;
        let reconstructed = reconstruct_bundle(&bundle)?;
        validate_local_validation_evidence(validation)?;
        let locally_validated = validation.passed
            && reconstructed
                .procedures
                .iter()
                .all(|procedure| run_sandbox_tests(procedure).is_ok());
        let status = if locally_validated {
            CapabilityStatus::Provisional
        } else {
            CapabilityStatus::Rejected
        };
        let status_name = match status {
            CapabilityStatus::Provisional => "provisional",
            CapabilityStatus::Rejected => "rejected",
            _ => unreachable!("atomic import only produces terminal local validation states"),
        };
        // A pre-existing quarantined or rejected copy must transition with the
        // same atomic upsert. Returning the proposed state without updating
        // that row would let callers believe local validation succeeded while
        // the durable authority still said "quarantined". Conversely, a
        // failed fresh revalidation must immediately revoke a formerly active
        // status for the same portable content identity.
        self.conn.execute(
            "INSERT INTO capability_bundles(content_id, name, status, bundle_json, locally_validated, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(content_id) DO UPDATE SET
                status = excluded.status,
                locally_validated = excluded.locally_validated",
            params![
                bundle.content_id,
                bundle.name,
                status_name,
                serde_json::to_string(&bundle)?,
                i64::from(locally_validated),
                unix_time()
            ],
        )?;
        self.imported_status(&bundle.content_id)
    }

    /// Reconstruct the neutral procedure graph from the stored canonical
    /// bundle. This does not grant authority or execute a foreign procedure.
    pub fn reconstruct(
        &self,
        content_id: &str,
    ) -> Result<ReconstructedCapability, CapabilityError> {
        let json: String = self
            .conn
            .query_row(
                "SELECT bundle_json FROM capability_bundles WHERE content_id = ?1",
                params![content_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| CapabilityError::Invalid("capability not found".into()))?;
        reconstruct_bundle(&serde_json::from_str(&json)?)
    }

    fn imported_status(&self, content_id: &str) -> Result<ImportedCapability, CapabilityError> {
        self.conn
            .query_row(
                "SELECT content_id, name, status, locally_validated
                 FROM capability_bundles WHERE content_id = ?1",
                params![content_id],
                |row| {
                    let status: String = row.get(2)?;
                    let status = match status.as_str() {
                        "quarantined" => CapabilityStatus::Quarantined,
                        "provisional" => CapabilityStatus::Provisional,
                        "active" => CapabilityStatus::Active,
                        "rejected" => CapabilityStatus::Rejected,
                        _ => {
                            return Err(rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                format!("unknown stored capability status {status}").into(),
                            ));
                        }
                    };
                    Ok(ImportedCapability {
                        content_id: row.get(0)?,
                        name: row.get(1)?,
                        status,
                        locally_validated: row.get::<_, i64>(3)? != 0,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| CapabilityError::Invalid("capability import was not persisted".into()))
    }

    pub fn export(&self, content_id: &str) -> Result<Vec<u8>, CapabilityError> {
        let json: String = self
            .conn
            .query_row(
                "SELECT bundle_json FROM capability_bundles WHERE content_id = ?1",
                params![content_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| CapabilityError::Invalid("capability not found".into()))?;
        export_bundle(&serde_json::from_str(&json)?)
    }

    pub fn grant(&self, content_id: &str, permission: &Permission) -> Result<(), CapabilityError> {
        let bundle_json: Option<String> = self
            .conn
            .query_row(
                "SELECT bundle_json FROM capability_bundles WHERE content_id = ?1",
                params![content_id],
                |row| row.get(0),
            )
            .optional()?;
        let bundle: CapabilityBundle = serde_json::from_str(
            &bundle_json.ok_or_else(|| CapabilityError::Invalid("capability not found".into()))?,
        )?;
        if !bundle
            .procedures
            .iter()
            .any(|procedure| procedure.permissions.contains(permission))
        {
            return Err(CapabilityError::Invalid(
                "permission was not declared by the capability".into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO capability_grants(content_id, permission_json, revoked) VALUES (?1, ?2, 0)
             ON CONFLICT(content_id, permission_json) DO UPDATE SET revoked = 0",
            params![content_id, serde_json::to_string(permission)?],
        )?;
        Ok(())
    }

    pub fn revoke(&self, content_id: &str, permission: &Permission) -> Result<(), CapabilityError> {
        self.conn.execute(
            "UPDATE capability_grants SET revoked = 1 WHERE content_id = ?1 AND permission_json = ?2",
            params![content_id, serde_json::to_string(permission)?],
        )?;
        Ok(())
    }

    pub fn revalidate(
        &self,
        content_id: &str,
        validation: &LocalValidation,
    ) -> Result<ImportedCapability, CapabilityError> {
        let bundle_json: String = self.conn.query_row(
            "SELECT bundle_json FROM capability_bundles WHERE content_id = ?1",
            params![content_id],
            |row| row.get(0),
        )?;
        let bundle: CapabilityBundle = serde_json::from_str(&bundle_json)?;
        if bundle.content_id != content_id {
            return Err(CapabilityError::InvalidBundle(
                "stored content identity mismatch".into(),
            ));
        }
        let reconstructed = reconstruct_bundle(&bundle)?;
        validate_local_validation_evidence(validation)?;
        let locally_validated = validation.passed
            && reconstructed
                .procedures
                .iter()
                .all(|procedure| run_sandbox_tests(procedure).is_ok());
        let status = if locally_validated {
            CapabilityStatus::Provisional
        } else {
            CapabilityStatus::Rejected
        };
        // A single update is the revalidation commit point. Local receipts do
        // not enter the bundle, preserving its exported content identity.
        self.conn.execute(
            "UPDATE capability_bundles SET status = ?2, locally_validated = ?3 WHERE content_id = ?1",
            params![
                content_id,
                if locally_validated { "provisional" } else { "rejected" },
                i64::from(locally_validated)
            ],
        )?;
        Ok(ImportedCapability {
            content_id: content_id.into(),
            name: bundle.name,
            status,
            locally_validated,
        })
    }

    pub fn require_permissions(
        &self,
        content_id: &str,
        permissions: &[Permission],
    ) -> Result<(), CapabilityError> {
        if permissions.is_empty() {
            return Err(CapabilityError::PermissionRequired(
                "at least one declared permission".into(),
            ));
        }
        let stored: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT status, bundle_json FROM capability_bundles WHERE content_id = ?1",
                params![content_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (status, bundle_json) = stored.ok_or(CapabilityError::NotRevalidated)?;
        if status != "provisional" && status != "active" {
            return Err(CapabilityError::NotRevalidated);
        }
        let bundle: CapabilityBundle = serde_json::from_str(&bundle_json)?;
        for permission in permissions {
            if !bundle
                .procedures
                .iter()
                .any(|procedure| procedure.permissions.contains(permission))
            {
                return Err(CapabilityError::PermissionRequired(format!(
                    "undeclared {permission:?}"
                )));
            }
            let granted: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM capability_grants WHERE content_id = ?1 AND permission_json = ?2 AND revoked = 0)",
                params![content_id, serde_json::to_string(permission)?],
                |row| row.get(0),
            )?;
            if !granted {
                return Err(CapabilityError::PermissionRequired(format!(
                    "{permission:?}"
                )));
            }
        }
        Ok(())
    }

    /// Authorize the exact permissions declared by one procedure.
    ///
    /// A bundle may contain multiple procedures with disjoint authority;
    /// authorizing one of them must not accidentally authorize another.
    pub fn require_procedure_permissions(
        &self,
        content_id: &str,
        procedure_id: &str,
    ) -> Result<CapabilityProcedure, CapabilityError> {
        let bundle_json: String = self
            .conn
            .query_row(
                "SELECT bundle_json FROM capability_bundles WHERE content_id = ?1",
                params![content_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(CapabilityError::NotRevalidated)?;
        let bundle: CapabilityBundle = serde_json::from_str(&bundle_json)?;
        let procedure = bundle
            .procedures
            .into_iter()
            .find(|procedure| procedure.id == procedure_id)
            .ok_or_else(|| CapabilityError::Invalid("procedure not found".into()))?;
        self.require_permissions(content_id, &procedure.permissions)?;
        Ok(procedure)
    }
}

fn validate_local_validation_evidence(validation: &LocalValidation) -> Result<(), CapabilityError> {
    if validation.passed
        && (validation.environment_digest.trim().is_empty()
            || validation.validation_episodes.is_empty()
            || validation
                .validation_episodes
                .iter()
                .any(|episode| episode.trim().is_empty()))
    {
        return Err(CapabilityError::Invalid(
            "local validation requires environment and episode evidence".into(),
        ));
    }
    Ok(())
}

fn unix_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface() -> InterfaceDescription {
        InterfaceDescription {
            source: "weather-api".into(),
            fingerprint: "api-v1".into(),
            operations: vec![DiscoveredOperation {
                name: "forecast".into(),
                input_schema: serde_json::json!({"type":"object"}),
                output_schema: serde_json::json!({"type":"object"}),
                host: "api.example.test".into(),
                method: "GET".into(),
                response_fixture: serde_json::json!({"temperature":72}),
            }],
        }
    }

    #[test]
    fn discovery_synthesizes_typed_effectful_procedure_and_round_trips() {
        let bundle = discover_interface(&interface()).unwrap();
        assert_eq!(
            bundle.procedures[0].primitive,
            NativePrimitive::NetworkRequest
        );
        run_sandbox_tests(&bundle.procedures[0]).unwrap();
        let bytes = export_bundle(&bundle).unwrap();
        let imported = import_bundle(&bytes).unwrap();
        assert_eq!(imported.content_id, bundle.content_id);
        assert_eq!(export_bundle(&imported).unwrap(), bytes);
    }

    #[test]
    fn imported_capability_is_quarantined_until_local_validation_and_grant() {
        let bundle = discover_interface(&interface()).unwrap();
        let bytes = export_bundle(&bundle).unwrap();
        let store = CapabilityStore::in_memory().unwrap();
        let imported = store.import(&bytes).unwrap();
        assert_eq!(imported.status, CapabilityStatus::Quarantined);
        assert!(
            store
                .require_permissions(&imported.content_id, &bundle.procedures[0].permissions)
                .is_err()
        );
        let validated = store
            .revalidate(
                &imported.content_id,
                &LocalValidation {
                    passed: true,
                    validation_episodes: vec!["trusted-local-episode".into()],
                    environment_digest: "local-env".into(),
                },
            )
            .unwrap();
        assert_eq!(validated.status, CapabilityStatus::Provisional);
        store
            .grant(&imported.content_id, &bundle.procedures[0].permissions[0])
            .unwrap();
        store
            .require_permissions(&imported.content_id, &bundle.procedures[0].permissions)
            .unwrap();
        let authorized = store
            .require_procedure_permissions(&imported.content_id, &bundle.procedures[0].id)
            .unwrap();
        assert_eq!(authorized.id, bundle.procedures[0].id);
        store
            .revoke(&imported.content_id, &bundle.procedures[0].permissions[0])
            .unwrap();
        assert!(
            store
                .require_permissions(&imported.content_id, &bundle.procedures[0].permissions)
                .is_err()
        );
    }

    #[test]
    fn secret_bearing_bundle_is_rejected_atomically() {
        let mut bundle = discover_interface(&interface()).unwrap();
        bundle.reconstruction.compatibility = vec!["Bearer do-not-transfer".into()];
        bundle.content_id = bundle_content_id(&bundle).unwrap();
        assert!(matches!(
            export_bundle(&bundle),
            Err(CapabilityError::InvalidBundle(_))
        ));
    }

    #[test]
    fn primitive_policy_blocks_path_escape_and_undeclared_effects() {
        let policy = PrimitivePolicy {
            network_hosts: BTreeSet::from(["api.example.test".into()]),
            file_read_prefixes: BTreeSet::from(["/tmp/ekg".into()]),
            file_write_prefixes: BTreeSet::new(),
            observe_targets: BTreeSet::from(["clock".into()]),
            sandbox_profiles: BTreeSet::from(["pure".into()]),
            bounds: ResourceBounds {
                max_bytes: 128,
                max_steps: 10,
                max_millis: 100,
            },
        };
        assert!(
            policy
                .authorize(&PrimitiveRequest::FileRead {
                    path: "/tmp/ekg/../secrets".into(),
                    bytes: 1,
                })
                .is_err()
        );
        assert!(
            policy
                .authorize(&PrimitiveRequest::FileRead {
                    path: "tmp/ekg/file".into(),
                    bytes: 1,
                })
                .is_err()
        );
        assert!(
            policy
                .authorize(&PrimitiveRequest::Network {
                    host: "evil.example.test".into(),
                    method: "GET".into(),
                    body_bytes: 1,
                })
                .is_err()
        );
        let receipt = policy
            .authorize(&PrimitiveRequest::Observe {
                target: "clock".into(),
            })
            .unwrap();
        assert_eq!(receipt.effect, Effect::Observation);
        assert!(
            policy
                .authorize(&PrimitiveRequest::SandboxExecute {
                    profile: "pure".into(),
                    steps: 11,
                })
                .is_err()
        );

        let executor = NativePrimitiveExecutor::new(policy.clone());
        let execution = executor
            .network_request(
                &PrimitiveRequest::Network {
                    host: "api.example.test".into(),
                    method: "GET".into(),
                    body_bytes: 64,
                },
                &serde_json::json!({"city":"Phoenix"}),
                |host, method, body| {
                    assert_eq!(host, "api.example.test");
                    assert_eq!(method, "GET");
                    assert_eq!(body["city"], "Phoenix");
                    Ok(serde_json::json!({"temperature": 72}))
                },
            )
            .unwrap();
        assert_eq!(execution.receipt.target, "api.example.test");
        assert_eq!(execution.output["temperature"], 72);
    }

    #[test]
    fn bundle_rejects_secret_values_and_machine_local_material() {
        for compatibility in [
            "Bearer exported-credential",
            "C:\\Users\\alice\\build.cmd",
            "${HOME}/private/tool",
        ] {
            let mut bundle = discover_interface(&interface()).unwrap();
            bundle.reconstruction.compatibility = vec![compatibility.into()];
            bundle.content_id = bundle_content_id(&bundle).unwrap();
            assert!(matches!(
                export_bundle(&bundle),
                Err(CapabilityError::InvalidBundle(_))
            ));
        }
    }

    #[test]
    fn bundle_rejects_mismatched_authority_and_unbounded_resources() {
        let mut bundle = discover_interface(&interface()).unwrap();
        bundle.procedures[0].effects = vec![Effect::Observation];
        bundle.content_id = bundle_content_id(&bundle).unwrap();
        assert!(validate_bundle(&bundle).is_err());

        let mut bundle = discover_interface(&interface()).unwrap();
        bundle.procedures[0].bounds.max_bytes = u64::MAX;
        bundle.content_id = bundle_content_id(&bundle).unwrap();
        assert!(validate_bundle(&bundle).is_err());
    }

    #[test]
    fn registry_rejects_ambient_or_empty_permission_checks() {
        let bundle = discover_interface(&interface()).unwrap();
        let store = CapabilityStore::in_memory().unwrap();
        let imported = store.import(&export_bundle(&bundle).unwrap()).unwrap();
        let ambient = Permission::ObserveTarget {
            target: "clock".into(),
        };
        assert!(store.grant(&imported.content_id, &ambient).is_err());
        assert!(
            store
                .require_permissions(&imported.content_id, &[])
                .is_err()
        );
    }

    fn dependency(
        name: &str,
        hash_byte: char,
        dependencies: Vec<DependencyReference>,
    ) -> Dependency {
        Dependency {
            name: name.into(),
            version: "1.0.0".into(),
            content_hash: format!("sha256:{}", hash_byte.to_string().repeat(64)),
            dependencies,
        }
    }

    fn reference(dependency: &Dependency) -> DependencyReference {
        DependencyReference {
            name: dependency.name.clone(),
            version: dependency.version.clone(),
            content_hash: dependency.content_hash.clone(),
        }
    }

    #[test]
    fn clean_store_reconstructs_neutral_procedures_fixtures_and_dependency_dag() {
        let mut bundle = discover_interface(&interface()).unwrap();
        let transport = dependency("transport", 'a', Vec::new());
        let client = dependency("weather-client", 'b', vec![reference(&transport)]);
        bundle.dependencies = vec![client.clone(), transport.clone()];
        bundle.procedures[0].dependencies = vec![reference(&client)];
        bundle.content_id = bundle_content_id(&bundle).unwrap();

        let store = CapabilityStore::in_memory().unwrap();
        let imported = store.import(&export_bundle(&bundle).unwrap()).unwrap();
        let reconstructed = store.reconstruct(&imported.content_id).unwrap();

        assert_eq!(
            reconstructed
                .dependency_order
                .iter()
                .map(|dependency| dependency.name.as_str())
                .collect::<Vec<_>>(),
            vec!["transport", "weather-client"]
        );
        assert_eq!(
            reconstructed.procedures[0].neutral_metadata.kind,
            NeutralProcedureKind::NativePrimitive
        );
        run_sandbox_tests(&reconstructed.procedures[0]).unwrap();
        assert_eq!(
            export_bundle(&bundle).unwrap(),
            store.export(&imported.content_id).unwrap()
        );
    }

    #[test]
    fn dependency_cycles_and_incomplete_closure_are_rejected_before_import() {
        let mut bundle = discover_interface(&interface()).unwrap();
        let transport = dependency("transport", 'a', Vec::new());
        let client = dependency("weather-client", 'b', vec![reference(&transport)]);
        let mut cyclic_transport = transport.clone();
        cyclic_transport.dependencies = vec![reference(&client)];
        bundle.dependencies = vec![cyclic_transport, client];
        bundle.content_id = bundle_content_id(&bundle).unwrap();
        assert!(matches!(
            export_bundle(&bundle),
            Err(CapabilityError::InvalidBundle(_))
        ));

        let mut bundle = discover_interface(&interface()).unwrap();
        let missing = DependencyReference {
            name: "missing".into(),
            version: "1.0.0".into(),
            content_hash: format!("sha256:{}", "c".repeat(64)),
        };
        bundle.dependencies = vec![dependency("transport", 'a', vec![missing])];
        bundle.content_id = bundle_content_id(&bundle).unwrap();
        assert!(matches!(
            import_bundle(&canonical_json(&bundle).unwrap()),
            Err(CapabilityError::InvalidBundle(_))
        ));
    }

    #[test]
    fn atomic_import_and_revalidation_never_exposes_a_fixture_failure_as_validated() {
        let mut bundle = discover_interface(&interface()).unwrap();
        bundle.procedures[0].tests[0].fixture_output = serde_json::json!({"temperature": 0});
        bundle.content_id = bundle_content_id(&bundle).unwrap();
        let store = CapabilityStore::in_memory().unwrap();
        let result = store
            .import_and_revalidate(
                &export_bundle(&bundle).unwrap(),
                &LocalValidation {
                    passed: true,
                    validation_episodes: vec!["local-fixture-check".into()],
                    environment_digest: "sha256:local-environment".into(),
                },
            )
            .unwrap();

        assert_eq!(result.status, CapabilityStatus::Rejected);
        assert!(!result.locally_validated);
        assert!(matches!(
            store.require_procedure_permissions(&result.content_id, &bundle.procedures[0].id),
            Err(CapabilityError::NotRevalidated)
        ));
        assert!(store.reconstruct(&result.content_id).is_ok());
    }

    #[test]
    fn atomic_import_revalidation_updates_an_existing_quarantined_bundle() {
        let bundle = discover_interface(&interface()).unwrap();
        let bytes = export_bundle(&bundle).unwrap();
        let store = CapabilityStore::in_memory().unwrap();
        let quarantined = store.import(&bytes).unwrap();
        assert_eq!(quarantined.status, CapabilityStatus::Quarantined);

        let revalidated = store
            .import_and_revalidate(
                &bytes,
                &LocalValidation {
                    passed: true,
                    validation_episodes: vec!["local-fixture-check".into()],
                    environment_digest: "sha256:local-environment".into(),
                },
            )
            .unwrap();
        assert_eq!(revalidated.status, CapabilityStatus::Provisional);
        assert!(revalidated.locally_validated);

        // A repeated import reports the durable state rather than pretending
        // the already revalidated capability was newly quarantined.
        assert_eq!(store.import(&bytes).unwrap(), revalidated);
        store
            .grant(
                &revalidated.content_id,
                &bundle.procedures[0].permissions[0],
            )
            .unwrap();
        store
            .require_procedure_permissions(&revalidated.content_id, &bundle.procedures[0].id)
            .unwrap();
    }

    #[test]
    fn failed_atomic_revalidation_revokes_an_existing_provisional_status() {
        let bundle = discover_interface(&interface()).unwrap();
        let bytes = export_bundle(&bundle).unwrap();
        let store = CapabilityStore::in_memory().unwrap();
        let validated = store
            .import_and_revalidate(
                &bytes,
                &LocalValidation {
                    passed: true,
                    validation_episodes: vec!["local-fixture-check".into()],
                    environment_digest: "sha256:local-environment".into(),
                },
            )
            .unwrap();
        store
            .grant(&validated.content_id, &bundle.procedures[0].permissions[0])
            .unwrap();

        let rejected = store
            .import_and_revalidate(
                &bytes,
                &LocalValidation {
                    passed: false,
                    validation_episodes: Vec::new(),
                    environment_digest: String::new(),
                },
            )
            .unwrap();
        assert_eq!(rejected.status, CapabilityStatus::Rejected);
        assert!(!rejected.locally_validated);
        assert!(matches!(
            store.require_procedure_permissions(&rejected.content_id, &bundle.procedures[0].id),
            Err(CapabilityError::NotRevalidated)
        ));
    }

    #[test]
    fn clock_observation_requires_a_local_grant_and_mints_a_receipt() {
        let policy = PrimitivePolicy {
            observe_targets: BTreeSet::from(["clock".into()]),
            ..PrimitivePolicy::default()
        };
        let execution = NativePrimitiveExecutor::new(policy)
            .observe(&PrimitiveRequest::Observe {
                target: "clock".into(),
            })
            .unwrap();
        assert_eq!(execution.receipt.primitive, NativePrimitive::Observe);
        assert_eq!(execution.output["source"], "native:clock");
        assert!(execution.output["unixSeconds"].is_number());
    }

    #[test]
    fn file_adapters_stay_inside_a_real_scoped_directory() {
        let root = std::env::temp_dir().join(format!("ekg-native-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("fixture.txt");
        let prefix = root.to_string_lossy().to_string();
        let policy = PrimitivePolicy {
            file_read_prefixes: BTreeSet::from([prefix.clone()]),
            file_write_prefixes: BTreeSet::from([prefix]),
            sandbox_profiles: BTreeSet::from(["fixture".into()]),
            bounds: ResourceBounds {
                max_bytes: 64,
                ..ResourceBounds::default()
            },
            ..PrimitivePolicy::default()
        };
        let executor = NativePrimitiveExecutor::new(policy);
        executor
            .write_file(
                &PrimitiveRequest::FileWrite {
                    path: target.to_string_lossy().into(),
                    bytes: 7,
                },
                &serde_json::json!("fixture"),
            )
            .unwrap();
        let read = executor
            .read_file(&PrimitiveRequest::FileRead {
                path: target.to_string_lossy().into(),
                bytes: 7,
            })
            .unwrap();
        assert_eq!(
            read.output["bytes"],
            serde_json::json!([102, 105, 120, 116, 117, 114, 101])
        );
        let sandbox = executor.sandbox_fixture(
            &PrimitiveRequest::SandboxExecute {
                profile: "fixture".into(),
                steps: 1,
            },
            &serde_json::json!({"input": 1}),
        );
        assert_eq!(sandbox.unwrap().output["sandboxed"], true);
        std::fs::remove_dir_all(root).unwrap();
    }
}
