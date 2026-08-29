use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{ResolvedSecret, SecretError, hex_encode, wipe};

/// SHA-256 compression block size in bytes, the `B` of RFC 2104.
pub const HMAC_SHA256_BLOCK: usize = 64;

/// Domain separator mixed into every signature. Without it, a MAC produced
/// here could be replayed as a MAC for some other protocol that happens to
/// share the key.
pub const SIGNATURE_DOMAIN: &str = "spoon.secret.signature.hmac-sha256.v1";

const MIN_KEY_BYTES: usize = 32;
const MAC_BYTES: usize = 32;

/// The only algorithm implemented. Present as an enum because a signature is a
/// durable artifact: a reader must be able to see which algorithm produced it
/// rather than infer it, and a future asymmetric scheme has to be
/// distinguishable from this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignatureAlgorithm {
    /// HMAC over SHA-256. Symmetric: it authenticates a holder of the shared
    /// key. It does not provide non-repudiation or public verifiability.
    HmacSha256,
}

impl SignatureAlgorithm {
    fn label(self) -> &'static str {
        match self {
            Self::HmacSha256 => "hmac-sha256",
        }
    }
}

/// A detached authentication tag over a payload.
///
/// This doubles as the authentication receipt: it records which local identity
/// authenticated the payload and when. Every field is covered by the tag, so
/// altering the recorded identity or timestamp invalidates it.
///
/// Suitable for attaching to a published artifact by a caller that owns the
/// artifact bytes, for example a forge attaching a publication signature over
/// a bundle digest. Verification requires the same shared key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    pub identity: String,
    pub algorithm: SignatureAlgorithm,
    pub signed_at: i64,
    /// Lowercase hex of the 32-byte tag. Not secret.
    pub mac: String,
}

/// A local symmetric identity: a name plus the key that speaks for it.
///
/// The key never leaves this type. There is no accessor, no `Clone`, no
/// `Serialize`, and the manual `Debug` prints a marker, so key material cannot
/// become a Spoon value, reach a receipt, or appear in a log.
pub struct SigningIdentity {
    id: String,
    key: Vec<u8>,
}

impl SigningIdentity {
    /// RFC 2104 recommends a key at least as long as the hash output, so keys
    /// shorter than 32 bytes are refused rather than silently accepted and
    /// zero-padded.
    pub fn new(id: &str, key: &[u8]) -> Result<Self, SecretError> {
        if id.is_empty()
            || id.len() > 128
            || !id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
            })
        {
            return Err(SecretError::Invalid(
                "identity id must be 1 to 128 bytes of alphanumerics with - _ . :".into(),
            ));
        }
        if key.len() < MIN_KEY_BYTES {
            return Err(SecretError::Invalid(format!(
                "identity key must be at least {MIN_KEY_BYTES} bytes"
            )));
        }
        Ok(Self {
            id: id.to_owned(),
            key: key.to_vec(),
        })
    }

    /// Build an identity from a just-in-time resolution, which is how a real
    /// host gets one: the key is a scoped, expiring secret rather than a
    /// constant compiled into the process.
    pub fn from_secret(id: &str, secret: &ResolvedSecret) -> Result<Self, SecretError> {
        Self::new(id, secret.expose())
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn sign(&self, payload: &[u8], signed_at: i64) -> Signature {
        let algorithm = SignatureAlgorithm::HmacSha256;
        let mac = self.tag(algorithm, signed_at, payload);
        Signature {
            identity: self.id.clone(),
            algorithm,
            signed_at,
            mac: hex_encode(&mac),
        }
    }

    /// Recompute the tag over the payload and the signature's own metadata,
    /// then compare in constant time. A mismatch is reported without saying
    /// which part disagreed, because the comparison result is all a caller may
    /// act on.
    pub fn verify(&self, payload: &[u8], signature: &Signature) -> Result<(), SecretError> {
        if signature.identity != self.id {
            return Err(SecretError::IdentityMismatch {
                expected: self.id.clone(),
                found: signature.identity.clone(),
            });
        }
        let mismatch = || SecretError::SignatureMismatch {
            identity: self.id.clone(),
        };
        let found = hex_decode(&signature.mac).ok_or_else(mismatch)?;
        let expected = self.tag(signature.algorithm, signature.signed_at, payload);
        if constant_time_eq(&expected, &found) {
            Ok(())
        } else {
            Err(mismatch())
        }
    }

    /// The signed message is a length-prefixed encoding of every field, not a
    /// concatenation. Concatenation would let an identity of `"a"` with a
    /// payload of `"bc"` collide with `"ab"` and `"c"`.
    fn tag(
        &self,
        algorithm: SignatureAlgorithm,
        signed_at: i64,
        payload: &[u8],
    ) -> [u8; MAC_BYTES] {
        let mut message = Vec::new();
        push_field(&mut message, SIGNATURE_DOMAIN.as_bytes());
        push_field(&mut message, self.id.as_bytes());
        push_field(&mut message, algorithm.label().as_bytes());
        push_field(&mut message, &signed_at.to_le_bytes());
        push_field(&mut message, payload);
        hmac_sha256(&self.key, &message)
    }
}

impl fmt::Debug for SigningIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "SigningIdentity {{ id: {:?}, key: [redacted] }}",
            self.id
        )
    }
}

impl Drop for SigningIdentity {
    fn drop(&mut self) {
        wipe(&mut self.key);
    }
}

/// HMAC-SHA-256 as specified by RFC 2104: a key longer than the block size is
/// hashed first, a shorter key is zero-padded to the block size, and the
/// padded key is combined with `ipad` (0x36) and `opad` (0x5c).
///
/// Public so that its output can be checked against the RFC 4231 test vectors
/// from outside the crate. Prefer [`SigningIdentity`] for signing, which adds
/// domain separation and unambiguous field encoding.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; MAC_BYTES] {
    let mut padded_key = [0u8; HMAC_SHA256_BLOCK];
    if key.len() > HMAC_SHA256_BLOCK {
        padded_key[..MAC_BYTES].copy_from_slice(Sha256::digest(key).as_slice());
    } else {
        padded_key[..key.len()].copy_from_slice(key);
    }

    let mut inner_pad = [0x36u8; HMAC_SHA256_BLOCK];
    let mut outer_pad = [0x5cu8; HMAC_SHA256_BLOCK];
    for ((inner, outer), key_byte) in inner_pad
        .iter_mut()
        .zip(outer_pad.iter_mut())
        .zip(padded_key.iter())
    {
        *inner ^= key_byte;
        *outer ^= key_byte;
    }

    let inner = Sha256::new()
        .chain_update(inner_pad)
        .chain_update(message)
        .finalize();
    let outer = Sha256::new()
        .chain_update(outer_pad)
        .chain_update(inner)
        .finalize();

    // The pads are key-derived, so they are wiped alongside anything else that
    // held the key.
    wipe(&mut padded_key);
    wipe(&mut inner_pad);
    wipe(&mut outer_pad);

    let mut tag = [0u8; MAC_BYTES];
    tag.copy_from_slice(outer.as_slice());
    tag
}

/// Comparison whose running time depends on the length of the inputs but not
/// on where they first differ, so a caller cannot learn a correct tag one byte
/// at a time by measuring rejections.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left_byte, right_byte) in left.iter().zip(right) {
        difference |= left_byte ^ right_byte;
    }
    difference == 0
}

fn push_field(message: &mut Vec<u8>, field: &[u8]) {
    let length = u64::try_from(field.len()).unwrap_or(u64::MAX);
    message.extend_from_slice(&length.to_le_bytes());
    message.extend_from_slice(field);
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks(2) {
        let high = char::from(pair[0]).to_digit(16)?;
        let low = char::from(pair[1]).to_digit(16)?;
        bytes.push(u8::try_from(high * 16 + low).ok()?);
    }
    Some(bytes)
}
