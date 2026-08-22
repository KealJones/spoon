//! Policy-enforced capability acquisition and portable bundle exchange.
//!
//! This crate intentionally models the native substrate without performing
//! ambient I/O. Discovery produces typed candidates, sandbox validation uses
//! supplied fixtures, and invocation requires an explicit local grant.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const BUNDLE_FORMAT_VERSION: u16 = 1;
pub const MAX_PROCEDURES: usize = 64;
pub const MAX_DEPENDENCIES: usize = 128;
pub const MAX_TESTS: usize = 256;
pub const MAX_SCHEMA_BYTES: usize = 32 * 1024;
pub const MAX_BUNDLE_BYTES: usize = 512 * 1024;

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
pub struct InvocationReceipt {
    pub primitive: NativePrimitive,
    pub effect: Effect,
    pub target: String,
    pub payload_digest: String,
    pub bounds: ResourceBounds,
}

impl PrimitivePolicy {
    pub fn authorize(
        &self,
        request: &PrimitiveRequest,
    ) -> Result<InvocationReceipt, CapabilityError> {
        let (primitive, effect, target, amount) = match request {
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
                    *body_bytes,
                )
            }
            PrimitiveRequest::FileRead { path, bytes } => {
                if !path_allowed(path, &self.file_read_prefixes) {
                    return Err(CapabilityError::PermissionRequired(format!(
                        "file read {path}"
                    )));
                }
                (
                    NativePrimitive::FileRead,
                    Effect::FileRead,
                    path.clone(),
                    *bytes,
                )
            }
            PrimitiveRequest::FileWrite { path, bytes } => {
                if !path_allowed(path, &self.file_write_prefixes) {
                    return Err(CapabilityError::PermissionRequired(format!(
                        "file write {path}"
                    )));
                }
                (
                    NativePrimitive::FileWrite,
                    Effect::FileWrite,
                    path.clone(),
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
        Ok(InvocationReceipt {
            primitive,
            effect,
            target,
            payload_digest,
            bounds: self.bounds.clone(),
        })
    }
}

fn path_allowed(path: &str, prefixes: &BTreeSet<String>) -> bool {
    let candidate = Path::new(path);
    if candidate
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return false;
    }
    prefixes
        .iter()
        .any(|prefix| candidate.starts_with(Path::new(prefix)))
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
pub struct CapabilityTest {
    pub name: String,
    pub input: Value,
    pub expected_output: Value,
    pub fixture_output: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: String,
    pub discovered_at: i64,
    pub interface_fingerprint: String,
    pub validation_episodes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    pub content_hash: String,
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
pub struct CapabilityProcedure {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub primitive: NativePrimitive,
    pub input_schema: Value,
    pub output_schema: Value,
    pub contract: Value,
    pub permissions: Vec<Permission>,
    pub effects: Vec<Effect>,
    pub bounds: ResourceBounds,
    pub dependencies: Vec<Dependency>,
    pub tests: Vec<CapabilityTest>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityBundle {
    pub format_version: u16,
    pub name: String,
    pub version: String,
    pub content_id: String,
    pub procedures: Vec<CapabilityProcedure>,
    pub dependencies: Vec<Dependency>,
    pub provenance: Provenance,
    pub reconstruction: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedCapability {
    pub content_id: String,
    pub name: String,
    pub status: CapabilityStatus,
    pub locally_validated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalValidation {
    pub passed: bool,
    pub validation_episodes: Vec<String>,
    pub environment_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredOperation {
    pub name: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub host: String,
    pub method: String,
    pub response_fixture: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        reconstruction: serde_json::json!({"kind":"native_primitive_procedure"}),
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
    reject_secrets(bundle)?;
    reject_local_authority(bundle)?;
    let mut dependency_hashes = BTreeSet::new();
    for dependency in &bundle.dependencies {
        if dependency.name.trim().is_empty()
            || dependency.version.trim().is_empty()
            || dependency.content_hash.trim().is_empty()
            || !dependency_hashes.insert(dependency.content_hash.clone())
        {
            return Err(CapabilityError::InvalidBundle(
                "dependency identity or closure is invalid".into(),
            ));
        }
    }
    let mut ids = BTreeSet::new();
    for procedure in &bundle.procedures {
        if procedure.id.trim().is_empty()
            || !ids.insert(procedure.id.clone())
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
        for test in &procedure.tests {
            if test.name.trim().is_empty() {
                return Err(CapabilityError::InvalidBundle(
                    "sandbox test name is required".into(),
                ));
            }
        }
        for dependency in &procedure.dependencies {
            if !dependency_hashes.contains(&dependency.content_hash) {
                return Err(CapabilityError::InvalidBundle(
                    "procedure dependency is missing from bundle closure".into(),
                ));
            }
        }
    }
    Ok(())
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
    collect_keys(&json, &mut keys);
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
    }) {
        return Err(CapabilityError::InvalidBundle(
            "secret-bearing field is prohibited".into(),
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
    }) || strings.iter().any(|value| {
        value.starts_with('/') || value.starts_with("file://") || value.contains("\\\\")
    }) {
        return Err(CapabilityError::InvalidBundle(
            "local authority or environment-specific path is prohibited".into(),
        ));
    }
    Ok(())
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

fn collect_keys(value: &Value, keys: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                keys.push(key.to_lowercase());
                collect_keys(child, keys);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_keys(item, keys)),
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
        let json = serde_json::to_string(&bundle)?;
        self.conn.execute(
            "INSERT INTO capability_bundles(content_id, name, status, bundle_json, locally_validated, created_at)
             VALUES (?1, ?2, 'quarantined', ?3, 0, ?4)
             ON CONFLICT(content_id) DO NOTHING",
            params![bundle.content_id, bundle.name, json, unix_time()],
        )?;
        Ok(ImportedCapability {
            content_id: bundle.content_id,
            name: bundle.name,
            status: CapabilityStatus::Quarantined,
            locally_validated: false,
        })
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
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM capability_bundles WHERE content_id = ?1)",
            params![content_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(CapabilityError::Invalid("capability not found".into()));
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
        if validation.passed {
            for procedure in &bundle.procedures {
                run_sandbox_tests(procedure)?;
            }
            // Local validation receipts live in the registry row, not in the
            // exported bundle, so revalidation never changes content identity.
            let json = serde_json::to_string(&bundle)?;
            self.conn.execute(
                "UPDATE capability_bundles SET status = 'provisional', bundle_json = ?2, locally_validated = 1 WHERE content_id = ?1",
                params![content_id, json],
            )?;
        } else {
            self.conn.execute(
                "UPDATE capability_bundles SET status = 'rejected' WHERE content_id = ?1",
                params![content_id],
            )?;
        }
        Ok(ImportedCapability {
            content_id: content_id.into(),
            name: bundle.name,
            status: if validation.passed {
                CapabilityStatus::Provisional
            } else {
                CapabilityStatus::Rejected
            },
            locally_validated: validation.passed,
        })
    }

    pub fn require_permissions(
        &self,
        content_id: &str,
        permissions: &[Permission],
    ) -> Result<(), CapabilityError> {
        let status: Option<String> = self
            .conn
            .query_row(
                "SELECT status FROM capability_bundles WHERE content_id = ?1",
                params![content_id],
                |row| row.get(0),
            )
            .optional()?;
        if status.as_deref() != Some("provisional") && status.as_deref() != Some("active") {
            return Err(CapabilityError::NotRevalidated);
        }
        for permission in permissions {
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
        bundle.reconstruction = serde_json::json!({"api_key":"do-not-transfer"});
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
    }
}
