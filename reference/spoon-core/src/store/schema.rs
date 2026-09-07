//! SQLite schema creation and connection setup.

pub const SCHEMA_VERSION: i64 = 1;

/// Apply pragmas and create all tables idempotently.
/// Returns whether FTS5 is available.
pub fn apply(conn: &rusqlite::Connection) -> anyhow::Result<bool> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=5000;",
    )?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS kv (
            key  TEXT PRIMARY KEY,
            json TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS concepts (
            id         TEXT PRIMARY KEY,
            json       TEXT NOT NULL,
            tier       TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS actions (
            id         TEXT PRIMARY KEY,
            json       TEXT NOT NULL,
            tier       TEXT NOT NULL,
            effect     TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS facts (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            pred           TEXT    NOT NULL,
            args_json      TEXT    NOT NULL,
            truth          INTEGER NOT NULL,
            modal          TEXT,
            asserted_at    INTEGER NOT NULL,
            invalidated_at INTEGER,
            source         TEXT    NOT NULL DEFAULT '',
            episode_id     INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_facts_pred
            ON facts(pred, invalidated_at);
        CREATE INDEX IF NOT EXISTS idx_facts_episode
            ON facts(episode_id);

        CREATE TABLE IF NOT EXISTS fact_args (
            fact_id    INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
            pos        INTEGER NOT NULL,
            value_text TEXT    NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_fact_args_value
            ON fact_args(value_text);
        CREATE INDEX IF NOT EXISTS idx_fact_args_fact_pos
            ON fact_args(fact_id, pos);

        CREATE TABLE IF NOT EXISTS episodes (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT    NOT NULL,
            at         INTEGER NOT NULL,
            user_text  TEXT    NOT NULL,
            sce        TEXT    NOT NULL,
            json       TEXT    NOT NULL,
            credit     INTEGER NOT NULL DEFAULT 0,
            keywords   TEXT    NOT NULL DEFAULT ''
        );

        CREATE INDEX IF NOT EXISTS idx_episodes_session
            ON episodes(session_id, at);
        CREATE INDEX IF NOT EXISTS idx_episodes_at
            ON episodes(at);

        CREATE TABLE IF NOT EXISTS pairs (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            utterance TEXT    NOT NULL,
            sce       TEXT    NOT NULL,
            source    TEXT    NOT NULL DEFAULT '',
            at        INTEGER NOT NULL,
            credit    INTEGER NOT NULL DEFAULT 0,
            UNIQUE(utterance, sce)
        );

        CREATE INDEX IF NOT EXISTS idx_pairs_credit
            ON pairs(credit);

        CREATE TABLE IF NOT EXISTS stances (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            topic        TEXT    NOT NULL,
            stance       TEXT    NOT NULL,
            reasons_json TEXT    NOT NULL,
            confidence   REAL    NOT NULL,
            source       TEXT    NOT NULL DEFAULT '',
            at           INTEGER NOT NULL,
            UNIQUE(topic, stance)
        );",
    )?;

    // Mark schema version
    conn.execute(
        "INSERT OR IGNORE INTO kv (key, json) VALUES ('schema_version', ?1)",
        rusqlite::params![SCHEMA_VERSION.to_string()],
    )?;

    // Attempt FTS5 setup (bundled SQLite always has it, but guard gracefully)
    let has_fts5 = setup_fts5(conn);
    Ok(has_fts5)
}

fn setup_fts5(conn: &rusqlite::Connection) -> bool {
    let res = conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS episodes_fts USING fts5(
            user_text, sce, keywords,
            content='episodes',
            content_rowid='id'
        );

        CREATE TRIGGER IF NOT EXISTS episodes_ai
        AFTER INSERT ON episodes BEGIN
            INSERT INTO episodes_fts(rowid, user_text, sce, keywords)
            VALUES (new.id, new.user_text, new.sce, new.keywords);
        END;

        CREATE TRIGGER IF NOT EXISTS episodes_ad
        AFTER DELETE ON episodes BEGIN
            INSERT INTO episodes_fts(episodes_fts, rowid, user_text, sce, keywords)
            VALUES ('delete', old.id, old.user_text, old.sce, old.keywords);
        END;

        CREATE TRIGGER IF NOT EXISTS episodes_au
        AFTER UPDATE ON episodes BEGIN
            INSERT INTO episodes_fts(episodes_fts, rowid, user_text, sce, keywords)
            VALUES ('delete', old.id, old.user_text, old.sce, old.keywords);
            INSERT INTO episodes_fts(rowid, user_text, sce, keywords)
            VALUES (new.id, new.user_text, new.sce, new.keywords);
        END;",
    );
    match res {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("FTS5 unavailable, will use LIKE fallback: {e}");
            false
        }
    }
}
