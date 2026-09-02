//! Read-only paged listings for the inspector's `/debug/*` endpoints.
//! `q` is a case-insensitive substring filter (NULL means no filter); rows
//! come newest first.

use std::collections::HashMap;

use rusqlite::params;

use super::{row_to_episode, row_to_fact, row_to_pair, row_to_stance, Stance, Store};
use crate::types::{Episode, Fact, Pair};

/// `%q%` for LIKE, or NULL (which the listing queries read as "no filter").
fn like_pattern(q: Option<&str>) -> Option<String> {
    q.map(str::trim).filter(|s| !s.is_empty()).map(|s| format!("%{s}%"))
}

impl Store {
    pub fn list_facts(&self, q: Option<&str>, limit: usize, offset: usize) -> anyhow::Result<Vec<Fact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, pred, args_json, truth, modal, asserted_at, invalidated_at, source, episode_id
             FROM facts
             WHERE ?1 IS NULL OR pred LIKE ?1 OR args_json LIKE ?1 OR source LIKE ?1
             ORDER BY id DESC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![like_pattern(q), limit as i64, offset as i64], row_to_fact)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn list_pairs(&self, q: Option<&str>, limit: usize, offset: usize) -> anyhow::Result<Vec<Pair>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, utterance, sce, source, at, credit FROM pairs
             WHERE ?1 IS NULL OR utterance LIKE ?1 OR sce LIKE ?1 OR source LIKE ?1
             ORDER BY at DESC, id DESC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![like_pattern(q), limit as i64, offset as i64], row_to_pair)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn list_stances(&self, q: Option<&str>, limit: usize, offset: usize) -> anyhow::Result<Vec<Stance>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, topic, stance, reasons_json, confidence, source, at FROM stances
             WHERE ?1 IS NULL OR topic LIKE ?1 OR stance LIKE ?1 OR reasons_json LIKE ?1 OR source LIKE ?1
             ORDER BY at DESC, id DESC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![like_pattern(q), limit as i64, offset as i64], row_to_stance)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The episode `json` column holds the user text, sce and reply, so one
    /// LIKE over it covers all three.
    pub fn list_episodes(&self, q: Option<&str>, limit: usize, offset: usize) -> anyhow::Result<Vec<Episode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, at, user_text, sce, json, credit, keywords FROM episodes
             WHERE ?1 IS NULL OR session_id LIKE ?1 OR json LIKE ?1
             ORDER BY id DESC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![like_pattern(q), limit as i64, offset as i64], row_to_episode)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn list_kv(&self, q: Option<&str>, limit: usize, offset: usize) -> anyhow::Result<Vec<(String, serde_json::Value)>> {
        let mut stmt = self.conn.prepare(
            "SELECT key, json FROM kv
             WHERE ?1 IS NULL OR key LIKE ?1 OR json LIKE ?1
             ORDER BY key LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![like_pattern(q), limit as i64, offset as i64], |r| {
            let key: String = r.get(0)?;
            let json: String = r.get(1)?;
            Ok((key, json))
        })?;
        rows.map(|r| -> anyhow::Result<(String, serde_json::Value)> {
            let (key, json) = r?;
            Ok((key, serde_json::from_str(&json)?))
        })
        .collect()
    }

    /// `id -> updated_at` for every persisted action. Kernel actions live only
    /// in the CAN, so they are absent here.
    pub fn action_updated_at(&self) -> anyhow::Result<HashMap<String, i64>> {
        self.updated_at_map("SELECT id, updated_at FROM actions")
    }

    pub fn concept_updated_at(&self) -> anyhow::Result<HashMap<String, i64>> {
        self.updated_at_map("SELECT id, updated_at FROM concepts")
    }

    fn updated_at_map(&self, sql: &str) -> anyhow::Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
