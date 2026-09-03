//! Belief over time.
//!
//! Two clocks run here. *Transaction time* (`asserted_at`, `invalidated_at`)
//! is when the system believed something. *Valid time* (`valid_from`,
//! `valid_to`) is when the thing was actually the case. They are independent:
//! learning today that Greg worked at Workiva in 2019 is a row asserted now
//! with a validity window in the past.
//!
//! Nothing is deleted. Retracting sets `invalidated_at`, so the history stays
//! intact: something that was true does not become false merely because it is
//! no longer true now.
//!
//! Both clocks are stored as milliseconds since the epoch. A timestamp read
//! back is therefore the millisecond truncation of the one written, which is
//! finer than any resolution the rest of the system reasons about and keeps
//! the columns comparable with plain integer predicates.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction, params};
use spoon_concept::{Concept, Provenance};

use crate::Store;
use crate::concepts::force_tree;
use crate::encode::{
    decode_concept, ts_from_sql, ts_from_sql_opt, ts_to_sql, u64_from_sql, u64_to_sql,
};
use crate::error::Result;
use crate::seed::SeedAssertion;

/// Row id of one assertion. Assertions are the one thing in the store with a
/// surrogate key: the same concept can be asserted many times, by different
/// sources, over different validity windows, and each of those is a separate
/// fact about the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssertionId(pub i64);

/// One recorded belief, with both clocks and its evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct AssertionRecord {
    pub id: AssertionId,
    pub concept: Concept,
    pub asserted_at: DateTime<Utc>,
    /// `None` while the belief still stands.
    pub invalidated_at: Option<DateTime<Utc>>,
    /// `None` means "as far back as anyone knows".
    pub valid_from: Option<DateTime<Utc>>,
    /// `None` means "still the case".
    pub valid_to: Option<DateTime<Utc>>,
    pub provenance: Provenance,
    pub episode: Option<u64>,
    pub confidence: Option<f64>,
}

pub(crate) const ASSERTION_COLUMNS: &str =
    "id, asserted_at, invalidated_at, valid_from, valid_to, provenance, episode, confidence";

/// Raw column values, before the types that sqlite cannot hold are rebuilt.
pub(crate) type AssertionTuple = (
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    String,
    Option<i64>,
    Option<f64>,
);

pub(crate) fn read_assertion_tuple(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<AssertionTuple> {
    Ok((
        row.get(offset)?,
        row.get(offset + 1)?,
        row.get(offset + 2)?,
        row.get(offset + 3)?,
        row.get(offset + 4)?,
        row.get(offset + 5)?,
        row.get(offset + 6)?,
        row.get(offset + 7)?,
    ))
}

pub(crate) fn build_record(tuple: AssertionTuple, concept: Concept) -> Result<AssertionRecord> {
    let (id, asserted_at, invalidated_at, valid_from, valid_to, provenance, episode, confidence) =
        tuple;
    Ok(AssertionRecord {
        id: AssertionId(id),
        concept,
        asserted_at: ts_from_sql("assertions", asserted_at)?,
        invalidated_at: ts_from_sql_opt("assertions", invalidated_at)?,
        valid_from: ts_from_sql_opt("assertions", valid_from)?,
        valid_to: ts_from_sql_opt("assertions", valid_to)?,
        provenance: serde_json::from_str(&provenance)?,
        episode: episode.map(u64_from_sql),
        confidence,
    })
}

impl Store {
    /// Assert that this concept holds, starting now and with no end.
    ///
    /// Asserting forces a row for the concept even when it is a bare ground
    /// value, because saying something about `42` is exactly what turns 42
    /// into a stored concept.
    pub fn assert_concept(
        &self,
        c: &Concept,
        provenance: Provenance,
        episode: Option<u64>,
        confidence: Option<f64>,
    ) -> Result<AssertionId> {
        self.assert_concept_during(c, provenance, episode, confidence, None, None)
    }

    /// Assert with an explicit validity window. `valid_from = None` means "as
    /// far back as anyone knows", `valid_to = None` means "still the case".
    ///
    /// This is the general form; [`Store::assert_concept`] is the common case
    /// where the window is unbounded.
    pub fn assert_concept_during(
        &self,
        c: &Concept,
        provenance: Provenance,
        episode: Option<u64>,
        confidence: Option<f64>,
        valid_from: Option<DateTime<Utc>>,
        valid_to: Option<DateTime<Utc>>,
    ) -> Result<AssertionId> {
        let record = SeedAssertion {
            concept: c.clone(),
            asserted_at: Utc::now(),
            invalidated_at: None,
            valid_from,
            valid_to,
            provenance,
            episode,
            confidence,
        };
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let id = write_assertion(&tx, &record)?;
        tx.commit()?;
        Ok(id)
    }

    /// Stop believing an assertion, as of `at`. Returns false when the id is
    /// unknown or was already retracted.
    ///
    /// The row survives with its `invalidated_at` set, so a query about an
    /// earlier instant still sees the belief that was held then.
    pub fn retract(&self, id: AssertionId, at: DateTime<Utc>) -> Result<bool> {
        let conn = self.conn.lock();
        let changed = conn.execute(
            "UPDATE assertions SET invalidated_at = ?1
             WHERE id = ?2 AND invalidated_at IS NULL",
            params![ts_to_sql(at), id.0],
        )?;
        Ok(changed > 0)
    }

    /// Assertions about this concept that are still believed, whatever their
    /// validity window says. Use this to see the evidence; use
    /// [`Store::holds`] to ask whether the concept is true right now.
    ///
    /// This is [`Store::assertions_at`] with `when` set to now, so the two can
    /// never disagree about what counts as believed.
    pub fn live_assertions(&self, c: &Concept) -> Result<Vec<AssertionRecord>> {
        self.assertions_at(c, Utc::now())
    }

    /// What was believed about this concept at a past instant.
    ///
    /// This is transaction time only: an assertion counts if it had been made
    /// by `when` and had not been retracted by `when`.
    pub fn assertions_at(&self, c: &Concept, when: DateTime<Utc>) -> Result<Vec<AssertionRecord>> {
        let ms = ts_to_sql(when);
        self.query_assertions(
            c,
            "SELECT {cols} FROM assertions
             WHERE content_id = ?1
               AND asserted_at <= ?2
               AND (invalidated_at IS NULL OR invalidated_at > ?2)
             ORDER BY asserted_at, id",
            &[ms],
        )
    }

    /// Is this concept true right now: believed, and inside its validity
    /// window.
    pub fn holds(&self, c: &Concept) -> Result<bool> {
        let now = ts_to_sql(Utc::now());
        let conn = self.conn.lock();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM assertions
                 WHERE content_id = ?1
                   AND (invalidated_at IS NULL OR invalidated_at > ?2)
                   AND (valid_from IS NULL OR valid_from <= ?2)
                   AND (valid_to IS NULL OR valid_to > ?2)
                 LIMIT 1",
                params![&c.content_id().0[..], now],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    fn query_assertions(
        &self,
        c: &Concept,
        sql: &str,
        extra: &[i64],
    ) -> Result<Vec<AssertionRecord>> {
        let sql = sql.replace("{cols}", ASSERTION_COLUMNS);
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let id = c.content_id();
        let key: &[u8] = &id.0[..];
        let mut binds: Vec<&dyn rusqlite::ToSql> = vec![&key];
        for value in extra {
            binds.push(value);
        }
        let rows = stmt.query_map(binds.as_slice(), |row| read_assertion_tuple(row, 0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(build_record(row?, c.clone())?);
        }
        Ok(out)
    }
}

/// Read every assertion in the database, newest key order last, for seed
/// export. Kept here so the assertion column list has one owner.
pub(crate) fn all_assertions(store: &Store) -> Result<Vec<AssertionRecord>> {
    let conn = store.conn.lock();
    let sql = format!(
        "SELECT {}, c.encoded FROM assertions a
         JOIN concepts c ON c.content_id = a.content_id
         ORDER BY a.id",
        ASSERTION_COLUMNS
            .split(", ")
            .map(|c| format!("a.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        let tuple = read_assertion_tuple(row, 0)?;
        let encoded: String = row.get(8)?;
        Ok((tuple, encoded))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (tuple, encoded) = row?;
        let concept = decode_concept("concepts", &encoded)?;
        out.push(build_record(tuple, concept)?);
    }
    Ok(out)
}

/// Insert one assertion exactly as described, preserving both clocks.
///
/// Seed import needs to replay historical timestamps rather than stamping
/// everything with the import time, so the timestamps are inputs here rather
/// than being taken from the clock.
pub(crate) fn write_assertion(tx: &Transaction<'_>, a: &SeedAssertion) -> Result<AssertionId> {
    force_tree(tx, &a.concept)?;
    tx.execute(
        "INSERT INTO assertions
            (content_id, asserted_at, invalidated_at, valid_from, valid_to,
             provenance, episode, confidence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
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
    )?;
    Ok(AssertionId(tx.last_insert_rowid()))
}
