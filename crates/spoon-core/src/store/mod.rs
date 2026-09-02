//! SQLite persistence. Synchronous rusqlite; the Brain wraps this in a Mutex.

mod inspect;
mod schema;

use std::path::Path;

use rusqlite::{Row, params};

use crate::can::Can;
use crate::types::*;

pub struct Store {
    conn: rusqlite::Connection,
    has_fts5: bool,
}

// rusqlite::Connection is Send, so Store is Send.
unsafe impl Send for Store {}

/// Git-friendly, shareable snapshot of learned knowledge (not episodes).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct Seed {
    pub version: u32,
    pub name: String,
    pub created_at: i64,
    pub concepts: Vec<Concept>,
    pub actions: Vec<Action>,
    pub pairs: Vec<Pair>,
    pub facts: Vec<Fact>,
    pub stances: Vec<Stance>,
    /// Arbitrary key/value config and lexicon extras (slang tables etc).
    #[serde(default)]
    pub kv: Vec<(String, serde_json::Value)>,
}

/// An opinion Spoon holds. Learned in pretrain or from conversation; revisable.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stance {
    pub id: i64,
    pub topic: String,
    pub stance: String,
    pub reasons: Vec<String>,
    pub confidence: f32,
    pub source: String,
    pub at: i64,
}

pub struct EpisodeQuery<'a> {
    pub session_id: Option<&'a str>,
    pub keywords: &'a [String],
    pub since_ms: Option<i64>,
    pub limit: usize,
}

// ---- helpers ----------------------------------------------------------------

fn canonical_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn row_to_fact(row: &Row<'_>) -> rusqlite::Result<Fact> {
    let args_json: String = row.get(2)?;
    let args: Vec<Value> = serde_json::from_str(&args_json).unwrap_or_default();
    Ok(Fact {
        id: row.get(0)?,
        pred: ActionId(row.get::<_, String>(1)?),
        args,
        truth: row.get::<_, i64>(3)? != 0,
        modal: row.get(4)?,
        asserted_at: row.get(5)?,
        invalidated_at: row.get(6)?,
        source: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
        episode_id: row.get(8)?,
    })
}

fn row_to_episode(row: &Row<'_>) -> rusqlite::Result<Episode> {
    let id: i64 = row.get(0)?;
    let json: String = row.get(5)?;
    let credit: i64 = row.get(6)?;
    let mut ep: Episode = serde_json::from_str(&json)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    ep.id = id;
    ep.credit = credit as i8;
    Ok(ep)
}

fn row_to_stance(row: &Row<'_>) -> rusqlite::Result<Stance> {
    let reasons_json: String = row.get(3)?;
    let reasons: Vec<String> = serde_json::from_str(&reasons_json).unwrap_or_default();
    Ok(Stance {
        id: row.get(0)?,
        topic: row.get(1)?,
        stance: row.get(2)?,
        reasons,
        confidence: row.get::<_, f64>(4)? as f32,
        source: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
        at: row.get(6)?,
    })
}

fn row_to_pair(row: &Row<'_>) -> rusqlite::Result<Pair> {
    Ok(Pair {
        id: row.get(0)?,
        utterance: row.get(1)?,
        sce: row.get(2)?,
        source: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        at: row.get(4)?,
        credit: row.get::<_, i64>(5)? as i8,
    })
}

/// Sanitize keywords for FTS5 MATCH: keep alphanumeric/whitespace, strip quotes.
fn fts_match_expr(keywords: &[String]) -> Option<String> {
    let terms: Vec<String> = keywords
        .iter()
        .map(|kw| {
            kw.chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace())
                .collect::<String>()
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

fn setup_conn(conn: &rusqlite::Connection) -> anyhow::Result<bool> {
    schema::apply(conn)
}

// ---- Store ------------------------------------------------------------------

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Store> {
        let conn = rusqlite::Connection::open(path)?;
        let has_fts5 = setup_conn(&conn)?;
        Ok(Store { conn, has_fts5 })
    }

    pub fn open_memory() -> anyhow::Result<Store> {
        let conn = rusqlite::Connection::open_in_memory()?;
        let has_fts5 = setup_conn(&conn)?;
        Ok(Store { conn, has_fts5 })
    }

    // ---- CAN ----------------------------------------------------------------

    pub fn load_can(&self) -> anyhow::Result<Can> {
        let mut can = Can::new();
        let mut stmt = self.conn.prepare("SELECT json FROM concepts")?;
        let concepts: Vec<Concept> = stmt
            .query_map([], |r| {
                let j: String = r.get(0)?;
                serde_json::from_str(&j)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?
            .collect::<Result<_, _>>()?;
        for c in concepts {
            can.add_concept(c);
        }

        let mut stmt = self.conn.prepare("SELECT json FROM actions")?;
        let actions: Vec<Action> = stmt
            .query_map([], |r| {
                let j: String = r.get(0)?;
                serde_json::from_str(&j)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?
            .collect::<Result<_, _>>()?;
        for a in actions {
            can.add_action(a);
        }
        Ok(can)
    }

    pub fn save_concept(&self, c: &Concept) -> anyhow::Result<()> {
        let json = serde_json::to_string(c)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO concepts (id, json, tier, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![c.id.0, json, format!("{:?}", c.tier), now_ms()],
        )?;
        Ok(())
    }

    pub fn save_action(&self, a: &Action) -> anyhow::Result<()> {
        let json = serde_json::to_string(a)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO actions (id, json, tier, effect, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![a.id.0, json, format!("{:?}", a.tier), format!("{:?}", a.effect), now_ms()],
        )?;
        Ok(())
    }

    pub fn delete_action(&self, id: &ActionId) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM actions WHERE id = ?1", params![id.0])?;
        Ok(())
    }

    // ---- facts --------------------------------------------------------------

    pub fn insert_fact(&self, f: &Fact) -> anyhow::Result<i64> {
        let args_json = serde_json::to_string(&f.args)?;
        self.conn.execute(
            "INSERT INTO facts (pred, args_json, truth, modal, asserted_at, invalidated_at, source, episode_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                f.pred.0,
                args_json,
                f.truth as i64,
                f.modal,
                f.asserted_at,
                f.invalidated_at,
                f.source,
                f.episode_id,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        for (pos, val) in f.args.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO fact_args (fact_id, pos, value_text) VALUES (?1, ?2, ?3)",
                params![id, pos as i64, canonical_json(val)],
            )?;
        }
        Ok(id)
    }

    pub fn query_facts(&self, pred: &ActionId, pattern: &[Option<Value>]) -> anyhow::Result<Vec<Fact>> {
        let mut sql = String::from(
            "SELECT id, pred, args_json, truth, modal, asserted_at, invalidated_at, source, episode_id
             FROM facts WHERE pred = ?1 AND invalidated_at IS NULL",
        );
        // params[0] = pred (?1). Each pattern slot adds two params: pos (?n) and value (?n+1).
        let mut flat: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(pred.0.clone())];

        for (i, slot) in pattern.iter().enumerate() {
            if let Some(v) = slot {
                let pos_idx = flat.len() + 1;
                let val_idx = flat.len() + 2;
                sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM fact_args WHERE fact_id = facts.id AND pos = ?{pos_idx} AND value_text = ?{val_idx})"
                ));
                flat.push(Box::new(i as i64));
                flat.push(Box::new(canonical_json(v)));
            }
        }
        sql.push_str(" ORDER BY asserted_at DESC");

        let refs: Vec<&dyn rusqlite::types::ToSql> = flat.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), row_to_fact)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn facts_about(&self, value: &Value) -> anyhow::Result<Vec<Fact>> {
        let val_json = canonical_json(value);
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.pred, f.args_json, f.truth, f.modal,
                    f.asserted_at, f.invalidated_at, f.source, f.episode_id
             FROM facts f
             JOIN fact_args fa ON fa.fact_id = f.id
             WHERE fa.value_text = ?1 AND f.invalidated_at IS NULL
             GROUP BY f.id
             ORDER BY f.asserted_at DESC",
        )?;
        let rows = stmt.query_map(params![val_json], row_to_fact)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn invalidate_fact(&self, id: i64, at: i64) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE facts SET invalidated_at = ?1 WHERE id = ?2",
            params![at, id],
        )?;
        Ok(())
    }

    // ---- episodes -----------------------------------------------------------

    pub fn insert_episode(&self, e: &Episode) -> anyhow::Result<i64> {
        let json = serde_json::to_string(e)?;
        let keywords = e.keywords.join(" ");
        self.conn.execute(
            "INSERT INTO episodes (session_id, at, user_text, sce, json, credit, keywords)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![e.session_id, e.at, e.user_text, e.sce, json, e.credit as i64, keywords],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_episode_credit(&self, id: i64, credit: i8) -> anyhow::Result<()> {
        // Update indexed column. Also patch json so round-trips stay consistent.
        let json: String = self.conn.query_row(
            "SELECT json FROM episodes WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        let mut ep: Episode = serde_json::from_str(&json)?;
        ep.credit = credit;
        let new_json = serde_json::to_string(&ep)?;
        self.conn.execute(
            "UPDATE episodes SET credit = ?1, json = ?2 WHERE id = ?3",
            params![credit as i64, new_json, id],
        )?;
        Ok(())
    }

    pub fn episodes(&self, q: &EpisodeQuery<'_>) -> anyhow::Result<Vec<Episode>> {
        let limit = q.limit.max(1) as i64;

        // If we have keywords, try FTS5 first
        if let Some(fts_expr) = (!q.keywords.is_empty()).then(|| fts_match_expr(q.keywords)).flatten() {
            if self.has_fts5 {
                return self.episodes_fts(q, &fts_expr, limit);
            }
            return self.episodes_like(q, q.keywords, limit);
        }
        self.episodes_plain(q, limit)
    }

    fn episodes_fts(&self, q: &EpisodeQuery<'_>, fts_expr: &str, limit: i64) -> anyhow::Result<Vec<Episode>> {
        // Use the table name (not alias) for MATCH -- SQLite FTS5 requires it.
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(fts_expr.to_string())];
        let mut where_extras = String::new();

        if let Some(sid) = q.session_id {
            let idx = params.len() + 1;
            where_extras.push_str(&format!(" AND e.session_id = ?{idx}"));
            params.push(Box::new(sid.to_string()));
        }
        if let Some(since) = q.since_ms {
            let idx = params.len() + 1;
            where_extras.push_str(&format!(" AND e.at >= ?{idx}"));
            params.push(Box::new(since));
        }
        let limit_idx = params.len() + 1;
        params.push(Box::new(limit));

        let sql = format!(
            "SELECT e.id, e.session_id, e.at, e.user_text, e.sce, e.json, e.credit, e.keywords
             FROM episodes_fts
             JOIN episodes e ON e.id = episodes_fts.rowid
             WHERE episodes_fts MATCH ?1{where_extras}
             ORDER BY episodes_fts.rank, e.at DESC LIMIT ?{limit_idx}"
        );

        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), row_to_episode)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn episodes_like(&self, q: &EpisodeQuery<'_>, keywords: &[String], limit: i64) -> anyhow::Result<Vec<Episode>> {
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut like_parts: Vec<String> = Vec::new();

        for kw in keywords {
            let pat = format!("%{}%", kw.to_lowercase());
            let a = params.len() + 1;
            let b = params.len() + 2;
            let c = params.len() + 3;
            like_parts.push(format!(
                "(LOWER(user_text) LIKE ?{a} OR LOWER(sce) LIKE ?{b} OR LOWER(keywords) LIKE ?{c})"
            ));
            params.push(Box::new(pat.clone()));
            params.push(Box::new(pat.clone()));
            params.push(Box::new(pat));
        }

        let mut sql = format!(
            "SELECT id, session_id, at, user_text, sce, json, credit, keywords
             FROM episodes WHERE ({})",
            like_parts.join(" OR ")
        );

        if let Some(sid) = q.session_id {
            let idx = params.len() + 1;
            sql.push_str(&format!(" AND session_id = ?{idx}"));
            params.push(Box::new(sid.to_string()));
        }
        if let Some(since) = q.since_ms {
            let idx = params.len() + 1;
            sql.push_str(&format!(" AND at >= ?{idx}"));
            params.push(Box::new(since));
        }
        let limit_idx = params.len() + 1;
        sql.push_str(&format!(" ORDER BY at DESC LIMIT ?{limit_idx}"));
        params.push(Box::new(limit));

        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), row_to_episode)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn episodes_plain(&self, q: &EpisodeQuery<'_>, limit: i64) -> anyhow::Result<Vec<Episode>> {
        let mut sql = String::from(
            "SELECT id, session_id, at, user_text, sce, json, credit, keywords FROM episodes WHERE 1=1",
        );
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(sid) = q.session_id {
            let idx = params.len() + 1;
            sql.push_str(&format!(" AND session_id = ?{idx}"));
            params.push(Box::new(sid.to_string()));
        }
        if let Some(since) = q.since_ms {
            let idx = params.len() + 1;
            sql.push_str(&format!(" AND at >= ?{idx}"));
            params.push(Box::new(since));
        }
        let limit_idx = params.len() + 1;
        sql.push_str(&format!(" ORDER BY at DESC LIMIT ?{limit_idx}"));
        params.push(Box::new(limit));

        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), row_to_episode)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn last_episode(&self, session_id: &str) -> anyhow::Result<Option<Episode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, at, user_text, sce, json, credit, keywords
             FROM episodes WHERE session_id = ?1 ORDER BY at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![session_id], row_to_episode)?;
        Ok(rows.next().transpose()?)
    }

    // ---- pairs --------------------------------------------------------------

    pub fn insert_pair(&self, p: &Pair) -> anyhow::Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO pairs (utterance, sce, source, at, credit) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![p.utterance, p.sce, p.source, p.at, p.credit as i64],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM pairs WHERE utterance = ?1 AND sce = ?2",
            params![p.utterance, p.sce],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn pairs(&self, min_credit: i8) -> anyhow::Result<Vec<Pair>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, utterance, sce, source, at, credit FROM pairs WHERE credit >= ?1 ORDER BY at DESC",
        )?;
        let rows = stmt.query_map(params![min_credit as i64], row_to_pair)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn update_pair_credit(&self, id: i64, credit: i8) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE pairs SET credit = ?1 WHERE id = ?2",
            params![credit as i64, id],
        )?;
        Ok(())
    }

    // ---- stances ------------------------------------------------------------

    pub fn upsert_stance(&self, s: &Stance) -> anyhow::Result<i64> {
        let reasons_json = serde_json::to_string(&s.reasons)?;
        self.conn.execute(
            "INSERT INTO stances (topic, stance, reasons_json, confidence, source, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(topic, stance) DO UPDATE SET
                 reasons_json = excluded.reasons_json,
                 confidence   = excluded.confidence,
                 source       = excluded.source,
                 at           = excluded.at",
            params![s.topic, s.stance, reasons_json, s.confidence as f64, s.source, s.at],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM stances WHERE topic = ?1 AND stance = ?2",
            params![s.topic, s.stance],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn stances(&self, topic_keywords: &[String]) -> anyhow::Result<Vec<Stance>> {
        if topic_keywords.is_empty() {
            let mut stmt = self.conn.prepare(
                "SELECT id, topic, stance, reasons_json, confidence, source, at
                 FROM stances ORDER BY at DESC",
            )?;
            let rows = stmt.query_map([], row_to_stance)?;
            return Ok(rows.collect::<Result<_, _>>()?);
        }

        let like_clause = topic_keywords
            .iter()
            .enumerate()
            .map(|(i, _)| format!("LOWER(topic) LIKE ?{}", i + 1))
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT id, topic, stance, reasons_json, confidence, source, at
             FROM stances WHERE {like_clause} ORDER BY at DESC"
        );
        let like_params: Vec<String> =
            topic_keywords.iter().map(|kw| format!("%{}%", kw.to_lowercase())).collect();
        let refs: Vec<&dyn rusqlite::types::ToSql> = like_params
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), row_to_stance)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // ---- kv -----------------------------------------------------------------

    pub fn kv_get(&self, key: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let mut stmt = self.conn.prepare("SELECT json FROM kv WHERE key = ?1")?;
        let mut rows = stmt.query_map(params![key], |r| r.get::<_, String>(0))?;
        match rows.next() {
            None => Ok(None),
            Some(s) => {
                let s = s?;
                Ok(Some(serde_json::from_str(&s)?))
            }
        }
    }

    pub fn kv_set(&self, key: &str, value: &serde_json::Value) -> anyhow::Result<()> {
        let json = serde_json::to_string(value)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO kv (key, json) VALUES (?1, ?2)",
            params![key, json],
        )?;
        Ok(())
    }

    // ---- seed ---------------------------------------------------------------

    pub fn export_seed(&self, name: &str) -> anyhow::Result<Seed> {
        // Concepts: exclude Kernel tier
        let concepts: Vec<Concept> = {
            let mut stmt = self.conn.prepare("SELECT json FROM concepts WHERE tier != 'Kernel'")?;
            stmt.query_map([], |r| {
                let j: String = r.get(0)?;
                serde_json::from_str(&j)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?
            .collect::<Result<_, _>>()?
        };

        // Actions: exclude Kernel tier
        let actions: Vec<Action> = {
            let mut stmt = self.conn.prepare("SELECT json FROM actions WHERE tier != 'Kernel'")?;
            stmt.query_map([], |r| {
                let j: String = r.get(0)?;
                serde_json::from_str(&j)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?
            .collect::<Result<_, _>>()?
        };

        // Pairs: credit >= 0
        let pairs: Vec<Pair> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, utterance, sce, source, at, credit FROM pairs WHERE credit >= 0 ORDER BY at DESC",
            )?;
            stmt.query_map([], row_to_pair)?.collect::<Result<_, _>>()?
        };

        // Valid facts from teacher or seed only (user facts are private)
        let facts: Vec<Fact> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, pred, args_json, truth, modal, asserted_at, invalidated_at, source, episode_id
                 FROM facts
                 WHERE invalidated_at IS NULL AND source IN ('teacher', 'seed')
                 ORDER BY asserted_at DESC",
            )?;
            stmt.query_map([], row_to_fact)?.collect::<Result<_, _>>()?
        };

        // All stances
        let stances: Vec<Stance> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, topic, stance, reasons_json, confidence, source, at FROM stances ORDER BY at DESC",
            )?;
            stmt.query_map([], row_to_stance)?.collect::<Result<_, _>>()?
        };

        // kv where key starts with "seed."
        let kv: Vec<(String, serde_json::Value)> = {
            let mut stmt = self.conn.prepare("SELECT key, json FROM kv WHERE key LIKE 'seed.%'")?;
            stmt.query_map([], |r| {
                let key: String = r.get(0)?;
                let json: String = r.get(1)?;
                Ok((key, json))
            })?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|(k, j)| serde_json::from_str(&j).ok().map(|v| (k, v)))
            .collect()
        };

        Ok(Seed {
            version: 1,
            name: name.to_string(),
            created_at: now_ms(),
            concepts,
            actions,
            pairs,
            facts,
            stances,
            kv,
        })
    }

    /// Merge a seed in. Existing kernel-tier actions are not overwritten.
    pub fn import_seed(&self, seed: &Seed) -> anyhow::Result<usize> {
        let mut written = 0usize;

        for c in &seed.concepts {
            self.save_concept(c)?;
            written += 1;
        }

        for a in &seed.actions {
            // Do not overwrite existing Kernel-tier actions
            let existing_tier: Option<String> = self
                .conn
                .query_row("SELECT tier FROM actions WHERE id = ?1", params![a.id.0], |r| r.get(0))
                .ok();
            if existing_tier.as_deref() == Some("Kernel") {
                continue;
            }
            self.save_action(a)?;
            written += 1;
        }

        for p in &seed.pairs {
            self.conn.execute(
                "INSERT OR IGNORE INTO pairs (utterance, sce, source, at, credit) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![p.utterance, p.sce, p.source, p.at, p.credit as i64],
            )?;
            written += self.conn.changes() as usize;
        }

        for f in &seed.facts {
            let args_json = serde_json::to_string(&f.args)?;
            self.conn.execute(
                "INSERT INTO facts (pred, args_json, truth, modal, asserted_at, invalidated_at, source, episode_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    f.pred.0, args_json, f.truth as i64, f.modal,
                    f.asserted_at, f.invalidated_at, f.source, f.episode_id,
                ],
            )?;
            let fid = self.conn.last_insert_rowid();
            for (pos, val) in f.args.iter().enumerate() {
                self.conn.execute(
                    "INSERT INTO fact_args (fact_id, pos, value_text) VALUES (?1, ?2, ?3)",
                    params![fid, pos as i64, canonical_json(val)],
                )?;
            }
            written += 1;
        }

        for s in &seed.stances {
            self.upsert_stance(s)?;
            written += 1;
        }

        for (key, val) in &seed.kv {
            self.kv_set(key, val)?;
            written += 1;
        }

        Ok(written)
    }

    pub fn counts(&self) -> anyhow::Result<StoreCounts> {
        let count = |sql: &str| -> anyhow::Result<usize> {
            Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as usize)
        };
        Ok(StoreCounts {
            concepts: count("SELECT COUNT(*) FROM concepts")?,
            actions: count("SELECT COUNT(*) FROM actions")?,
            facts: count("SELECT COUNT(*) FROM facts")?,
            fact_args: count("SELECT COUNT(*) FROM fact_args")?,
            episodes: count("SELECT COUNT(*) FROM episodes")?,
            pairs: count("SELECT COUNT(*) FROM pairs")?,
            stances: count("SELECT COUNT(*) FROM stances")?,
            kv: count("SELECT COUNT(*) FROM kv")?,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct StoreCounts {
    pub concepts: usize,
    pub actions: usize,
    pub facts: usize,
    #[serde(default)]
    pub fact_args: usize,
    pub episodes: usize,
    pub pairs: usize,
    pub stances: usize,
    #[serde(default)]
    pub kv: usize,
}
