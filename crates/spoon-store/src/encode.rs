//! Turning concepts into rows and back.
//!
//! Three jobs live here: the column projection a concept produces, the
//! participant walk that makes "find everything about Greg" possible, and the
//! primitive conversions (content ids, symbol ids, timestamps) that sqlite
//! cannot express directly.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use spoon_concept::{Concept, ConceptId, ContentId, Ground, SymbolId};

use crate::error::{Result, StoreError};

/// Shape tag written to `concepts.shape`.
///
/// Only two values are ever written. Holes are rejected before they reach a
/// row (see [`StoreError::HoleNotStorable`]), so there is no third tag to
/// handle on read.
pub(crate) const SHAPE_ATOMIC: &str = "atomic";
pub(crate) const SHAPE_COMPOUND: &str = "compound";

/// Position recorded for a compound's head in `participants`.
///
/// Argument positions are 0-based, so the head needs a slot outside that
/// range. -1 keeps head and args in one table with one index instead of two.
pub(crate) const HEAD_POSITION: i64 = -1;

/// A `SymbolId` is a `u64`; sqlite integers are `i64`. The cast is a
/// reinterpretation of the same 64 bits, not a numeric conversion, so ids
/// above `i64::MAX` land as negative numbers in the database and come back
/// intact. Always pair [`symbol_to_sql`] with [`symbol_from_sql`]; treating a
/// stored symbol as a number (ordering it, summing it, comparing it to a
/// literal id printed in decimal) will be wrong for half of all symbols.
pub(crate) fn symbol_to_sql(id: SymbolId) -> i64 {
    id.as_u64() as i64
}

pub(crate) fn symbol_from_sql(raw: i64) -> SymbolId {
    SymbolId(raw as u64)
}

/// Episode numbers get the same bit-cast treatment as symbols, for the same
/// reason: sqlite has no unsigned integer type.
pub(crate) fn u64_to_sql(v: u64) -> i64 {
    v as i64
}

pub(crate) fn u64_from_sql(raw: i64) -> u64 {
    raw as u64
}

pub(crate) fn ts_to_sql(at: DateTime<Utc>) -> i64 {
    at.timestamp_millis()
}

pub(crate) fn ts_from_sql(table: &'static str, raw: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp_millis(raw)
        .ok_or_else(|| StoreError::corrupt(table, format!("timestamp {raw} out of range")))
}

pub(crate) fn ts_from_sql_opt(
    table: &'static str,
    raw: Option<i64>,
) -> Result<Option<DateTime<Utc>>> {
    match raw {
        Some(ms) => Ok(Some(ts_from_sql(table, ms)?)),
        None => Ok(None),
    }
}

pub(crate) fn content_id_from_sql(table: &'static str, raw: &[u8]) -> Result<ContentId> {
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| StoreError::corrupt(table, format!("content id is {} bytes", raw.len())))?;
    Ok(ContentId(bytes))
}

/// Whether this concept earns a row of its own when it is merely mentioned.
///
/// Named atomics and compounds do: a name means nothing without the store, and
/// a compound is the assertion itself. Ground atomics do not: `42` is
/// self-describing, so evaluating `Add<42, 1>` must not write two rows nobody
/// asked for. A ground concept still gets a row the moment something is
/// asserted *about* it, which goes through the forcing path instead.
pub(crate) fn is_storable(c: &Concept) -> bool {
    match c {
        Concept::Atomic(ConceptId::Named(_)) => true,
        Concept::Atomic(ConceptId::Ground(_)) => false,
        Concept::Compound { .. } => true,
        Concept::Hole(_) => false,
    }
}

/// The columns `concepts` needs for one concept.
pub(crate) struct ConceptRow {
    pub content_id: ContentId,
    pub shape: &'static str,
    pub symbol: Option<i64>,
    pub ground_kind: Option<String>,
    pub ground_json: Option<String>,
    pub head_symbol: Option<i64>,
    pub head_id: Option<Vec<u8>>,
    pub arity: i64,
    pub encoded: String,
}

impl ConceptRow {
    pub(crate) fn build(c: &Concept) -> Result<ConceptRow> {
        reject_non_finite(c)?;
        let encoded = serde_json::to_string(c)?;
        let content_id = c.content_id();
        match c {
            Concept::Atomic(ConceptId::Named(sym)) => Ok(ConceptRow {
                content_id,
                shape: SHAPE_ATOMIC,
                symbol: Some(symbol_to_sql(*sym)),
                ground_kind: None,
                ground_json: None,
                head_symbol: None,
                head_id: None,
                arity: 0,
                encoded,
            }),
            Concept::Atomic(ConceptId::Ground(g)) => Ok(ConceptRow {
                content_id,
                shape: SHAPE_ATOMIC,
                symbol: None,
                ground_kind: Some(g.kind().as_str().to_string()),
                ground_json: Some(serde_json::to_string(g)?),
                head_symbol: None,
                head_id: None,
                arity: 0,
                encoded,
            }),
            Concept::Compound { head, args } => Ok(ConceptRow {
                content_id,
                shape: SHAPE_COMPOUND,
                symbol: None,
                ground_kind: None,
                ground_json: None,
                head_symbol: c.head_symbol().map(symbol_to_sql),
                head_id: Some(head.content_id().0.to_vec()),
                arity: args.len() as i64,
                encoded,
            }),
            Concept::Hole(_) => Err(StoreError::HoleNotStorable),
        }
    }
}

/// One entry of the participant index.
pub(crate) struct Participant {
    pub id: ContentId,
    /// Index of the top-level argument whose subtree contains this
    /// participant, or [`HEAD_POSITION`] when it sits under the head. Deep
    /// participants keep the top-level position so a query can ask "concepts
    /// where Greg appears in argument 0" without walking the tree again.
    pub position: i64,
    /// Nesting distance from the containing concept. Direct arguments are 1.
    pub depth: i64,
}

/// Every concept reachable inside `c`, flattened.
///
/// Ground participants are included: `Add<42, 1>` writes no row for 42, but
/// once the compound itself is stored, 42 is findable through it. Holes are
/// excluded because hole numbering is local to the pattern that contains it,
/// so a shared index on `Hole(0)` would join unrelated rules together.
///
/// A participant appearing at several depths under the same top-level position
/// is recorded once, at its shallowest depth, which is the one a caller cares
/// about.
pub(crate) fn participants(c: &Concept) -> Vec<Participant> {
    let mut shallowest: HashMap<(ContentId, i64), i64> = HashMap::new();
    if let Concept::Compound { head, args } = c {
        walk(head, HEAD_POSITION, 1, &mut shallowest);
        for (index, arg) in args.iter().enumerate() {
            walk(arg, index as i64, 1, &mut shallowest);
        }
    }
    let mut out: Vec<Participant> = shallowest
        .into_iter()
        .map(|((id, position), depth)| Participant {
            id,
            position,
            depth,
        })
        .collect();
    // Deterministic insert order keeps write behaviour reproducible, which
    // matters when a test diffs two databases built the same way.
    out.sort_by(|a, b| (a.position, a.id, a.depth).cmp(&(b.position, b.id, b.depth)));
    out
}

fn walk(
    c: &Concept,
    position: i64,
    depth: i64,
    shallowest: &mut HashMap<(ContentId, i64), i64>,
) {
    if c.is_hole() {
        return;
    }
    let key = (c.content_id(), position);
    shallowest
        .entry(key)
        .and_modify(|d| {
            if depth < *d {
                *d = depth;
            }
        })
        .or_insert(depth);
    if let Concept::Compound { head, args } = c {
        walk(head, position, depth + 1, shallowest);
        for arg in args.iter() {
            walk(arg, position, depth + 1, shallowest);
        }
    }
}

/// JSON cannot represent NaN or infinity. `serde_json` writes them as `null`,
/// which then fails to parse back into an `f64`, so a store that accepted one
/// would hand out a row that can never be read again. Refusing at write time
/// keeps the failure where the caller can still see what caused it.
pub(crate) fn reject_non_finite(c: &Concept) -> Result<()> {
    match c {
        Concept::Atomic(ConceptId::Ground(Ground::Float(f))) if !f.is_finite() => {
            Err(StoreError::NonFiniteFloat { value: *f })
        }
        Concept::Compound { head, args } => {
            reject_non_finite(head)?;
            for arg in args.iter() {
                reject_non_finite(arg)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(crate) fn decode_concept(table: &'static str, encoded: &str) -> Result<Concept> {
    serde_json::from_str(encoded)
        .map_err(|e| StoreError::corrupt(table, format!("undecodable concept: {e}")))
}
