//! Persisted Intent Catalog: durable mapping from stable intent keys
//! (`"arithmetic.multiply"`) to bound procedures and the surface patterns
//! that have proven, by repeated independent evidence, that they mean that
//! intent. See `docs/superpowers/specs/2026-08-28-front-language-analysis-design.md`,
//! "Intent Catalog".
//!
//! Keys are stable strings, never database UUIDs, so training and export
//! artifacts built from this table stay portable across instances.

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use spoon_core::Lifecycle;
use unicode_normalization::UnicodeNormalization;

use crate::EngineError;

/// Patterns per key. Beyond this the store evicts the weakest `Provisional`
/// candidate rather than growing unboundedly from noisy phrasing.
pub const MAX_PATTERNS_PER_KEY: usize = 16;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentSlotSchema {
    pub name: String,
    pub required: bool,
    pub value_kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentCatalogEntry {
    pub key: String,
    pub slots: Vec<IntentSlotSchema>,
    pub concept_id: Option<String>,
    pub procedure_id: Option<String>,
    pub procedure_version: Option<u32>,
    pub lifecycle: Lifecycle,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentCatalogPattern {
    pub key: String,
    pub skeleton: String,
    pub pattern: String,
    pub support: u32,
    pub contradictions: u32,
    pub lifecycle: Lifecycle,
    pub first_episode: String,
    pub last_episode: String,
}

/// What happened as a result of `admit_pattern`. Every branch of the pattern
/// lifecycle in the design doc has a distinct variant so callers (and tests)
/// can assert on the exact transition rather than re-deriving it from state.
#[derive(Debug, Clone, PartialEq)]
pub enum PatternAdmission {
    /// First time this skeleton has been seen for this key.
    Admitted,
    /// A distinct episode repeated an already-known skeleton, but it has not
    /// yet earned promotion (already `Active`, or still short of support 2).
    SupportIncremented { support: u32 },
    /// Support crossed the promotion threshold with no outstanding
    /// contradictions: the pattern may now drive interpreter-off matching.
    Promoted { support: u32 },
    /// The same episode admitted this skeleton before. Re-dispatch inside a
    /// single cycle must not inflate support.
    AlreadyCounted,
    /// The key's pattern cap is full of patterns this store will not evict.
    Refused { reason: String },
    /// The cap was full; the weakest `Provisional` pattern was evicted to
    /// make room for the new one.
    Evicted { evicted_skeleton: String },
}

pub struct IntentCatalogStore {
    conn: Connection,
}

impl IntentCatalogStore {
    pub fn open(path: &str) -> Result<Self, EngineError> {
        let store = Self {
            conn: Connection::open(path)?,
        };
        store.schema()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.schema()?;
        Ok(store)
    }

    fn schema(&self) -> Result<(), EngineError> {
        self.conn.execute_batch(
            "PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS intent_catalog_entry (
                key            TEXT PRIMARY KEY,
                slots          TEXT NOT NULL,
                concept_id     TEXT,
                procedure_id   TEXT,
                procedure_ver  INTEGER,
                lifecycle      TEXT NOT NULL,
                created_at     INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS intent_catalog_pattern (
                key            TEXT NOT NULL REFERENCES intent_catalog_entry(key),
                skeleton       TEXT NOT NULL,
                pattern        TEXT NOT NULL,
                support        INTEGER NOT NULL,
                contradictions INTEGER NOT NULL,
                lifecycle      TEXT NOT NULL,
                first_episode  TEXT NOT NULL,
                last_episode   TEXT NOT NULL,
                PRIMARY KEY (key, skeleton)
            );
            CREATE INDEX IF NOT EXISTS idx_intent_catalog_pattern_skeleton
                ON intent_catalog_pattern(skeleton, lifecycle);",
        )?;
        Ok(())
    }

    pub fn upsert_entry(&self, entry: &IntentCatalogEntry) -> Result<(), EngineError> {
        let slots_json = serde_json::to_string(&entry.slots)?;
        let lifecycle_json = serde_json::to_string(&entry.lifecycle)?;
        self.conn.execute(
            "INSERT INTO intent_catalog_entry
                (key, slots, concept_id, procedure_id, procedure_ver, lifecycle, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(key) DO UPDATE SET
                slots = excluded.slots,
                concept_id = excluded.concept_id,
                procedure_id = excluded.procedure_id,
                procedure_ver = excluded.procedure_ver,
                lifecycle = excluded.lifecycle",
            params![
                entry.key,
                slots_json,
                entry.concept_id,
                entry.procedure_id,
                entry.procedure_version,
                lifecycle_json,
                entry.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_entry(&self, key: &str) -> Result<Option<IntentCatalogEntry>, EngineError> {
        self.conn
            .query_row(
                "SELECT key, slots, concept_id, procedure_id, procedure_ver, lifecycle, created_at
                 FROM intent_catalog_entry WHERE key = ?1",
                params![key],
                decode_entry_row,
            )
            .optional()
            .map_err(EngineError::from)
    }

    pub fn list_entries(&self, limit: usize) -> Result<Vec<IntentCatalogEntry>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT key, slots, concept_id, procedure_id, procedure_ver, lifecycle, created_at
             FROM intent_catalog_entry ORDER BY key ASC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit as i64], decode_entry_row)?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }

    pub fn bind_procedure(
        &self,
        key: &str,
        concept_id: &str,
        procedure_id: &str,
        version: u32,
    ) -> Result<(), EngineError> {
        let changed = self.conn.execute(
            "UPDATE intent_catalog_entry
             SET concept_id = ?2, procedure_id = ?3, procedure_ver = ?4
             WHERE key = ?1",
            params![key, concept_id, procedure_id, version],
        )?;
        if changed == 0 {
            return Err(EngineError::InvalidInput(format!(
                "intent catalog entry {key:?} does not exist"
            )));
        }
        Ok(())
    }

    /// Admits (or re-counts, or evicts for) a surface pattern under `key`.
    /// The read-modify-write on `support`/`contradictions` runs inside an
    /// `Immediate` transaction so concurrent admissions never race on the
    /// same skeleton.
    pub fn admit_pattern(
        &self,
        key: &str,
        pattern: &str,
        episode: &str,
    ) -> Result<PatternAdmission, EngineError> {
        let entry = self.get_entry(key)?.ok_or_else(|| {
            EngineError::InvalidInput(format!("intent catalog entry {key:?} does not exist"))
        })?;
        let skeleton = normalize_skeleton(pattern, &entry.slots)?;

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let existing = tx
            .query_row(
                "SELECT support, contradictions, lifecycle, last_episode
                 FROM intent_catalog_pattern WHERE key = ?1 AND skeleton = ?2",
                params![key, skeleton],
                decode_pattern_counts_row,
            )
            .optional()?;

        let admission = if let Some((support, contradictions, lifecycle, last_episode)) = existing {
            if last_episode == episode {
                PatternAdmission::AlreadyCounted
            } else {
                let new_support = support + 1;
                let was_active = lifecycle == Lifecycle::Active;
                let new_lifecycle = if contradictions == 0 && new_support >= 2 {
                    Lifecycle::Active
                } else {
                    lifecycle
                };
                tx.execute(
                    "UPDATE intent_catalog_pattern
                     SET support = ?3, lifecycle = ?4, last_episode = ?5
                     WHERE key = ?1 AND skeleton = ?2",
                    params![
                        key,
                        skeleton,
                        new_support,
                        serde_json::to_string(&new_lifecycle)?,
                        episode,
                    ],
                )?;
                if !was_active && new_lifecycle == Lifecycle::Active {
                    PatternAdmission::Promoted {
                        support: new_support,
                    }
                } else {
                    PatternAdmission::SupportIncremented {
                        support: new_support,
                    }
                }
            }
        } else {
            let pattern_count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM intent_catalog_pattern WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )?;
            let mut eviction = None;
            if pattern_count as usize >= MAX_PATTERNS_PER_KEY {
                let candidate = tx
                    .query_row(
                        "SELECT skeleton FROM intent_catalog_pattern
                         WHERE key = ?1 AND lifecycle = ?2
                         ORDER BY support ASC, first_episode ASC, skeleton ASC LIMIT 1",
                        params![key, serde_json::to_string(&Lifecycle::Provisional)?],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                match candidate {
                    Some(evicted_skeleton) => {
                        tx.execute(
                            "DELETE FROM intent_catalog_pattern WHERE key = ?1 AND skeleton = ?2",
                            params![key, evicted_skeleton],
                        )?;
                        eviction = Some(evicted_skeleton);
                    }
                    None => {
                        tx.commit()?;
                        return Ok(PatternAdmission::Refused {
                            reason: format!(
                                "intent catalog key {key:?} is at its {MAX_PATTERNS_PER_KEY}-pattern cap and every pattern is Active"
                            ),
                        });
                    }
                }
            }
            tx.execute(
                "INSERT INTO intent_catalog_pattern
                    (key, skeleton, pattern, support, contradictions, lifecycle, first_episode, last_episode)
                 VALUES (?1, ?2, ?3, 1, 0, ?4, ?5, ?5)",
                params![
                    key,
                    skeleton,
                    pattern,
                    serde_json::to_string(&Lifecycle::Provisional)?,
                    episode,
                ],
            )?;
            match eviction {
                Some(evicted_skeleton) => PatternAdmission::Evicted { evicted_skeleton },
                None => PatternAdmission::Admitted,
            }
        };
        tx.commit()?;
        Ok(admission)
    }

    /// Increments `contradictions` for the pattern, dropping it out of the
    /// active matching set at 1 and retiring it at 2. Runs inside an
    /// `Immediate` transaction for the same read-modify-write reason as
    /// `admit_pattern`.
    pub fn record_contradiction(&self, key: &str, skeleton: &str) -> Result<(), EngineError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let contradictions: Option<u32> = tx
            .query_row(
                "SELECT contradictions FROM intent_catalog_pattern
                 WHERE key = ?1 AND skeleton = ?2",
                params![key, skeleton],
                |row| row.get(0),
            )
            .optional()?;
        let Some(contradictions) = contradictions else {
            return Err(EngineError::InvalidInput(format!(
                "no intent catalog pattern for key {key:?} with skeleton {skeleton:?}"
            )));
        };
        let new_contradictions = contradictions + 1;
        let new_lifecycle = if new_contradictions >= 2 {
            Lifecycle::Retired
        } else {
            Lifecycle::UnderReview
        };
        tx.execute(
            "UPDATE intent_catalog_pattern
             SET contradictions = ?3, lifecycle = ?4
             WHERE key = ?1 AND skeleton = ?2",
            params![
                key,
                skeleton,
                new_contradictions,
                serde_json::to_string(&new_lifecycle)?,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Only `Active` patterns may drive interpreter-off local matching: one
    /// lucky dispatch, or a pattern under review after a contradiction, is
    /// not enough to become weaning fuel.
    pub fn matching_patterns(
        &self,
        skeleton: &str,
    ) -> Result<Vec<IntentCatalogPattern>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT key, skeleton, pattern, support, contradictions, lifecycle, first_episode, last_episode
             FROM intent_catalog_pattern
             WHERE skeleton = ?1 AND lifecycle = ?2
             ORDER BY key ASC",
        )?;
        let rows = statement.query_map(
            params![skeleton, serde_json::to_string(&Lifecycle::Active)?],
            decode_pattern_row,
        )?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }

    /// All patterns for a key regardless of lifecycle, for inspection and
    /// tests. `matching_patterns` is the one that gates real dispatch.
    pub fn list_patterns(&self, key: &str) -> Result<Vec<IntentCatalogPattern>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT key, skeleton, pattern, support, contradictions, lifecycle, first_episode, last_episode
             FROM intent_catalog_pattern
             WHERE key = ?1
             ORDER BY support DESC, skeleton ASC",
        )?;
        let rows = statement.query_map(params![key], decode_pattern_row)?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }
}

fn decode_entry_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IntentCatalogEntry> {
    let slots: String = row.get(1)?;
    let lifecycle: String = row.get(5)?;
    Ok(IntentCatalogEntry {
        key: row.get(0)?,
        slots: serde_json::from_str(&slots).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        concept_id: row.get(2)?,
        procedure_id: row.get(3)?,
        procedure_version: row.get(4)?,
        lifecycle: serde_json::from_str(&lifecycle).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        created_at: row.get(6)?,
    })
}

fn decode_pattern_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IntentCatalogPattern> {
    let lifecycle: String = row.get(5)?;
    Ok(IntentCatalogPattern {
        key: row.get(0)?,
        skeleton: row.get(1)?,
        pattern: row.get(2)?,
        support: row.get(3)?,
        contradictions: row.get(4)?,
        lifecycle: serde_json::from_str(&lifecycle).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        first_episode: row.get(6)?,
        last_episode: row.get(7)?,
    })
}

/// Just the columns `admit_pattern` needs to decide the next transition:
/// support, contradictions, lifecycle, last_episode.
fn decode_pattern_counts_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(u32, u32, Lifecycle, String)> {
    let lifecycle: String = row.get(2)?;
    Ok((
        row.get(0)?,
        row.get(1)?,
        serde_json::from_str(&lifecycle).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        row.get(3)?,
    ))
}

/// Deterministic skeleton normalization, no lexicon required. See the design
/// doc's "Skeleton normalization" section for the exact ordered steps this
/// mirrors: NFKC, lowercase, positional slot substitution, whitespace
/// collapse, then edge punctuation strip.
pub fn normalize_skeleton(
    pattern: &str,
    slots: &[IntentSlotSchema],
) -> Result<String, EngineError> {
    let nfkc: String = pattern.nfkc().collect();
    // The workspace has no dedicated Unicode "simple case fold" crate; std's
    // `to_lowercase` is deterministic and lexicon-free, which is what this
    // step actually requires.
    let lowered = nfkc.to_lowercase();
    let replaced = replace_slot_placeholders(&lowered, slots)?;
    let collapsed = collapse_whitespace(&replaced);
    Ok(strip_edge_punctuation(&collapsed))
}

fn replace_slot_placeholders(
    input: &str,
    slots: &[IntentSlotSchema],
) -> Result<String, EngineError> {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(open) = rest.find('{') {
        output.push_str(&rest[..open]);
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            return Err(EngineError::InvalidInput(format!(
                "unterminated slot placeholder in pattern: {input:?}"
            )));
        };
        let name = &after_open[..close];
        let index = slots
            .iter()
            .position(|slot| slot.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                EngineError::InvalidInput(format!("pattern references unknown slot {name:?}"))
            })?;
        output.push('{');
        output.push_str(&index.to_string());
        output.push('}');
        rest = &after_open[close + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Curly braces are placeholder syntax, not sentence punctuation, so they
/// are never candidates for stripping even when they land at an edge (a
/// pattern can be nothing but a swapped argument pair, e.g. `"{1} times
/// {0}"`).
fn strip_edge_punctuation(input: &str) -> String {
    let is_strippable =
        |c: char| c != '{' && c != '}' && !c.is_alphanumeric() && !c.is_whitespace();
    input
        .trim_start_matches(is_strippable)
        .trim_end_matches(is_strippable)
        .to_string()
}
