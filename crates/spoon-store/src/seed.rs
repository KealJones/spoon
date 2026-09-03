//! Git-friendly export and import.
//!
//! A seed is the whole brain as one JSON document: symbols, concepts,
//! descriptions, realizations, and assertions. Two properties make it useful
//! in a repository.
//!
//! **Deterministic.** Every collection is sorted by a total key, so exporting
//! the same brain twice produces byte-identical text and a diff shows only
//! what actually changed.
//!
//! **Idempotent.** Importing the same seed twice leaves the same database.
//! Concepts, descriptions and realizations are keyed writes, so they simply
//! overwrite. Assertions are the exception: they have a surrogate row id and
//! nothing in the schema stops the same fact being recorded twice, so import
//! checks for an identical row first. That check is deliberately at import
//! time rather than a unique constraint, because two genuinely separate
//! assertions of the same fact in the same millisecond are legal.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use spoon_concept::{Concept, ConceptMeta, Provenance, Realization, SymbolId};

use crate::assertions::write_assertion;
use crate::concepts::force_tree;
use crate::encode::{decode_concept, ts_to_sql, u64_to_sql};
use crate::error::Result;
use crate::realizations::write_realization;
use crate::schema::SCHEMA_VERSION;
use crate::symbols::write_symbol;
use crate::{Store, meta};

/// A whole brain, serialized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seed {
    /// Name of this seed. Recorded so an imported concept can be traced back
    /// to the file it came from.
    pub name: String,
    /// Schema version the seed was exported from. A seed is data rather than
    /// schema, so this is informational, but it tells a human reading a diff
    /// which build wrote the file.
    pub schema_version: u32,
    pub symbols: Vec<SeedSymbol>,
    pub concepts: Vec<Concept>,
    pub meta: Vec<ConceptMeta>,
    pub realizations: Vec<Realization>,
    pub assertions: Vec<SeedAssertion>,
}

/// A symbol id with the name it was derived from. The id is redundant (it is a
/// pure function of the name) but recording it makes a corrupted or
/// hand-edited seed detectable on import instead of silently renaming things.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedSymbol {
    pub id: SymbolId,
    pub name: String,
}

/// One assertion without its row id.
///
/// The id is a surrogate key, not identity: it means nothing outside the
/// database that issued it, so it is left out of the seed and reassigned on
/// import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeedAssertion {
    pub concept: Concept,
    pub asserted_at: DateTime<Utc>,
    pub invalidated_at: Option<DateTime<Utc>>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub provenance: Provenance,
    pub episode: Option<u64>,
    pub confidence: Option<f64>,
}

/// What an import actually changed, so a caller can report it rather than
/// guess.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportStats {
    pub symbols: usize,
    pub concepts: usize,
    pub meta: usize,
    pub realizations: usize,
    pub assertions: usize,
    /// Assertions already present byte for byte, so not written again. On a
    /// second import of the same seed this equals `seed.assertions.len()`.
    pub assertions_skipped: usize,
}

impl Store {
    /// Serialize the whole brain.
    pub fn export_seed(&self, name: &str) -> Result<Seed> {
        let mut symbols: Vec<SeedSymbol> = self
            .all_symbols()?
            .into_iter()
            .map(|(id, name)| SeedSymbol { id, name })
            .collect();
        symbols.sort_by(|a, b| (&a.name, a.id).cmp(&(&b.name, b.id)));

        let mut concepts = self.all_concepts()?;
        concepts.sort_by_key(|c| c.content_id());

        let mut meta = meta::all_meta(self)?;
        meta.sort_by_key(|m| m.concept.content_id());

        // Already name-ordered by the query, which is the stable key here.
        let realizations = self.all_realizations()?;

        let mut keyed: Vec<(String, SeedAssertion)> = crate::assertions::all_assertions(self)?
            .into_iter()
            .map(|record| {
                let seeded = SeedAssertion {
                    concept: record.concept,
                    asserted_at: record.asserted_at,
                    invalidated_at: record.invalidated_at,
                    valid_from: record.valid_from,
                    valid_to: record.valid_to,
                    provenance: record.provenance,
                    episode: record.episode,
                    confidence: record.confidence,
                };
                let key = assertion_key(&seeded)?;
                Ok((key, seeded))
            })
            .collect::<Result<Vec<_>>>()?;
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        let assertions = keyed.into_iter().map(|(_, a)| a).collect();

        Ok(Seed {
            name: name.to_string(),
            schema_version: SCHEMA_VERSION,
            symbols,
            concepts,
            meta,
            realizations,
            assertions,
        })
    }

    /// Load a seed. Safe to run on a store that already contains it.
    ///
    /// Everything lands in one transaction: a half-imported brain is worse
    /// than no import at all, because nothing downstream can tell which half
    /// it got.
    pub fn import_seed(&self, seed: &Seed) -> Result<ImportStats> {
        let mut stats = ImportStats::default();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        for symbol in &seed.symbols {
            write_symbol(&tx, &symbol.name)?;
            stats.symbols += 1;
        }
        for concept in &seed.concepts {
            // Forced, not put: a seed records the rows that exist, including
            // ground concepts that earned one, and import must reproduce them
            // exactly rather than re-apply the lazy rule.
            force_tree(&tx, concept)?;
            stats.concepts += 1;
        }
        for meta in &seed.meta {
            crate::meta::write_meta(&tx, meta)?;
            stats.meta += 1;
        }
        for realization in &seed.realizations {
            write_realization(&tx, realization)?;
            stats.realizations += 1;
        }
        for assertion in &seed.assertions {
            if assertion_exists(&tx, assertion)? {
                stats.assertions_skipped += 1;
                continue;
            }
            write_assertion(&tx, assertion)?;
            stats.assertions += 1;
        }

        tx.commit()?;
        Ok(stats)
    }

    /// Every stored concept, unordered. Seed export sorts it; the inspector
    /// pages through it.
    pub fn all_concepts(&self) -> Result<Vec<Concept>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT encoded FROM concepts")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(decode_concept("concepts", &row?)?);
        }
        Ok(out)
    }
}

/// Total ordering key for an assertion.
///
/// Content id first so a diff groups every claim about one concept together,
/// then transaction time, then the full serialization to break ties between
/// two assertions made about the same concept in the same millisecond.
fn assertion_key(a: &SeedAssertion) -> Result<String> {
    Ok(format!(
        "{}|{:020}|{}",
        a.concept.content_id().to_hex(),
        a.asserted_at.timestamp_millis(),
        serde_json::to_string(a)?
    ))
}

/// Is this exact assertion already recorded? Every column participates,
/// including both clocks, because an assertion that differs in any of them is
/// a different claim.
fn assertion_exists(tx: &rusqlite::Transaction<'_>, a: &SeedAssertion) -> Result<bool> {
    let found: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM assertions
             WHERE content_id = ?1
               AND asserted_at = ?2
               AND invalidated_at IS ?3
               AND valid_from IS ?4
               AND valid_to IS ?5
               AND provenance = ?6
               AND episode IS ?7
               AND confidence IS ?8
             LIMIT 1",
            params![
                &a.concept.content_id().0[..],
                ts_to_sql(a.asserted_at),
                a.invalidated_at.map(ts_to_sql),
                a.valid_from.map(ts_to_sql),
                a.valid_to.map(ts_to_sql),
                serde_json::to_string(&a.provenance)?,
                a.episode.map(u64_to_sql),
                a.confidence,
            ],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}
