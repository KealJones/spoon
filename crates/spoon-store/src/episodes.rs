//! Episode storage and the vocabulary ranking the ears run on.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use spoon_concept::Activation;

use crate::Store;
use crate::error::Result;

impl Store {
    /// The id the next episode should take.
    pub fn next_episode_id(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let highest: Option<i64> = conn
            .query_row("SELECT MAX(id) FROM episodes", [], |row| row.get(0))
            .optional()?
            .flatten();
        Ok(highest.unwrap_or(0) as u64 + 1)
    }

    /// Record one turn. Episodes are never rewritten, only appended: the whole
    /// point is to still have the path that produced a wrong answer.
    pub fn put_episode(&self, json: &str, id: u64) -> Result<()> {
        let session = serde_json::from_str::<serde_json::Value>(json)
            .ok()
            .and_then(|v| v["session"].as_str().map(str::to_string))
            .unwrap_or_default();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO episodes (id, at, session, json) VALUES (?1, ?2, ?3, ?4)",
            params![id as i64, Utc::now().timestamp_millis(), session, json],
        )?;
        Ok(())
    }

    /// Episodes newest first, optionally for one session.
    pub fn recent_episodes(&self, limit: usize, session: Option<&str>) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut out = Vec::new();
        match session {
            Some(session) => {
                let mut stmt = conn.prepare(
                    "SELECT json FROM episodes WHERE session = ?1 ORDER BY id DESC LIMIT ?2",
                )?;
                let rows =
                    stmt.query_map(params![session, limit as i64], |r| r.get::<_, String>(0))?;
                for row in rows {
                    out.push(row?);
                }
            }
            None => {
                let mut stmt =
                    conn.prepare("SELECT json FROM episodes WHERE 1=1 ORDER BY id DESC LIMIT ?1")?;
                let rows = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0))?;
                for row in rows {
                    out.push(row?);
                }
            }
        }
        Ok(out)
    }

    pub fn count_episodes(&self) -> Result<usize> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM episodes", [], |row| row.get(0))?;
        Ok(n as usize)
    }

    /// Surface forms worth showing the ears, most useful first.
    ///
    /// Ranked by ACT-R activation, so a brain used for one domain surfaces that
    /// domain's words. The prompt is working memory rather than a dictionary:
    /// listing everything Spoon knows would bury the handful of concepts this
    /// user actually reaches for, and cost more to do it.
    pub fn ranked_surface_forms(&self, limit: usize, now: DateTime<Utc>) -> Result<Vec<Arc<str>>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT surface_forms, activation FROM concept_meta WHERE surface_forms != '[]'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut scored: Vec<(f64, Arc<str>)> = Vec::new();
        for row in rows {
            let (forms_json, activation_json) = row?;
            let forms: Vec<String> = serde_json::from_str(&forms_json).unwrap_or_default();
            let Some(first) = forms.into_iter().next() else {
                continue;
            };
            let activation: Activation = match serde_json::from_str(&activation_json) {
                Ok(a) => a,
                Err(_) => continue,
            };
            // No history means unknown rather than bad, so an unused concept
            // sits below proven ones without being unreachable.
            let score = activation.base_level(now).unwrap_or(-8.0) * activation.success_rate();
            scored.push((score, Arc::from(first.as_str())));
        }
        // Ties break on the word so the prompt is byte-identical across runs,
        // which is what makes a failed turn reproducible.
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });
        Ok(scored.into_iter().take(limit).map(|(_, w)| w).collect())
    }
}

impl Store {
    /// Record that a later turn said this one was wrong.
    ///
    /// The episode is amended rather than replaced: the original reading, the
    /// realization chosen, and the answer given all stay exactly as they were,
    /// because the point of keeping them is to be able to ask what went wrong.
    pub fn mark_episode_corrected(&self, id: u64, correction: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let existing: Option<String> = conn
            .query_row(
                "SELECT json FROM episodes WHERE id = ?1",
                params![id as i64],
                |r| r.get(0),
            )
            .optional()?;
        let Some(existing) = existing else {
            return Ok(false);
        };
        let mut value: serde_json::Value = serde_json::from_str(&existing)?;
        value["correction"] = serde_json::Value::String(correction.to_string());
        conn.execute(
            "UPDATE episodes SET json = ?1 WHERE id = ?2",
            params![serde_json::to_string(&value)?, id as i64],
        )?;
        Ok(true)
    }

    /// Turns a later message said were wrong. The doctor reads these first.
    pub fn corrected_episodes(&self, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT json FROM episodes WHERE json_extract(json, '$.correction') IS NOT NULL \
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}
