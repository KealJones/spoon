//! Names for symbols.
//!
//! Identity does not depend on this table: `SymbolId::of` is a pure function
//! of the name, so nothing here is load-bearing for correctness. It exists so
//! that errors, the inspector, and the mouth can print `friend-with` instead
//! of a 64-bit integer.
//!
//! Because ids are computed rather than allocated, storing a concept teaches
//! the store nothing about names. Whoever knows the spelling (the reader, the
//! ears, a seed file) has to call [`Store::register_symbol`].

use rusqlite::{Connection, OptionalExtension, params};
use spoon_concept::{SymbolId, SymbolTable};

use crate::Store;
use crate::encode::{symbol_from_sql, symbol_to_sql};
use crate::error::{Result, StoreError};

impl Store {
    /// Record a name and return its id.
    ///
    /// The spelling is stored as written, matching `SymbolTable::intern`:
    /// identity already ignores casing, and `FriendWith` reads better in a log
    /// than `friendwith`. First registration wins, so a later spelling of the
    /// same symbol does not rewrite what everything else is already printing.
    pub fn register_symbol(&self, name: &str) -> Result<SymbolId> {
        let conn = self.conn.lock();
        write_symbol(&conn, name)
    }

    pub fn symbol_name(&self, id: SymbolId) -> Result<Option<String>> {
        let conn = self.conn.lock();
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM symbols WHERE id = ?1",
                params![symbol_to_sql(id)],
                |row| row.get(0),
            )
            .optional()?;
        Ok(name)
    }

    /// Rebuild an in-process symbol table from the database, so ids print as
    /// names again after a restart.
    pub fn load_symbol_table(&self) -> Result<SymbolTable> {
        let table = SymbolTable::new();
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name FROM symbols ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (raw, name) = row?;
            let stored = symbol_from_sql(raw);
            let derived = SymbolId::of(&name);
            if stored != derived {
                return Err(StoreError::corrupt(
                    "symbols",
                    format!("id {stored:?} does not match the id of {name:?}"),
                ));
            }
            table.intern(&name);
        }
        Ok(table)
    }

    /// Every (id, name) pair, id-ordered. Seed export uses this; so does the
    /// inspector's symbol listing.
    pub fn all_symbols(&self) -> Result<Vec<(SymbolId, String)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name FROM symbols ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (raw, name) = row?;
            out.push((symbol_from_sql(raw), name));
        }
        Ok(out)
    }
}

/// Insert one symbol name. Takes a plain connection so it works both on its
/// own and inside a seed-import transaction.
///
/// Existing rows are left alone rather than overwritten. `SymbolTable::intern`
/// is first-write-wins, and if the store were last-write-wins the same brain
/// would print a symbol differently before and after a reload.
pub(crate) fn write_symbol(conn: &Connection, name: &str) -> Result<SymbolId> {
    let id = SymbolId::of(name);
    conn.execute(
        "INSERT INTO symbols (id, name) VALUES (?1, ?2)
         ON CONFLICT(id) DO NOTHING",
        params![symbol_to_sql(id), name.trim()],
    )?;
    Ok(id)
}
