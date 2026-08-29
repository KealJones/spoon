use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{GrantLedger, Redactor, SecretError, SecretRef, SecretUse, wipe};

/// A secret value, alive for one use.
///
/// Deliberately missing: `Clone`, so a value cannot be duplicated into a
/// longer-lived place by accident; `Serialize`, so it cannot reach a receipt,
/// an episode, or a log; and a derived `Debug`, so no formatting path can
/// print it. The buffer is zeroed on drop.
///
/// The wipe covers this allocation only. Resolvers therefore take ownership of
/// the buffer they were handed rather than copying out of it, and callers
/// should pass [`expose`](Self::expose) directly to the operation that needs
/// it rather than materializing an owned copy.
pub struct ResolvedSecret {
    reference: SecretRef,
    value: Vec<u8>,
}

impl ResolvedSecret {
    /// Called by a resolver. An empty value is refused: it is always a
    /// misconfiguration, and it would make redaction of that secret a no-op.
    pub fn new(reference: SecretRef, value: Vec<u8>) -> Result<Self, SecretError> {
        if value.is_empty() {
            return Err(SecretError::Invalid(format!(
                "{reference} resolved to an empty value"
            )));
        }
        Ok(Self { reference, value })
    }

    pub fn reference(&self) -> &SecretRef {
        &self.reference
    }

    /// The one way to read the value. Named to be conspicuous at the call site
    /// and in review.
    pub fn expose(&self) -> &[u8] {
        &self.value
    }
}

impl fmt::Debug for ResolvedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "ResolvedSecret {{ reference: {}, value: {} }}",
            self.reference,
            crate::redaction_marker(&self.reference)
        )
    }
}

impl Drop for ResolvedSecret {
    fn drop(&mut self) {
        wipe(&mut self.value);
    }
}

/// The boundary between naming a secret and holding one.
///
/// An implementation is host-owned configuration. It is called just in time,
/// from inside an authorized adapter, and only through a [`SecretBroker`],
/// which is what enforces scope, expiry, and rotation. Calling a resolver
/// directly skips authorization and is only appropriate when the caller is
/// itself the authorization layer.
pub trait SecretResolver {
    fn resolve(&self, reference: &SecretRef) -> Result<ResolvedSecret, SecretError>;
}

/// A resolver holding material in process memory. Used by tests, and by hosts
/// that load secrets from a keychain at startup.
#[derive(Default)]
pub struct InMemoryResolver {
    values: BTreeMap<SecretRef, Vec<u8>>,
}

impl InMemoryResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, reference: SecretRef, value: &[u8]) -> Result<(), SecretError> {
        if value.is_empty() {
            return Err(SecretError::Invalid(format!(
                "{reference} cannot hold an empty value"
            )));
        }
        if let Some(previous) = self.values.insert(reference, value.to_vec()) {
            let mut previous = previous;
            wipe(&mut previous);
        }
        Ok(())
    }

    /// Retire material for a reference. A rotation should call this for the
    /// superseded version so the old value stops existing, while its grant
    /// record stays in the ledger.
    pub fn remove(&mut self, reference: &SecretRef) -> bool {
        match self.values.remove(reference) {
            Some(mut value) => {
                wipe(&mut value);
                true
            }
            None => false,
        }
    }
}

impl fmt::Debug for InMemoryResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "InMemoryResolver {{ entries: {} }}",
            self.values.len()
        )
    }
}

impl Drop for InMemoryResolver {
    fn drop(&mut self) {
        for value in self.values.values_mut() {
            wipe(value);
        }
    }
}

impl SecretResolver for InMemoryResolver {
    fn resolve(&self, reference: &SecretRef) -> Result<ResolvedSecret, SecretError> {
        let value = self
            .values
            .get(reference)
            .ok_or_else(|| SecretError::Unknown(reference.clone()))?;
        ResolvedSecret::new(reference.clone(), value.clone())
    }
}

/// A resolver reading from the process environment, restricted to an explicit
/// allowlist of variable names.
///
/// The variable name is derived from the reference rather than configured per
/// secret, so there is one mapping to audit instead of two. The version is
/// part of the name, so rotating to a new version reads a different variable
/// and the old value does not silently keep working.
///
/// The environment is process-global and readable by anything in the process,
/// so a value stored there is not confidential against in-process inspection.
/// The allowlist bounds which variables this code will read; it does not make
/// the environment a secure store.
#[derive(Debug, Clone)]
pub struct EnvResolver {
    allowlist: BTreeSet<String>,
}

impl EnvResolver {
    pub fn new(keys: impl IntoIterator<Item = String>) -> Result<Self, SecretError> {
        let mut allowlist = BTreeSet::new();
        for key in keys {
            if key.is_empty() || key.contains('=') || key.contains('\0') {
                return Err(SecretError::Invalid(
                    "environment allowlist entry is not a variable name".into(),
                ));
            }
            allowlist.insert(key);
        }
        if allowlist.is_empty() {
            return Err(SecretError::Invalid(
                "environment resolver requires a non-empty allowlist".into(),
            ));
        }
        Ok(Self { allowlist })
    }

    /// The variable a reference reads, for example
    /// `secret://forge/publish-key@v2` becomes `SPOON_SECRET_FORGE_PUBLISH_KEY_V2`.
    pub fn env_key(reference: &SecretRef) -> String {
        let segment = |text: &str| text.replace(['-', '.'], "_").to_ascii_uppercase();
        format!(
            "SPOON_SECRET_{}_{}_V{}",
            segment(reference.namespace()),
            segment(reference.name()),
            reference.version()
        )
    }

    pub fn allows(&self, key: &str) -> bool {
        self.allowlist.contains(key)
    }
}

impl SecretResolver for EnvResolver {
    fn resolve(&self, reference: &SecretRef) -> Result<ResolvedSecret, SecretError> {
        let key = Self::env_key(reference);
        if !self.allows(&key) {
            return Err(SecretError::EnvNotAllowed { key });
        }
        // `into_bytes` moves the string's allocation into the resolved secret
        // rather than copying it, so the wipe on drop covers the only buffer
        // this code produced.
        let value = std::env::var(&key)
            .map_err(|_| SecretError::EnvUnavailable { key: key.clone() })?
            .into_bytes();
        if value.is_empty() {
            return Err(SecretError::EnvUnavailable { key });
        }
        ResolvedSecret::new(reference.clone(), value)
    }
}

/// The adapter-facing entry point. Holds the authority record and the resolver
/// together so that no code path can obtain a value without first stating what
/// it is about to do with it.
#[derive(Debug)]
pub struct SecretBroker<R: SecretResolver> {
    ledger: GrantLedger,
    resolver: R,
}

impl<R: SecretResolver> SecretBroker<R> {
    pub fn new(ledger: GrantLedger, resolver: R) -> Self {
        Self { ledger, resolver }
    }

    pub fn ledger(&self) -> &GrantLedger {
        &self.ledger
    }

    pub fn ledger_mut(&mut self) -> &mut GrantLedger {
        &mut self.ledger
    }

    pub fn resolver_mut(&mut self) -> &mut R {
        &mut self.resolver
    }

    /// Authorize, then resolve. Authorization happens first so that a use
    /// outside scope never touches secret material at all.
    pub fn resolve(
        &self,
        reference: &SecretRef,
        use_site: &SecretUse,
        now: i64,
    ) -> Result<ResolvedSecret, SecretError> {
        self.ledger.authorize(reference, use_site, now)?;
        self.resolver.resolve(reference)
    }

    /// Resolve and arm redaction in one step, so a caller cannot end up
    /// holding a value that the receipt and log path does not know to hide.
    pub fn resolve_redacted(
        &self,
        reference: &SecretRef,
        use_site: &SecretUse,
        now: i64,
        redactor: &mut Redactor,
    ) -> Result<ResolvedSecret, SecretError> {
        let secret = self.resolve(reference, use_site, now)?;
        redactor.register(&secret);
        Ok(secret)
    }
}
