//! Secrets and identity for Spoon.
//!
//! The subsystem is built around one rule: a secret value exists only inside
//! an authorized adapter, only for the duration of one use. Everything that
//! travels anywhere else, an episode, a receipt, a bundle, a log line, carries
//! a [`SecretRef`], which names a secret and cannot carry its value.
//!
//! The pieces fit together as a single path:
//!
//! 1. A host issues a [`SecretGrant`] into a [`GrantLedger`]. The grant names
//!    a reference, the scopes it may be used in, and when it expires.
//! 2. An adapter asks a [`SecretBroker`] for a value, naming the [`SecretUse`]
//!    it is about to perform. The broker authorizes against the ledger and
//!    only then calls a [`SecretResolver`].
//! 3. The resolver returns a [`ResolvedSecret`], which cannot be cloned or
//!    serialized, prints redacted, and zeroes its buffer on drop.
//! 4. The same value is registered with a [`Redactor`], so if it later leaks
//!    into a rendered string or a JSON receipt it is replaced with a marker
//!    that names the reference and not the value.
//!
//! This complements, rather than replaces, the bundle-import denylist in
//! `spoon-capability`: that layer refuses to let secret-shaped material cross
//! a portability boundary at all, and this layer governs the local material
//! that legitimately exists.
//!
//! # Strength of the signing implementation
//!
//! [`SigningIdentity`] is HMAC over SHA-256 (RFC 2104), a symmetric MAC. It is
//! not a digital signature. It authenticates a holder of the shared key and
//! detects tampering. It does not provide non-repudiation, because the
//! verifier holds the same key it would have needed to forge the value, and it
//! does not provide public verifiability, because there is no public half to
//! publish. It is honest for a local machine identity and it is not sufficient
//! for a publisher identity in a distributed registry, which needs an
//! asymmetric scheme. No asymmetric dependency is added here.

mod grant;
mod identity;
mod redact;
mod reference;
mod resolve;

pub use grant::{GrantLedger, GrantRecord, GrantStatus, SecretGrant, SecretScope, SecretUse};
pub use identity::{
    HMAC_SHA256_BLOCK, SIGNATURE_DOMAIN, Signature, SignatureAlgorithm, SigningIdentity,
    hmac_sha256,
};
pub use redact::{Redactor, redaction_marker};
pub use reference::{SECRET_SCHEME, SecretRef};
pub use resolve::{EnvResolver, InMemoryResolver, ResolvedSecret, SecretBroker, SecretResolver};

use spoon_capability::CapabilityError;

/// Every failure is describable without naming a secret value. The variants
/// carry references, targets, and timestamps only, so an error may be logged
/// or put in a receipt verbatim.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("secret input is not well formed: {0}")]
    Invalid(String),
    #[error("no secret grant or material is registered for {0}")]
    Unknown(SecretRef),
    #[error("{reference} is not granted for use at {use_site}")]
    OutOfScope {
        reference: SecretRef,
        use_site: SecretUse,
    },
    #[error("{reference} expired at {not_after} and the current time is {now}")]
    Expired {
        reference: SecretRef,
        not_after: i64,
        now: i64,
    },
    #[error("{reference} was superseded by version {current}")]
    Superseded { reference: SecretRef, current: u32 },
    #[error("{reference} was revoked at {at}")]
    Revoked { reference: SecretRef, at: i64 },
    #[error("environment variable {key} is not in the resolver allowlist")]
    EnvNotAllowed { key: String },
    #[error("environment variable {key} is not readable as a secret value")]
    EnvUnavailable { key: String },
    #[error("signature does not verify under identity {identity}")]
    SignatureMismatch { identity: String },
    #[error("signature names identity {found} and verification used {expected}")]
    IdentityMismatch { expected: String, found: String },
}

/// Adapters report capability failures, so a secret failure has to arrive in
/// that vocabulary. Anything that is a denial of use maps to
/// `PermissionRequired`; malformed input maps to `Invalid`.
impl From<SecretError> for CapabilityError {
    fn from(error: SecretError) -> Self {
        match error {
            SecretError::Invalid(reason) => Self::Invalid(format!("secret: {reason}")),
            other => Self::PermissionRequired(other.to_string()),
        }
    }
}

/// Overwrite a buffer that held secret material.
///
/// A plain assignment to memory that is never read again is a dead store the
/// optimizer may remove. A volatile write may not be removed, and the fence
/// keeps it from being reordered past the end of the buffer's lifetime.
///
/// This cannot recover copies made before the buffer arrived here, so callers
/// take ownership of the original allocation instead of copying into a new one
/// wherever that is possible.
pub(crate) fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // Safety: `byte` is a live, uniquely borrowed, aligned `u8`.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Lowercase hex, matching the digest form used elsewhere in the workspace.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}
