//! Schema definition and migration.
//!
//! The database carries its own version in `meta.schema_version`. Opening runs
//! every migration newer than that version, in order, inside one transaction
//! each. A database from a newer build is refused rather than read on a hope:
//! silently misreading columns is worse than not opening.

use rusqlite::{Connection, OptionalExtension};

use crate::error::{Result, StoreError};

/// Schema version this build writes and understands.
pub const SCHEMA_VERSION: u32 = 2;

pub(crate) const SCHEMA_VERSION_KEY: &str = "schema_version";

/// One forward step. `version` is the version the database is at *after* the
/// step runs, so migration `n` is what takes a database from `n - 1` to `n`.
struct Migration {
    version: u32,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: V1,
    },
    Migration {
        version: 2,
        sql: V2,
    },
];

/// Episodes: the record of what actually happened, turn by turn.
///
/// Stored as JSON rather than shredded into columns because an episode is read
/// whole, by the doctor and by credit assignment, and its shape will keep
/// changing as the cognitive loop grows. Indexing what is queried (session,
/// time) gets the useful part without freezing the rest.
const V2: &str = r#"
CREATE TABLE episodes (
    id         INTEGER PRIMARY KEY,
    at         INTEGER NOT NULL,
    session    TEXT NOT NULL,
    json       TEXT NOT NULL
);
CREATE INDEX episodes_session ON episodes(session);
CREATE INDEX episodes_at ON episodes(at);
"#;

const V1: &str = r#"
CREATE TABLE concepts (
    content_id  BLOB PRIMARY KEY,
    shape       TEXT NOT NULL,
    symbol      INTEGER,
    ground_kind TEXT,
    ground_json TEXT,
    head_symbol INTEGER,
    head_id     BLOB,
    arity       INTEGER NOT NULL,
    encoded     TEXT NOT NULL,
    created_at  INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX concepts_head_symbol ON concepts(head_symbol);
CREATE INDEX concepts_symbol ON concepts(symbol);
CREATE INDEX concepts_shape ON concepts(shape);

CREATE TABLE participants (
    content_id     BLOB NOT NULL,
    participant_id BLOB NOT NULL,
    position       INTEGER NOT NULL,
    depth          INTEGER NOT NULL,
    PRIMARY KEY (content_id, participant_id, position)
) WITHOUT ROWID;

CREATE INDEX participants_participant ON participants(participant_id);

CREATE TABLE assertions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    content_id     BLOB NOT NULL,
    asserted_at    INTEGER NOT NULL,
    invalidated_at INTEGER,
    valid_from     INTEGER,
    valid_to       INTEGER,
    provenance     TEXT NOT NULL,
    episode        INTEGER,
    confidence     REAL
);

CREATE INDEX assertions_content ON assertions(content_id);
CREATE INDEX assertions_invalidated ON assertions(invalidated_at);

CREATE TABLE concept_meta (
    content_id    BLOB PRIMARY KEY,
    surface_forms TEXT NOT NULL,
    activation    TEXT NOT NULL,
    provenance    TEXT NOT NULL,
    tier          TEXT NOT NULL,
    note          TEXT
) WITHOUT ROWID;

CREATE TABLE surface_forms (
    form_lower TEXT NOT NULL,
    content_id BLOB NOT NULL,
    position   INTEGER NOT NULL,
    PRIMARY KEY (form_lower, content_id)
) WITHOUT ROWID;

CREATE INDEX surface_forms_content ON surface_forms(content_id);

CREATE TABLE realizations (
    name       TEXT PRIMARY KEY,
    target_id  BLOB NOT NULL,
    kind       TEXT NOT NULL,
    spec       TEXT NOT NULL,
    effect     TEXT NOT NULL,
    activation TEXT NOT NULL,
    provenance TEXT NOT NULL,
    tier       TEXT NOT NULL
) WITHOUT ROWID;

CREATE INDEX realizations_target ON realizations(target_id);
CREATE INDEX realizations_kind ON realizations(kind);

CREATE TABLE symbols (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;
"#;

/// Connection-level settings applied on every open.
///
/// WAL keeps readers from blocking on a writer, which matters as soon as the
/// inspector reads the same brain the evaluator is writing. `foreign_keys` is
/// on for future constraints; `synchronous=NORMAL` is the standard WAL pairing
/// that trades a crash-window of the last commit for an order of magnitude of
/// write throughput.
pub(crate) fn apply_pragmas(conn: &Connection) -> Result<()> {
    // journal_mode returns a row, so it goes through execute_batch rather than
    // pragma_update, which rejects statements that produce results.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;",
    )?;
    Ok(())
}

/// Bring the database up to [`SCHEMA_VERSION`], creating it if it is empty.
/// Tables only the v1 system ever created.
///
/// v2 is a different system with an incompatible schema, not a later version of
/// v1, so there is no migration between them. Opening a v1 brain and creating
/// v2 tables alongside would leave a file that neither system can read, which
/// is a worse outcome than refusing.
const V1_ERA_TABLES: &[&str] = &["actions", "fact_args", "pairs", "stances"];

fn looks_like_v1_brain(conn: &Connection) -> Result<bool> {
    for table in V1_ERA_TABLES {
        let found: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .optional()?;
        if found.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    ensure_meta_table(conn)?;
    let current = read_version(conn)?;
    if current == 0 && looks_like_v1_brain(conn)? {
        return Err(StoreError::corrupt(
            "meta",
            "this file is a v1 Spoon brain, whose schema v2 cannot read. \
             v2 keeps its own file so the v1 brain stays intact; point --db \
             somewhere else, or use the default ~/.spoon/spoon-v2.db",
        ));
    }
    if current > SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    for migration in MIGRATIONS.iter().filter(|m| m.version > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(migration.sql)?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![SCHEMA_VERSION_KEY, migration.version.to_string()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

/// The `meta` table has to exist before the first migration can record that it
/// ran, so it is created outside the migration list. It is also created *by*
/// migration 1 for a fresh database, hence `IF NOT EXISTS` in both places.
fn ensure_meta_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
         ) WITHOUT ROWID;",
    )?;
    Ok(())
}

fn read_version(conn: &Connection) -> Result<u32> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [SCHEMA_VERSION_KEY],
            |row| row.get(0),
        )
        .optional()?;
    match raw {
        None => Ok(0),
        Some(text) => text
            .parse::<u32>()
            .map_err(|_| StoreError::corrupt("meta", format!("schema_version is {text:?}"))),
    }
}
