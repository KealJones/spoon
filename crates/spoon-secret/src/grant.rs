use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use spoon_capability::NativePrimitive;

use crate::{SecretError, SecretRef};

const MAX_TARGET_BYTES: usize = 512;
const MAX_PURPOSE_BYTES: usize = 64;

/// Where a secret may be used. A scope is a pattern: for the file primitives
/// the target is a path prefix, and for every other primitive it is the exact
/// host, observe target, or sandbox profile.
///
/// `purpose` is matched exactly and is not optional. A secret granted for
/// `publish` must not be readable by a code path doing `authenticate`, even on
/// the same host, because the two disclose the value to different places.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretScope {
    pub primitive: NativePrimitive,
    pub target: String,
    pub purpose: String,
}

impl SecretScope {
    pub fn new(
        primitive: NativePrimitive,
        target: &str,
        purpose: &str,
    ) -> Result<Self, SecretError> {
        check_target(target)?;
        check_purpose(purpose)?;
        Ok(Self {
            primitive,
            target: target.to_owned(),
            purpose: purpose.to_owned(),
        })
    }

    pub fn permits(&self, use_site: &SecretUse) -> bool {
        self.primitive == use_site.primitive
            && self.purpose == use_site.purpose
            && match self.primitive {
                NativePrimitive::FileRead | NativePrimitive::FileWrite => {
                    path_prefix_permits(&self.target, &use_site.target)
                }
                _ => self.target == use_site.target,
            }
    }
}

/// One concrete point of use, declared by the adapter that is about to
/// disclose the value. This is the question a grant answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretUse {
    pub primitive: NativePrimitive,
    pub target: String,
    pub purpose: String,
}

impl SecretUse {
    pub fn new(
        primitive: NativePrimitive,
        target: &str,
        purpose: &str,
    ) -> Result<Self, SecretError> {
        check_target(target)?;
        check_purpose(purpose)?;
        Ok(Self {
            primitive,
            target: target.to_owned(),
            purpose: purpose.to_owned(),
        })
    }
}

impl fmt::Display for SecretUse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} on {} for {}",
            primitive_label(&self.primitive),
            self.target,
            self.purpose
        )
    }
}

/// Local authority to read one version of one secret, bounded by scope and by
/// time. A grant is host-owned local state; it is never carried by a portable
/// capability bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretGrant {
    pub reference: SecretRef,
    pub scopes: Vec<SecretScope>,
    pub issued_at: i64,
    /// Last instant at which the grant is usable, in unix seconds. Expiry is
    /// mandatory: an unbounded grant cannot be distinguished from a forgotten
    /// one.
    pub not_after: i64,
}

impl SecretGrant {
    pub fn new(
        reference: SecretRef,
        scopes: Vec<SecretScope>,
        issued_at: i64,
        not_after: i64,
    ) -> Result<Self, SecretError> {
        if scopes.is_empty() {
            return Err(SecretError::Invalid(
                "a secret grant must declare at least one scope".into(),
            ));
        }
        if not_after <= issued_at {
            return Err(SecretError::Invalid(
                "a secret grant must expire after it is issued".into(),
            ));
        }
        Ok(Self {
            reference,
            scopes,
            issued_at,
            not_after,
        })
    }

    pub fn permits(&self, use_site: &SecretUse) -> bool {
        self.scopes.iter().any(|scope| scope.permits(use_site))
    }

    pub fn is_expired(&self, now: i64) -> bool {
        now > self.not_after
    }
}

/// Why a grant is or is not usable. A superseded or revoked grant is kept
/// rather than deleted, so the ledger remains an audit trail of what was ever
/// authorized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum GrantStatus {
    Active,
    Superseded { by: u32, at: i64 },
    Revoked { at: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantRecord {
    pub grant: SecretGrant,
    pub status: GrantStatus,
}

/// The local authority record for secret use. Nothing resolves a secret
/// without passing this first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GrantLedger {
    records: BTreeMap<SecretRef, GrantRecord>,
}

impl GrantLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the first grant for a secret lineage. Later versions arrive
    /// through [`rotate`](Self::rotate), which is the only way to get a second
    /// version, so a rotation can never be recorded as an unrelated grant that
    /// leaves the previous version active.
    pub fn issue(&mut self, grant: SecretGrant) -> Result<(), SecretError> {
        if self
            .latest(grant.reference.namespace(), grant.reference.name())
            .is_some()
        {
            return Err(SecretError::Invalid(format!(
                "{} already has a grant; rotate it instead",
                grant.reference
            )));
        }
        self.records.insert(
            grant.reference.clone(),
            GrantRecord {
                grant,
                status: GrantStatus::Active,
            },
        );
        Ok(())
    }

    /// Supersede the current version with a higher one, returning the
    /// reference that was superseded. The old record stays in the ledger with
    /// its supersession time, and stops authorizing use.
    pub fn rotate(&mut self, grant: SecretGrant, at: i64) -> Result<SecretRef, SecretError> {
        let previous = self
            .latest(grant.reference.namespace(), grant.reference.name())
            .ok_or_else(|| {
                SecretError::Invalid(format!(
                    "{} has no grant to rotate; issue it first",
                    grant.reference
                ))
            })?;
        if grant.reference.version() <= previous.version() {
            return Err(SecretError::Invalid(format!(
                "rotation must advance {} past version {}",
                previous,
                previous.version()
            )));
        }
        let Some(record) = self.records.get_mut(&previous) else {
            return Err(SecretError::Unknown(previous));
        };
        record.status = GrantStatus::Superseded {
            by: grant.reference.version(),
            at,
        };
        self.records.insert(
            grant.reference.clone(),
            GrantRecord {
                grant,
                status: GrantStatus::Active,
            },
        );
        Ok(previous)
    }

    /// Withdraw an active grant without rotating. Only an active grant can be
    /// revoked, so revocation cannot overwrite a supersession record.
    pub fn revoke(&mut self, reference: &SecretRef, at: i64) -> Result<(), SecretError> {
        let Some(record) = self.records.get_mut(reference) else {
            return Err(SecretError::Unknown(reference.clone()));
        };
        if record.status != GrantStatus::Active {
            return Err(SecretError::Invalid(format!(
                "{reference} is not an active grant"
            )));
        }
        record.status = GrantStatus::Revoked { at };
        Ok(())
    }

    /// The single authorization decision: does this exact reference authorize
    /// this exact use at this instant.
    pub fn authorize(
        &self,
        reference: &SecretRef,
        use_site: &SecretUse,
        now: i64,
    ) -> Result<&GrantRecord, SecretError> {
        let record = self
            .records
            .get(reference)
            .ok_or_else(|| SecretError::Unknown(reference.clone()))?;
        match record.status {
            GrantStatus::Active => {}
            GrantStatus::Superseded { by, .. } => {
                return Err(SecretError::Superseded {
                    reference: reference.clone(),
                    current: by,
                });
            }
            GrantStatus::Revoked { at } => {
                return Err(SecretError::Revoked {
                    reference: reference.clone(),
                    at,
                });
            }
        }
        if record.grant.is_expired(now) {
            return Err(SecretError::Expired {
                reference: reference.clone(),
                not_after: record.grant.not_after,
                now,
            });
        }
        if !record.grant.permits(use_site) {
            return Err(SecretError::OutOfScope {
                reference: reference.clone(),
                use_site: use_site.clone(),
            });
        }
        Ok(record)
    }

    /// Highest known version of a lineage, whatever its status. Rotation
    /// compares against this rather than against the highest active version,
    /// so a revoked version still cannot be reissued.
    pub fn latest(&self, namespace: &str, name: &str) -> Option<SecretRef> {
        self.records
            .keys()
            .filter(|reference| reference.namespace() == namespace && reference.name() == name)
            .max_by_key(|reference| reference.version())
            .cloned()
    }

    pub fn record(&self, reference: &SecretRef) -> Option<&GrantRecord> {
        self.records.get(reference)
    }

    /// Every version of a lineage in version order, including superseded and
    /// revoked ones. This is the audit trail.
    pub fn history(&self, namespace: &str, name: &str) -> Vec<&GrantRecord> {
        self.records
            .iter()
            .filter(|(reference, _)| reference.namespace() == namespace && reference.name() == name)
            .map(|(_, record)| record)
            .collect()
    }
}

/// Prefix matching on a component boundary. A plain `starts_with` would let a
/// grant on `/var/data` read `/var/database`.
fn path_prefix_permits(prefix: &str, path: &str) -> bool {
    if path == prefix {
        return true;
    }
    let trimmed = prefix.strip_suffix('/').unwrap_or(prefix);
    path.strip_prefix(trimmed)
        .is_some_and(|rest| rest.starts_with('/'))
}

fn check_target(target: &str) -> Result<(), SecretError> {
    if target.is_empty() || target.len() > MAX_TARGET_BYTES {
        return Err(SecretError::Invalid(format!(
            "secret scope target must be 1 to {MAX_TARGET_BYTES} bytes"
        )));
    }
    if target.chars().any(char::is_control) {
        return Err(SecretError::Invalid(
            "secret scope target must not contain control characters".into(),
        ));
    }
    Ok(())
}

fn check_purpose(purpose: &str) -> Result<(), SecretError> {
    if purpose.is_empty() || purpose.len() > MAX_PURPOSE_BYTES {
        return Err(SecretError::Invalid(format!(
            "secret scope purpose must be 1 to {MAX_PURPOSE_BYTES} bytes"
        )));
    }
    if !purpose
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err(SecretError::Invalid(
            "secret scope purpose must be alphanumeric with - _ .".into(),
        ));
    }
    Ok(())
}

fn primitive_label(primitive: &NativePrimitive) -> &'static str {
    match primitive {
        NativePrimitive::NetworkRequest => "network_request",
        NativePrimitive::FileRead => "file_read",
        NativePrimitive::FileWrite => "file_write",
        NativePrimitive::Observe => "observe",
        NativePrimitive::SandboxExecute => "sandbox_execute",
    }
}
