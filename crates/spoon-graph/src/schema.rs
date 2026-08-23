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

        CREATE TABLE IF NOT EXISTS procedure_versions (
            id              TEXT NOT NULL,
            name            TEXT NOT NULL,
            params_json     TEXT NOT NULL,
            body_json       TEXT NOT NULL,
            contract_json   TEXT NOT NULL,
            test_cases_json TEXT NOT NULL,
            concept_id      TEXT,
            version         INTEGER NOT NULL,
            lifecycle       TEXT NOT NULL,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL,
            PRIMARY KEY (id, version)
        );

        CREATE TABLE IF NOT EXISTS concept_versions (
            id              TEXT NOT NULL,
            version         INTEGER NOT NULL,
            name            TEXT NOT NULL,
            description     TEXT,
            mutability      TEXT NOT NULL,
            confidence_json TEXT NOT NULL,
            lifecycle       TEXT NOT NULL,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL,
            PRIMARY KEY (id, version)
        );

        CREATE TABLE IF NOT EXISTS relationship_versions (
            id             TEXT NOT NULL,
            version        INTEGER NOT NULL,
            source         TEXT NOT NULL,
            target         TEXT NOT NULL,
            kind           TEXT NOT NULL,
            strength       REAL NOT NULL,
            scope_json     TEXT NOT NULL,
            evidence_json  TEXT NOT NULL,
            lifecycle      TEXT NOT NULL,
            created_at     INTEGER NOT NULL,
            PRIMARY KEY (id, version)
        );

        CREATE TABLE IF NOT EXISTS graph_change_receipts (
            idempotency_key TEXT PRIMARY KEY,
            request_json    TEXT NOT NULL,
            receipt_json    TEXT NOT NULL,
            created_at      INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS knowledge_bundle_receipts (
            idempotency_key TEXT PRIMARY KEY,
            request_json    TEXT NOT NULL,
            created_at      INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_concepts_name ON concepts(name);
        CREATE INDEX IF NOT EXISTS idx_relationships_source ON relationships(source);
        CREATE INDEX IF NOT EXISTS idx_relationships_target ON relationships(target);
        CREATE INDEX IF NOT EXISTS idx_relationships_kind ON relationships(kind);
        CREATE INDEX IF NOT EXISTS idx_procedures_name ON procedures(name);
        CREATE INDEX IF NOT EXISTS idx_procedures_concept_id ON procedures(concept_id);
        CREATE INDEX IF NOT EXISTS idx_procedure_versions_id_version
            ON procedure_versions(id, version);
        CREATE INDEX IF NOT EXISTS idx_concept_versions_id_version
            ON concept_versions(id, version);
        CREATE INDEX IF NOT EXISTS idx_relationship_versions_id_version
            ON relationship_versions(id, version);

        INSERT OR IGNORE INTO procedure_versions
            (id, name, params_json, body_json, contract_json, test_cases_json,
             concept_id, version, lifecycle, created_at, updated_at)
        SELECT id, name, params_json, body_json, contract_json, test_cases_json,
               concept_id, version, lifecycle, created_at, updated_at
        FROM procedures;

        INSERT OR IGNORE INTO concept_versions
            (id, version, name, description, mutability, confidence_json,
             lifecycle, created_at, updated_at)
        SELECT id, 1, name, description, mutability, confidence_json,
               lifecycle, created_at, updated_at
        FROM concepts;

        INSERT OR IGNORE INTO relationship_versions
            (id, version, source, target, kind, strength, scope_json,
             evidence_json, lifecycle, created_at)
        SELECT id, 1, source, target, kind, strength, scope_json,
               evidence_json, lifecycle, created_at
        FROM relationships;
        "#,
    )?;
    Ok(())
}
