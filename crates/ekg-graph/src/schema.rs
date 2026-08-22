use rusqlite::Connection;

use crate::error::Result;

/// Creates all tables and indexes used by the knowledge store, if they
/// don't already exist. Safe to call on every open.
pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS concepts (
            id              TEXT PRIMARY KEY,
            name            TEXT NOT NULL,
            description     TEXT,
            mutability      TEXT NOT NULL,
            confidence_json TEXT NOT NULL,
            lifecycle       TEXT NOT NULL,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS relationships (
            id             TEXT PRIMARY KEY,
            source         TEXT NOT NULL REFERENCES concepts(id),
            target         TEXT NOT NULL REFERENCES concepts(id),
            kind           TEXT NOT NULL,
            strength       REAL NOT NULL,
            scope_json     TEXT NOT NULL,
            evidence_json  TEXT NOT NULL,
            lifecycle      TEXT NOT NULL,
            created_at     INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS procedures (
            id              TEXT PRIMARY KEY,
            name            TEXT NOT NULL,
            params_json     TEXT NOT NULL,
            body_json       TEXT NOT NULL,
            contract_json   TEXT NOT NULL,
            test_cases_json TEXT NOT NULL,
            concept_id      TEXT REFERENCES concepts(id),
            version         INTEGER NOT NULL,
            lifecycle       TEXT NOT NULL,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_concepts_name ON concepts(name);
        CREATE INDEX IF NOT EXISTS idx_relationships_source ON relationships(source);
        CREATE INDEX IF NOT EXISTS idx_relationships_target ON relationships(target);
        CREATE INDEX IF NOT EXISTS idx_relationships_kind ON relationships(kind);
        CREATE INDEX IF NOT EXISTS idx_procedures_name ON procedures(name);
        CREATE INDEX IF NOT EXISTS idx_procedures_concept_id ON procedures(concept_id);
        "#,
    )?;
    Ok(())
}
