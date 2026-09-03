//! Writing and finding concepts.

use chrono::Utc;
use rusqlite::{OptionalExtension, Transaction, params};
use spoon_concept::{Concept, ContentId, SymbolId};

use crate::Store;
use crate::encode::{
    ConceptRow, decode_concept, is_storable, participants, symbol_to_sql, ts_to_sql,
};
use crate::error::{Result, StoreError};

impl Store {
    /// Record that this concept exists, with its participant index.
    ///
    /// Idempotent: storing the same concept twice leaves one row, and the
    /// original `created_at` stands, because the first time the system saw
    /// something is the interesting fact.
    ///
    /// A bare ground atomic writes nothing and returns its content id anyway.
    /// That is the whole point: `42` is self-describing, so nothing needs to be
    /// looked up and nothing needs to be written. Ground values appearing
    /// *inside* a stored compound are still indexed as participants, so
    /// `concepts_containing(42)` finds them.
    ///
    /// Named atomics reachable inside the concept get rows of their own, since
    /// a name with no row is a name nothing can ever be learned about.
    ///
    /// A bare hole is an error rather than a no-op: holes are the one shape
    /// that is not a concept.
    pub fn put_concept(&self, c: &Concept) -> Result<ContentId> {
        if c.is_hole() {
            return Err(StoreError::HoleNotStorable);
        }
        if !is_storable(c) {
            // Validate anyway, so a caller that later asserts something about
            // this value hits the same error now rather than then.
            return Ok(c.content_id());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let id = write_tree(&tx, c)?;
        tx.commit()?;
        Ok(id)
    }

    /// Force a row for this concept even when it is a bare ground value.
    ///
    /// This is the "something is being said about it" path: an assertion or a
    /// piece of metadata targeting `42` makes 42 a first-class citizen with a
    /// row, indexes, and everything else a named concept gets.
    pub fn materialize(&self, c: &Concept) -> Result<ContentId> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let id = force_tree(&tx, c)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn get_concept(&self, id: ContentId) -> Result<Option<Concept>> {
        let conn = self.conn.lock();
        let encoded: Option<String> = conn
            .query_row(
                "SELECT encoded FROM concepts WHERE content_id = ?1",
                params![&id.0[..]],
                |row| row.get(0),
            )
            .optional()?;
        match encoded {
            Some(text) => Ok(Some(decode_concept("concepts", &text)?)),
            None => Ok(None),
        }
    }

    pub fn has_concept(&self, id: ContentId) -> Result<bool> {
        let conn = self.conn.lock();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM concepts WHERE content_id = ?1",
                params![&id.0[..]],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Every stored compound whose head is this symbol. "Find all
    /// friendships."
    ///
    /// Ordered oldest first, with the content id breaking ties so the result
    /// is stable across runs even when a batch of concepts lands in the same
    /// millisecond.
    pub fn concepts_by_head(&self, head: SymbolId, limit: usize) -> Result<Vec<Concept>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT encoded FROM concepts
             WHERE head_symbol = ?1
             ORDER BY created_at, content_id
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![symbol_to_sql(head), limit as i64], |row| {
            row.get::<_, String>(0)
        })?;
        collect_concepts(rows)
    }

    /// Every stored concept that mentions this one anywhere inside it. "Find
    /// everything about Greg."
    ///
    /// Includes nested mentions: `Stated<Keal, IsSad<Greg>>` is found by a
    /// search for Greg, not only by a search for `IsSad<Greg>`.
    pub fn concepts_containing(
        &self,
        participant: ContentId,
        limit: usize,
    ) -> Result<Vec<Concept>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT c.encoded, c.created_at, c.content_id FROM participants p
             JOIN concepts c ON c.content_id = p.content_id
             WHERE p.participant_id = ?1
             GROUP BY c.content_id
             ORDER BY c.created_at, c.content_id
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![&participant.0[..], limit as i64], |row| {
            row.get::<_, String>(0)
        })?;
        collect_concepts(rows)
    }

    pub fn count_concepts(&self) -> Result<usize> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM concepts", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

fn collect_concepts<I>(rows: I) -> Result<Vec<Concept>>
where
    I: Iterator<Item = rusqlite::Result<String>>,
{
    let mut out = Vec::new();
    for row in rows {
        out.push(decode_concept("concepts", &row?)?);
    }
    Ok(out)
}

/// Insert `c` and everything under it that earns a row, plus the participant
/// index for each. Callers must have already decided that `c` itself deserves
/// a row.
pub(crate) fn write_tree(tx: &Transaction<'_>, c: &Concept) -> Result<ContentId> {
    let id = insert_row(tx, c)?;
    insert_participants(tx, c)?;
    if let Concept::Compound { head, args } = c {
        for child in std::iter::once(head.as_ref()).chain(args.iter()) {
            if is_storable(child) {
                write_tree(tx, child)?;
            }
        }
    }
    Ok(id)
}

/// Same as [`write_tree`] but for the case where the caller is asserting
/// something about the concept, which materializes even a bare ground value.
pub(crate) fn force_tree(tx: &Transaction<'_>, c: &Concept) -> Result<ContentId> {
    if c.is_hole() {
        return Err(StoreError::HoleNotStorable);
    }
    write_tree(tx, c)
}

fn insert_row(tx: &Transaction<'_>, c: &Concept) -> Result<ContentId> {
    let row = ConceptRow::build(c)?;
    tx.execute(
        "INSERT INTO concepts
            (content_id, shape, symbol, ground_kind, ground_json,
             head_symbol, head_id, arity, encoded, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(content_id) DO NOTHING",
        params![
            &row.content_id.0[..],
            row.shape,
            row.symbol,
            row.ground_kind,
            row.ground_json,
            row.head_symbol,
            row.head_id,
            row.arity,
            row.encoded,
            ts_to_sql(Utc::now()),
        ],
    )?;
    Ok(row.content_id)
}

fn insert_participants(tx: &Transaction<'_>, c: &Concept) -> Result<()> {
    let entries = participants(c);
    if entries.is_empty() {
        return Ok(());
    }
    let id = c.content_id();
    let mut stmt = tx.prepare(
        "INSERT INTO participants (content_id, participant_id, position, depth)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(content_id, participant_id, position)
         DO UPDATE SET depth = MIN(depth, excluded.depth)",
    )?;
    for entry in entries {
        stmt.execute(params![
            &id.0[..],
            &entry.id.0[..],
            entry.position,
            entry.depth
        ])?;
    }
    Ok(())
}
