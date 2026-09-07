//! What the store knows about a concept beyond its structure.
//!
//! Surface forms are duplicated into a small index table. The JSON array on
//! `concept_meta` stays the source of truth (it keeps preference order and is
//! what export writes); `surface_forms` is derived from it inside the same
//! transaction. Without that index every vocabulary lookup by the ears would
//! be a full scan of every described concept, and the ears do one per token.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction, params};
use spoon_concept::{Activation, Concept, ConceptMeta, Provenance, Tier};

use crate::Store;
use crate::concepts::force_tree;
use crate::encode::decode_concept;
use crate::error::Result;

/// Column list shared by every read of `concept_meta`.
const META_COLUMNS: &str = "surface_forms, activation, provenance, tier, note";

type MetaTuple = (String, String, String, String, Option<String>);

fn read_meta_tuple(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<MetaTuple> {
    Ok((
        row.get(offset)?,
        row.get(offset + 1)?,
        row.get(offset + 2)?,
        row.get(offset + 3)?,
        row.get(offset + 4)?,
    ))
}

fn build_meta(tuple: MetaTuple, concept: Concept) -> Result<ConceptMeta> {
    let (surface_forms, activation, provenance, tier, note) = tuple;
    Ok(ConceptMeta {
        concept,
        surface_forms: serde_json::from_str(&surface_forms)?,
        activation: serde_json::from_str(&activation)?,
        provenance: serde_json::from_str(&provenance)?,
        tier: serde_json::from_str(&tier)?,
        note: note.map(|n| n.into()),
    })
}

impl Store {
    /// Describe a concept. Overwrites any previous description of it.
    ///
    /// Writing metadata materializes the concept even when it is a bare ground
    /// value: giving `42` the surface form "forty-two" is precisely the kind of
    /// claim that earns it a row.
    pub fn put_meta(&self, meta: &ConceptMeta) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        write_meta(&tx, meta)?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_meta(&self, c: &Concept) -> Result<Option<ConceptMeta>> {
        let conn = self.conn.lock();
        let sql = format!("SELECT {META_COLUMNS} FROM concept_meta WHERE content_id = ?1");
        let tuple = conn
            .query_row(&sql, params![&c.content_id().0[..]], |row| {
                read_meta_tuple(row, 0)
            })
            .optional()?;
        match tuple {
            Some(tuple) => Ok(Some(build_meta(tuple, c.clone())?)),
            None => Ok(None),
        }
    }

    /// Record that this concept was used, and whether that went well.
    ///
    /// If the concept has never been described, a minimal description is
    /// created for it with `Provenance::Inferred` and `Tier::Provisional`. A
    /// concept the system actually reached for is worth tracking even when
    /// nobody wrote it down first, and refusing here would throw away evidence
    /// that cannot be recovered later.
    pub fn record_use(&self, c: &Concept, succeeded: bool, at: DateTime<Utc>) -> Result<()> {
        let mut meta = match self.get_meta(c)? {
            Some(meta) => meta,
            None => ConceptMeta::new(c.clone(), Provenance::Inferred, Tier::Provisional, at),
        };
        meta.activation.record(at, succeeded);
        self.put_meta(&meta)
    }

    /// Concepts that can be said this way. Case-insensitive, preferred forms
    /// first.
    ///
    /// Several concepts can share a surface form; that ambiguity is the
    /// caller's to resolve with context, not the store's to hide.
    pub fn surface_lookup(&self, form: &str) -> Result<Vec<Concept>> {
        let needle = form.trim().to_lowercase();
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT c.encoded FROM surface_forms s
             JOIN concepts c ON c.content_id = s.content_id
             WHERE s.form_lower = ?1
             ORDER BY s.position, c.created_at, c.content_id",
        )?;
        let rows = stmt.query_map(params![needle], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(decode_concept("concepts", &row?)?);
        }
        Ok(out)
    }

    /// Activation for a concept without loading the rest of its description.
    /// Selection asks this on the hot path.
    pub fn activation(&self, c: &Concept) -> Result<Option<Activation>> {
        let conn = self.conn.lock();
        let raw: Option<String> = conn
            .query_row(
                "SELECT activation FROM concept_meta WHERE content_id = ?1",
                params![&c.content_id().0[..]],
                |row| row.get(0),
            )
            .optional()?;
        match raw {
            Some(text) => Ok(Some(serde_json::from_str(&text)?)),
            None => Ok(None),
        }
    }
}

/// Every stored description, for seed export.
pub(crate) fn all_meta(store: &Store) -> Result<Vec<ConceptMeta>> {
    let conn = store.conn.lock();
    let sql = format!(
        "SELECT {}, c.encoded FROM concept_meta m
         JOIN concepts c ON c.content_id = m.content_id
         ORDER BY m.content_id",
        META_COLUMNS
            .split(", ")
            .map(|c| format!("m.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        let tuple = read_meta_tuple(row, 0)?;
        let encoded: String = row.get(5)?;
        Ok((tuple, encoded))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (tuple, encoded) = row?;
        let concept = decode_concept("concepts", &encoded)?;
        out.push(build_meta(tuple, concept)?);
    }
    Ok(out)
}

/// Write one description and rebuild its surface-form index rows. Shared by
/// [`Store::put_meta`] and seed import, which needs many of these inside one
/// transaction.
pub(crate) fn write_meta(tx: &Transaction<'_>, meta: &ConceptMeta) -> Result<()> {
    let surface_forms = serde_json::to_string(&meta.surface_forms)?;
    let activation = serde_json::to_string(&meta.activation)?;
    let provenance = serde_json::to_string(&meta.provenance)?;
    let tier = serde_json::to_string(&meta.tier)?;
    let id = meta.concept.content_id();

    force_tree(tx, &meta.concept)?;
    tx.execute(
        "INSERT INTO concept_meta
            (content_id, surface_forms, activation, provenance, tier, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(content_id) DO UPDATE SET
            surface_forms = excluded.surface_forms,
            activation    = excluded.activation,
            provenance    = excluded.provenance,
            tier          = excluded.tier,
            note          = excluded.note",
        params![
            &id.0[..],
            surface_forms,
            activation,
            provenance,
            tier,
            meta.note.as_ref().map(|n| n.to_string()),
        ],
    )?;
    tx.execute(
        "DELETE FROM surface_forms WHERE content_id = ?1",
        params![&id.0[..]],
    )?;
    let mut stmt = tx.prepare(
        "INSERT INTO surface_forms (form_lower, content_id, position)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(form_lower, content_id)
         DO UPDATE SET position = MIN(position, excluded.position)",
    )?;
    for (position, form) in meta.surface_forms.iter().enumerate() {
        stmt.execute(params![
            form.trim().to_lowercase(),
            &id.0[..],
            position as i64
        ])?;
    }
    Ok(())
}
