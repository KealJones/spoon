//! Stored ways to operationalize a concept.
//!
//! A concept may have several realizations at once and they compete, so these
//! are indexed by target as well as by name. The `kind` column is denormalized
//! out of the spec so a query can ask for "every rule" without parsing every
//! spec blob: rule application scans that set on each inference step.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction, params};
use spoon_concept::{Concept, Realization, RealizationKind};

use crate::Store;
use crate::concepts::force_tree;
use crate::encode::decode_concept;
use crate::error::{Result, StoreError};

const REALIZATION_COLUMNS: &str = "name, kind, spec, effect, activation, provenance, tier";

type RealizationTuple = (String, String, String, String, String, String, String);

fn read_realization_tuple(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<RealizationTuple> {
    Ok((
        row.get(offset)?,
        row.get(offset + 1)?,
        row.get(offset + 2)?,
        row.get(offset + 3)?,
        row.get(offset + 4)?,
        row.get(offset + 5)?,
        row.get(offset + 6)?,
    ))
}

fn build_realization(tuple: RealizationTuple, target: Concept) -> Result<Realization> {
    let (name, _kind, spec, effect, activation, provenance, tier) = tuple;
    Ok(Realization {
        target,
        name: name.into(),
        spec: serde_json::from_str(&spec)?,
        effect: serde_json::from_str(&effect)?,
        activation: serde_json::from_str(&activation)?,
        provenance: serde_json::from_str(&provenance)?,
        tier: serde_json::from_str(&tier)?,
    })
}

fn select(where_clause: &str) -> String {
    let columns = REALIZATION_COLUMNS
        .split(", ")
        .map(|c| format!("r.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT {columns}, c.encoded FROM realizations r
         JOIN concepts c ON c.content_id = r.target_id
         {where_clause}"
    )
}

impl Store {
    /// Store a realization, replacing any earlier one with the same name.
    ///
    /// The name is the identity: evidence accumulates against it, so
    /// overwriting is how a realization is revised in place rather than
    /// forked.
    /// Store a realization, replacing any earlier one with the same name.
    ///
    /// The name is the identity: evidence accumulates against it, so
    /// overwriting is how a realization is revised in place rather than
    /// forked.
    pub fn put_realization(&self, r: &Realization) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        write_realization(&tx, r)?;
        tx.commit()?;
        Ok(())
    }

    /// Every realization attached to this concept. These are the candidates
    /// selection chooses between.
    pub fn realizations_for(&self, target: &Concept) -> Result<Vec<Realization>> {
        let conn = self.conn.lock();
        let sql = select("WHERE r.target_id = ?1 ORDER BY r.name");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![&target.content_id().0[..]], |row| {
            let tuple = read_realization_tuple(row, 0)?;
            let encoded: String = row.get(7)?;
            Ok((tuple, encoded))
        })?;
        collect(rows)
    }

    pub fn realization_by_name(&self, name: &str) -> Result<Option<Realization>> {
        let conn = self.conn.lock();
        let sql = select("WHERE r.name = ?1");
        let row = conn
            .query_row(&sql, params![name], |row| {
                let tuple = read_realization_tuple(row, 0)?;
                let encoded: String = row.get(7)?;
                Ok((tuple, encoded))
            })
            .optional()?;
        match row {
            Some((tuple, encoded)) => {
                let target = decode_concept("concepts", &encoded)?;
                Ok(Some(build_realization(tuple, target)?))
            }
            None => Ok(None),
        }
    }

    /// Every realization of a given kind, for the engines that scan one kind:
    /// inference wants rules, the planner wants externals.
    pub fn realizations_by_kind(&self, kind: RealizationKind) -> Result<Vec<Realization>> {
        let conn = self.conn.lock();
        let sql = select("WHERE r.kind = ?1 ORDER BY r.name");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![kind.as_str()], |row| {
            let tuple = read_realization_tuple(row, 0)?;
            let encoded: String = row.get(7)?;
            Ok((tuple, encoded))
        })?;
        collect(rows)
    }

    /// Record an outcome against a realization.
    ///
    /// An unknown name is an error rather than a silent insert: evidence
    /// attributed to a realization nobody stored means the caller is holding a
    /// stale name, and inventing a row would hide that.
    pub fn record_realization_use(
        &self,
        name: &str,
        succeeded: bool,
        at: DateTime<Utc>,
    ) -> Result<()> {
        let conn = self.conn.lock();
        let raw: Option<String> = conn
            .query_row(
                "SELECT activation FROM realizations WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            return Err(StoreError::missing("realization", name));
        };
        let mut activation: spoon_concept::Activation = serde_json::from_str(&raw)?;
        activation.record(at, succeeded);
        conn.execute(
            "UPDATE realizations SET activation = ?1 WHERE name = ?2",
            params![serde_json::to_string(&activation)?, name],
        )?;
        Ok(())
    }

    /// Every realization in the brain, name-ordered. Used at load time to
    /// re-bind native keys and by seed export.
    pub fn all_realizations(&self) -> Result<Vec<Realization>> {
        let conn = self.conn.lock();
        let sql = select("ORDER BY r.name");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            let tuple = read_realization_tuple(row, 0)?;
            let encoded: String = row.get(7)?;
            Ok((tuple, encoded))
        })?;
        collect(rows)
    }

    /// Remove a realization outright.
    ///
    /// For realizations that should never have survived, as opposed to ones
    /// that lost: a bootstrap native whose Rust function was renamed away is
    /// not a weak candidate to be outcompeted, it is a promise the binary can
    /// no longer keep. Leaving it in place means every brain that ever ran an
    /// older build keeps offering a capability that cannot run.
    ///
    /// Losing realizations are deprecated instead, so their evidence survives.
    pub fn retire_realization(&self, name: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let n = conn.execute("DELETE FROM realizations WHERE name = ?1", [name])?;
        Ok(n > 0)
    }
}

fn collect<I>(rows: I) -> Result<Vec<Realization>>
where
    I: Iterator<Item = rusqlite::Result<(RealizationTuple, String)>>,
{
    let mut out = Vec::new();
    for row in rows {
        let (tuple, encoded) = row?;
        let target = decode_concept("concepts", &encoded)?;
        out.push(build_realization(tuple, target)?);
    }
    Ok(out)
}

/// Write one realization. Shared by [`Store::put_realization`] and seed
/// import, which needs a batch of them inside one transaction.
pub(crate) fn write_realization(tx: &Transaction<'_>, r: &Realization) -> Result<()> {
    let spec = serde_json::to_string(&r.spec)?;
    let effect = serde_json::to_string(&r.effect)?;
    let activation = serde_json::to_string(&r.activation)?;
    let provenance = serde_json::to_string(&r.provenance)?;
    let tier = serde_json::to_string(&r.tier)?;
    let target_id = r.target.content_id();

    force_tree(tx, &r.target)?;
    tx.execute(
        "INSERT INTO realizations
            (name, target_id, kind, spec, effect, activation, provenance, tier)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(name) DO UPDATE SET
            target_id  = excluded.target_id,
            kind       = excluded.kind,
            spec       = excluded.spec,
            effect     = excluded.effect,
            -- The name is the durable identity of a realization. Refreshing
            -- its implementation must not erase evidence collected against
            -- that identity.
            activation = realizations.activation,
            provenance = excluded.provenance,
            tier       = excluded.tier",
        params![
            r.name.as_ref(),
            &target_id.0[..],
            r.spec.kind().as_str(),
            spec,
            effect,
            activation,
            provenance,
            tier,
        ],
    )?;
    Ok(())
}
