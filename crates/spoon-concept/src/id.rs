//! Identity: symbols, holes, ground values, and concept identity.
//!
//! Two rules govern this module.
//!
//! 1. Identity must be stable across process restarts. A `SymbolId` computed
//!    today must equal the one computed after a reboot, a recompile, or a
//!    different machine. That rules out `std::hash::DefaultHasher`, whose
//!    output is deliberately randomized per process. Everything here that
//!    needs persistent identity goes through blake3.
//! 2. A ground concept's identity IS its value. `Atomic(Ground(Int(42)))` is
//!    self-describing: there is nothing to look up, which is why ground
//!    concepts cost no store row until something is asserted about them.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Stable identity for a named concept.
///
/// The id is a truncated blake3 digest of the name, so it is deterministic
/// across runs and machines. No allocator, no coordination, no remapping on
/// load. Names are recoverable through [`SymbolTable`], which is a display
/// convenience, not the source of identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SymbolId(pub u64);

impl SymbolId {
    /// Compute the id for a name. Deterministic and stable forever.
    ///
    /// Names are normalized to lowercase so `Greg` and `greg` are the same
    /// concept. Surface-form casing is presentation, stored separately in the
    /// concept's metadata.
    pub fn of(name: &str) -> Self {
        let normalized = name.trim().to_lowercase();
        let digest = blake3::hash(normalized.as_bytes());
        let bytes = digest.as_bytes();
        SymbolId(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Index of a hole within a single pattern or expression.
///
/// Holes are the one shape that is not a concept: a gap inside a rule pattern
/// or a partially resolved expression. Numbering is local to the pattern that
/// contains it, so `Hole(0)` in one rule is unrelated to `Hole(0)` in another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HoleId(pub u32);

impl HoleId {
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

/// A JSON payload with a precomputed content hash.
///
/// `serde_json::Value` implements neither `Eq` nor `Hash`, and hashing a large
/// blob on every lookup would be wasteful anyway. The digest is computed once
/// at construction over the canonical serialization. `serde_json::Value` backs
/// objects with a `BTreeMap`, so key order is already canonical as long as the
/// `preserve_order` feature stays off.
///
/// Serialization goes through the bare `serde_json::Value`: the digest is
/// derived data, so writing it to disk would be redundant and, worse, would let
/// a hand-edited seed file carry a digest that disagrees with its value.
/// Deserializing reconstructs it. Skipping the field instead would leave a
/// zeroed digest after a round trip, silently breaking `Eq` and `Hash`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "serde_json::Value", into = "serde_json::Value")]
pub struct JsonBlob {
    value: serde_json::Value,
    digest: [u8; 32],
}

impl JsonBlob {
    pub fn new(value: serde_json::Value) -> Self {
        let digest = Self::digest_of(&value);
        JsonBlob { value, digest }
    }

    fn digest_of(value: &serde_json::Value) -> [u8; 32] {
        let canonical = serde_json::to_vec(value).unwrap_or_default();
        *blake3::hash(&canonical).as_bytes()
    }

    pub fn value(&self) -> &serde_json::Value {
        &self.value
    }

    pub fn into_value(self) -> serde_json::Value {
        self.value
    }

    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    /// Recompute the digest after mutating the value through some other path.
    /// Normal construction and deserialization both keep it current already.
    pub fn rehash(&mut self) {
        self.digest = Self::digest_of(&self.value);
    }
}

impl PartialEq for JsonBlob {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest && self.value == other.value
    }
}

impl Eq for JsonBlob {}

impl std::hash::Hash for JsonBlob {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.digest.hash(state);
    }
}

impl From<serde_json::Value> for JsonBlob {
    fn from(value: serde_json::Value) -> Self {
        JsonBlob::new(value)
    }
}

impl From<JsonBlob> for serde_json::Value {
    fn from(blob: JsonBlob) -> Self {
        blob.value
    }
}

/// A literal value. The identity of a ground concept.
///
/// This is not a separate "value type" sitting beside concepts. A `Ground` is
/// how an atomic concept identifies itself when its meaning is its content
/// rather than a name. `42` is `Concept::Atomic(ConceptId::Ground(Int(42)))`,
/// a full citizen that can carry realizations and participate in
/// relationships exactly like `Greg` does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Ground {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(Arc<str>),
    Bytes(Arc<[u8]>),
    DateTime(DateTime<Utc>),
    Json(Arc<JsonBlob>),
}

impl Ground {
    pub fn text(s: impl AsRef<str>) -> Self {
        Ground::Text(Arc::from(s.as_ref()))
    }

    pub fn bytes(b: impl AsRef<[u8]>) -> Self {
        Ground::Bytes(Arc::from(b.as_ref()))
    }

    pub fn json(v: serde_json::Value) -> Self {
        Ground::Json(Arc::new(JsonBlob::new(v)))
    }

    /// Short tag naming the ground kind. Used by realizations that dispatch on
    /// the shape of their arguments, and by store indexes.
    pub fn kind(&self) -> GroundKind {
        match self {
            Ground::Bool(_) => GroundKind::Bool,
            Ground::Int(_) => GroundKind::Int,
            Ground::Float(_) => GroundKind::Float,
            Ground::Text(_) => GroundKind::Text,
            Ground::Bytes(_) => GroundKind::Bytes,
            Ground::DateTime(_) => GroundKind::DateTime,
            Ground::Json(_) => GroundKind::Json,
        }
    }

    /// Numeric widening. `Int` and `Float` are distinct identities (42 is not
    /// 42.0) but arithmetic realizations need to compare across them.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Ground::Int(i) => Some(*i as f64),
            Ground::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Ground::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Ground::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Ground::Text(t) => Some(t),
            _ => None,
        }
    }

    /// Feed this value into a blake3 hasher for stable structural identity.
    /// Every arm writes a distinct discriminant byte first so that, for
    /// example, `Int(0)` and `Bool(false)` cannot collide.
    pub(crate) fn write_digest(&self, hasher: &mut blake3::Hasher) {
        match self {
            Ground::Bool(b) => {
                hasher.update(&[0x01, *b as u8]);
            }
            Ground::Int(i) => {
                hasher.update(&[0x02]);
                hasher.update(&i.to_le_bytes());
            }
            Ground::Float(f) => {
                hasher.update(&[0x03]);
                hasher.update(&float_key(*f).to_le_bytes());
            }
            Ground::Text(t) => {
                hasher.update(&[0x04]);
                hasher.update(t.as_bytes());
            }
            Ground::Bytes(b) => {
                hasher.update(&[0x05]);
                hasher.update(b);
            }
            Ground::DateTime(dt) => {
                hasher.update(&[0x06]);
                hasher.update(&dt.timestamp_nanos_opt().unwrap_or(0).to_le_bytes());
            }
            Ground::Json(j) => {
                hasher.update(&[0x07]);
                hasher.update(j.digest());
            }
        }
    }
}

/// Canonical bit pattern for float equality and hashing.
///
/// `f64` is not `Eq` because NaN != NaN and -0.0 == 0.0 while their bit
/// patterns differ. Ground identity needs a total, hashable equality, so NaN
/// collapses to one quiet-NaN pattern and -0.0 collapses to 0.0. The
/// consequence is deliberate: `Float(NaN)` is one concept, equal to itself.
fn float_key(f: f64) -> u64 {
    if f.is_nan() {
        0x7ff8_0000_0000_0000
    } else if f == 0.0 {
        0
    } else {
        f.to_bits()
    }
}

impl PartialEq for Ground {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Ground::Bool(a), Ground::Bool(b)) => a == b,
            (Ground::Int(a), Ground::Int(b)) => a == b,
            (Ground::Float(a), Ground::Float(b)) => float_key(*a) == float_key(*b),
            (Ground::Text(a), Ground::Text(b)) => a == b,
            (Ground::Bytes(a), Ground::Bytes(b)) => a == b,
            (Ground::DateTime(a), Ground::DateTime(b)) => a == b,
            (Ground::Json(a), Ground::Json(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Ground {}

impl std::hash::Hash for Ground {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Ground::Bool(b) => b.hash(state),
            Ground::Int(i) => i.hash(state),
            Ground::Float(f) => float_key(*f).hash(state),
            Ground::Text(t) => t.hash(state),
            Ground::Bytes(b) => b.hash(state),
            Ground::DateTime(d) => d.hash(state),
            Ground::Json(j) => j.hash(state),
        }
    }
}

/// Discriminant of a [`Ground`], without the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GroundKind {
    Bool,
    Int,
    Float,
    Text,
    Bytes,
    DateTime,
    Json,
}

impl GroundKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GroundKind::Bool => "bool",
            GroundKind::Int => "int",
            GroundKind::Float => "float",
            GroundKind::Text => "text",
            GroundKind::Bytes => "bytes",
            GroundKind::DateTime => "datetime",
            GroundKind::Json => "json",
        }
    }
}

/// How an atomic concept is identified.
///
/// A named concept's meaning lives in the store: surface forms, realizations,
/// relationships. The name alone tells you nothing, so it always needs a row.
///
/// A ground concept's identity is its value, so it is self-describing and
/// needs no row until something is asserted about it. `Add<42, 1>` touches the
/// store zero times; `Synonym<"forty-two", 42>` materializes a row for 42.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConceptId {
    Named(SymbolId),
    Ground(Ground),
}

impl ConceptId {
    pub fn named(name: &str) -> Self {
        ConceptId::Named(SymbolId::of(name))
    }

    pub fn is_named(&self) -> bool {
        matches!(self, ConceptId::Named(_))
    }

    pub fn is_ground(&self) -> bool {
        matches!(self, ConceptId::Ground(_))
    }

    pub fn as_symbol(&self) -> Option<SymbolId> {
        match self {
            ConceptId::Named(s) => Some(*s),
            ConceptId::Ground(_) => None,
        }
    }

    pub fn as_ground(&self) -> Option<&Ground> {
        match self {
            ConceptId::Ground(g) => Some(g),
            ConceptId::Named(_) => None,
        }
    }

    pub(crate) fn write_digest(&self, hasher: &mut blake3::Hasher) {
        match self {
            ConceptId::Named(s) => {
                hasher.update(&[0xA0]);
                hasher.update(&s.0.to_le_bytes());
            }
            ConceptId::Ground(g) => {
                hasher.update(&[0xA1]);
                g.write_digest(hasher);
            }
        }
    }
}

/// Reverse map from [`SymbolId`] back to the name it was derived from.
///
/// Identity does not depend on this table: `SymbolId::of` is a pure function.
/// The table exists so errors, logs, the inspector, and the mouth can print
/// `friend-with` instead of a 64-bit integer. It is populated as names are
/// encountered and repopulated when the store loads.
#[derive(Debug, Default)]
pub struct SymbolTable {
    names: parking_lot::RwLock<std::collections::HashMap<SymbolId, Arc<str>>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a name and return its id. Calling this is optional: the id is
    /// computable without it. Registering only makes the name printable.
    ///
    /// The casing as written is kept, because identity already ignores it and
    /// `FriendWith` reads better in a log than `friendwith`. First registration
    /// wins; later spellings of the same symbol do not overwrite it, so display
    /// stays stable within a session.
    pub fn intern(&self, name: &str) -> SymbolId {
        let id = SymbolId::of(name);
        let written = name.trim();
        let mut names = self.names.write();
        names.entry(id).or_insert_with(|| Arc::from(written));
        id
    }

    pub fn resolve(&self, id: SymbolId) -> Option<Arc<str>> {
        self.names.read().get(&id).cloned()
    }

    /// Name for display, falling back to a hex form when the symbol has never
    /// been registered in this process.
    pub fn display(&self, id: SymbolId) -> String {
        match self.resolve(id) {
            Some(name) => name.to_string(),
            None => format!("#{:016x}", id.0),
        }
    }

    pub fn len(&self) -> usize {
        self.names.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.read().is_empty()
    }

    /// Every registered (id, name) pair. Used by the store when persisting the
    /// symbol table and by the inspector.
    pub fn entries(&self) -> Vec<(SymbolId, Arc<str>)> {
        self.names
            .read()
            .iter()
            .map(|(id, name)| (*id, name.clone()))
            .collect()
    }
}
