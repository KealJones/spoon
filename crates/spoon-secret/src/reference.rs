use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::SecretError;

/// Prefix of the textual form of a reference. Present so a reader of a receipt
/// or an episode can recognize the value as a secret name and know that it is
/// not the secret.
pub const SECRET_SCHEME: &str = "secret://";

const MAX_SEGMENT_BYTES: usize = 64;

/// An opaque name for a locally held secret, at one version.
///
/// This is the only secret-related value that is safe to serialize, store, and
/// print. It carries no value and has no field that could hold one, and both
/// its formatting implementations are written by hand so that adding such a
/// field later cannot silently start printing it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretRef {
    namespace: String,
    name: String,
    version: u32,
}

impl SecretRef {
    /// Versions start at 1 so that "version 0" is never a valid reference and
    /// a zero-initialized value cannot resolve to anything.
    pub fn new(namespace: &str, name: &str, version: u32) -> Result<Self, SecretError> {
        check_segment("namespace", namespace)?;
        check_segment("name", name)?;
        if version == 0 {
            return Err(SecretError::Invalid(
                "secret reference version starts at 1".into(),
            ));
        }
        Ok(Self {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
            version,
        })
    }

    /// Parse the textual form produced by [`Display`](fmt::Display).
    pub fn parse(text: &str) -> Result<Self, SecretError> {
        let body = text.strip_prefix(SECRET_SCHEME).ok_or_else(|| {
            SecretError::Invalid(format!("secret reference must begin with {SECRET_SCHEME}"))
        })?;
        let (namespace, rest) = body.split_once('/').ok_or_else(|| {
            SecretError::Invalid("secret reference must name a namespace and a name".into())
        })?;
        let (name, version) = rest.split_once("@v").ok_or_else(|| {
            SecretError::Invalid("secret reference must end with an @v<version> suffix".into())
        })?;
        let version = version
            .parse::<u32>()
            .map_err(|_| SecretError::Invalid("secret reference version is not a number".into()))?;
        Self::new(namespace, name, version)
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    /// The same secret at a different version. Rotation produces the next
    /// version of an existing lineage rather than an unrelated reference, so
    /// the audit trail stays connected across a rotation.
    pub fn at_version(&self, version: u32) -> Result<Self, SecretError> {
        Self::new(&self.namespace, &self.name, version)
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{SECRET_SCHEME}{}/{}@v{}",
            self.namespace, self.name, self.version
        )
    }
}

impl fmt::Debug for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SecretRef({self})")
    }
}

/// Serialized as its textual form rather than as a struct. That keeps the
/// on-disk shape identical to the printed shape, and makes a reference usable
/// as a JSON object key in a grant ledger.
impl Serialize for SecretRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Segments are restricted to a portable identifier alphabet so that a
/// reference can be embedded in a path, a JSON key, or an environment variable
/// name without escaping, and so that no separator character can be smuggled
/// into a segment to make one reference parse as another.
fn check_segment(label: &str, segment: &str) -> Result<(), SecretError> {
    if segment.is_empty() || segment.len() > MAX_SEGMENT_BYTES {
        return Err(SecretError::Invalid(format!(
            "secret reference {label} must be 1 to {MAX_SEGMENT_BYTES} bytes"
        )));
    }
    if !segment
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err(SecretError::Invalid(format!(
            "secret reference {label} must be alphanumeric with - _ ."
        )));
    }
    Ok(())
}
