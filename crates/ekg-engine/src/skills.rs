//! Durable lifecycle records for discovered skills.
//!
//! Discovery is deliberately separate from activation.  A candidate can be
//! inspected, replayed, shadowed, promoted, and retired without overwriting
//! the episode evidence from which it was derived.

use ekg_adapt::{PromotionVerdict, RetirementRecord, SkillCandidate};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::EngineError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLifecycle {
    Candidate,
    Shadow,
    Promoted,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSkill {
    pub id: String,
    pub candidate: SkillCandidate,
    pub lifecycle: SkillLifecycle,
    /// The latest replay decision. A rejection is durable evidence too.
    pub promotion_verdict: Option<PromotionVerdict>,
    /// Successful, receipt-backed live shadow observations.
    pub shadow_live_wins: u32,
    pub experience_uses: u32,
    pub experience_successes: u32,
    pub experience_failures: u32,
    pub retirement: Option<RetirementRecord>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub(crate) struct SkillStore {
    conn: Connection,
}

impl SkillStore {
    pub(crate) fn open(path: &str) -> Result<Self, EngineError> {
        let store = Self {
            conn: Connection::open(path)?,
        };
        store.schema()?;
        Ok(store)
    }

    pub(crate) fn in_memory() -> Result<Self, EngineError> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.schema()?;
        Ok(store)
    }

    fn schema(&self) -> Result<(), EngineError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ekg_managed_skills (
                id TEXT PRIMARY KEY,
                candidate_json TEXT NOT NULL,
                lifecycle TEXT NOT NULL,
                promotion_verdict_json TEXT,
                shadow_live_wins INTEGER NOT NULL DEFAULT 0,
                retirement_json TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ekg_skill_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                skill_id TEXT NOT NULL,
                event_kind TEXT NOT NULL,
                event_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY(skill_id) REFERENCES ekg_managed_skills(id)
            );
            CREATE INDEX IF NOT EXISTS idx_ekg_managed_skills_lifecycle
                ON ekg_managed_skills(lifecycle, updated_at DESC);",
        )?;
        for column in [
            "experience_uses INTEGER NOT NULL DEFAULT 0",
            "experience_successes INTEGER NOT NULL DEFAULT 0",
            "experience_failures INTEGER NOT NULL DEFAULT 0",
        ] {
            let name = column.split_whitespace().next().unwrap();
            let exists: bool = self.conn.query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('ekg_managed_skills') WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )?;
            if !exists {
                self.conn.execute(
                    &format!("ALTER TABLE ekg_managed_skills ADD COLUMN {column}"),
                    [],
                )?;
            }
        }
        Ok(())
    }

    /// Idempotently persists an immutable candidate identity derived from its
    /// canonical JSON. The lifecycle remains candidate until an explicit,
    /// replay-backed transition occurs.
    pub(crate) fn register(&self, candidate: &SkillCandidate) -> Result<ManagedSkill, EngineError> {
        validate_candidate(candidate)?;
        let candidate_json = serde_json::to_string(candidate)?;
        let id = candidate_id(&candidate_json);
        if let Some(existing) = self.get(&id)? {
            if existing.candidate == *candidate {
                return Ok(existing);
            }
            return Err(EngineError::InvalidInput(
                "skill candidate identity conflicts with existing evidence".into(),
            ));
        }
        let now = unix_time();
        let skill = ManagedSkill {
            id,
            candidate: candidate.clone(),
            lifecycle: SkillLifecycle::Candidate,
            promotion_verdict: None,
            shadow_live_wins: 0,
            experience_uses: 0,
            experience_successes: 0,
            experience_failures: 0,
            retirement: None,
            created_at: now,
            updated_at: now,
        };
        self.conn.execute(
            "INSERT INTO ekg_managed_skills
             (id, candidate_json, lifecycle, promotion_verdict_json, shadow_live_wins,
              experience_uses, experience_successes, experience_failures,
              retirement_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, NULL, 0, 0, 0, 0, NULL, ?4, ?4)",
            params![
                skill.id,
                candidate_json,
                lifecycle_name(skill.lifecycle),
                now
            ],
        )?;
        self.record_event(&skill.id, "discovered", candidate_json, now)?;
        Ok(skill)
    }

    pub(crate) fn get(&self, id: &str) -> Result<Option<ManagedSkill>, EngineError> {
        self.conn
            .query_row(
                "SELECT id, candidate_json, lifecycle, promotion_verdict_json,
                        shadow_live_wins, experience_uses, experience_successes,
                        experience_failures, retirement_json, created_at, updated_at
                 FROM ekg_managed_skills WHERE id = ?1",
                params![id],
                row_to_skill,
            )
            .optional()
            .map_err(EngineError::from)
    }

    pub(crate) fn list(&self, limit: u32) -> Result<Vec<ManagedSkill>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT id, candidate_json, lifecycle, promotion_verdict_json,
                    shadow_live_wins, experience_uses, experience_successes,
                    experience_failures, retirement_json, created_at, updated_at
             FROM ekg_managed_skills ORDER BY updated_at DESC, id ASC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit.clamp(1, 512)], row_to_skill)?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }

    pub(crate) fn list_active(&self, limit: u32) -> Result<Vec<ManagedSkill>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT id, candidate_json, lifecycle, promotion_verdict_json,
                    shadow_live_wins, experience_uses, experience_successes,
                    experience_failures, retirement_json, created_at, updated_at
             FROM ekg_managed_skills
             WHERE lifecycle != 'retired'
             ORDER BY updated_at DESC, id ASC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit.clamp(1, 512)], row_to_skill)?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }

    pub(crate) fn rank_active(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<ManagedSkill>, EngineError> {
        let mut skills = self.list_active(512)?;
        let terms: Vec<String> = query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .filter(|term| !term.is_empty())
            .collect();
        skills.sort_by(|left, right| {
            fn score<'a>(
                skill: &'a ManagedSkill,
                terms: &[String],
            ) -> (u32, u32, u32, u32, &'a str) {
                let text = format!("{} {}", skill.candidate.name, skill.candidate.rationale)
                    .to_ascii_lowercase();
                let matches = terms
                    .iter()
                    .filter(|term| text.contains(term.as_str()))
                    .count() as u32;
                (
                    matches,
                    skill.experience_successes,
                    skill.shadow_live_wins,
                    skill
                        .experience_uses
                        .saturating_sub(skill.experience_failures),
                    skill.id.as_str(),
                )
            }
            score(right, &terms).cmp(&score(left, &terms))
        });
        skills.truncate(limit.clamp(1, 512) as usize);
        Ok(skills)
    }

    pub(crate) fn record_experience(&self, id: &str, succeeded: bool) -> Result<(), EngineError> {
        self.required(id)?;
        let now = unix_time();
        self.conn.execute(
            "UPDATE ekg_managed_skills SET experience_uses = experience_uses + 1,
             experience_successes = experience_successes + ?2,
             experience_failures = experience_failures + ?3, updated_at = ?4 WHERE id = ?1",
            params![id, i64::from(succeeded), i64::from(!succeeded), now],
        )?;
        self.record_event(
            id,
            "experience",
            serde_json::json!({"succeeded": succeeded}).to_string(),
            now,
        )
    }

    pub(crate) fn record_replay_verdict(
        &self,
        id: &str,
        verdict: &PromotionVerdict,
    ) -> Result<ManagedSkill, EngineError> {
        let skill = self.required(id)?;
        if skill.lifecycle != SkillLifecycle::Candidate {
            return Err(EngineError::InvalidInput(
                "only a candidate skill may enter shadow evaluation".into(),
            ));
        }
        let lifecycle = if verdict.shadow_eligible() {
            SkillLifecycle::Shadow
        } else {
            SkillLifecycle::Candidate
        };
        let now = unix_time();
        let verdict_json = serde_json::to_string(verdict)?;
        self.conn.execute(
            "UPDATE ekg_managed_skills
             SET lifecycle = ?2, promotion_verdict_json = ?3, updated_at = ?4
             WHERE id = ?1",
            params![id, lifecycle_name(lifecycle), verdict_json, now],
        )?;
        self.record_event(id, "replay_verdict", serde_json::to_string(verdict)?, now)?;
        self.required(id)
    }

    pub(crate) fn promote_from_live_shadow(
        &self,
        id: &str,
        episode_id: &str,
    ) -> Result<ManagedSkill, EngineError> {
        let skill = self.required(id)?;
        if skill.lifecycle != SkillLifecycle::Shadow {
            return Err(EngineError::InvalidInput(
                "only a shadow skill can be promoted by a live win".into(),
            ));
        }
        let now = unix_time();
        self.conn.execute(
            "UPDATE ekg_managed_skills
             SET lifecycle = ?2, shadow_live_wins = shadow_live_wins + 1, updated_at = ?3
             WHERE id = ?1",
            params![id, lifecycle_name(SkillLifecycle::Promoted), now],
        )?;
        self.record_event(
            id,
            "live_shadow_win",
            serde_json::json!({ "episodeId": episode_id }).to_string(),
            now,
        )?;
        self.required(id)
    }

    pub(crate) fn retire(
        &self,
        id: &str,
        record: &RetirementRecord,
    ) -> Result<ManagedSkill, EngineError> {
        let skill = self.required(id)?;
        if skill.lifecycle == SkillLifecycle::Retired {
            return Ok(skill);
        }
        if !record.reconstructible || record.retired_skill != id {
            return Err(EngineError::InvalidInput(
                "retirement must retain a reconstructible record for this skill".into(),
            ));
        }
        let now = unix_time();
        let record_json = serde_json::to_string(record)?;
        self.conn.execute(
            "UPDATE ekg_managed_skills
             SET lifecycle = ?2, retirement_json = ?3, updated_at = ?4 WHERE id = ?1",
            params![
                id,
                lifecycle_name(SkillLifecycle::Retired),
                record_json,
                now
            ],
        )?;
        self.record_event(id, "retired", serde_json::to_string(record)?, now)?;
        self.required(id)
    }

    fn required(&self, id: &str) -> Result<ManagedSkill, EngineError> {
        self.get(id)?
            .ok_or_else(|| EngineError::InvalidInput(format!("unknown managed skill {id}")))
    }

    fn record_event(
        &self,
        id: &str,
        kind: &str,
        event_json: String,
        created_at: i64,
    ) -> Result<(), EngineError> {
        self.conn.execute(
            "INSERT INTO ekg_skill_events (skill_id, event_kind, event_json, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, kind, event_json, created_at],
        )?;
        Ok(())
    }
}

fn row_to_skill(row: &rusqlite::Row<'_>) -> rusqlite::Result<ManagedSkill> {
    fn decode<T: for<'de> Deserialize<'de>>(column: usize, value: String) -> rusqlite::Result<T> {
        serde_json::from_str(&value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    }
    let lifecycle: String = row.get(2)?;
    let lifecycle = lifecycle_from_name(&lifecycle).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            "invalid skill lifecycle".into(),
        )
    })?;
    let verdict: Option<String> = row.get(3)?;
    let retirement: Option<String> = row.get(8)?;
    Ok(ManagedSkill {
        id: row.get(0)?,
        candidate: decode(1, row.get(1)?)?,
        lifecycle,
        promotion_verdict: verdict.map(|value| decode(3, value)).transpose()?,
        shadow_live_wins: row.get(4)?,
        experience_uses: row.get(5)?,
        experience_successes: row.get(6)?,
        experience_failures: row.get(7)?,
        retirement: retirement.map(|value| decode(8, value)).transpose()?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn validate_candidate(candidate: &SkillCandidate) -> Result<(), EngineError> {
    if candidate.name.trim().is_empty()
        || candidate.rationale.trim().is_empty()
        || candidate.source_episode_ids.is_empty()
        || candidate.support_count == 0
        || candidate.support_count as usize != candidate.source_episode_ids.len()
    {
        return Err(EngineError::InvalidInput(
            "skill candidate requires a name, rationale, and exact source episode support".into(),
        ));
    }
    let mut sources = candidate.source_episode_ids.clone();
    sources.sort_unstable_by_key(|id| id.to_string());
    sources.dedup();
    if sources.len() != candidate.source_episode_ids.len() {
        return Err(EngineError::InvalidInput(
            "skill candidate source episodes must be unique".into(),
        ));
    }
    Ok(())
}

fn candidate_id(candidate_json: &str) -> String {
    let digest = Sha256::digest(candidate_json.as_bytes());
    format!("skill-{:x}", digest)
}

fn lifecycle_name(value: SkillLifecycle) -> &'static str {
    match value {
        SkillLifecycle::Candidate => "candidate",
        SkillLifecycle::Shadow => "shadow",
        SkillLifecycle::Promoted => "promoted",
        SkillLifecycle::Retired => "retired",
    }
}

fn lifecycle_from_name(value: &str) -> Option<SkillLifecycle> {
    match value {
        "candidate" => Some(SkillLifecycle::Candidate),
        "shadow" => Some(SkillLifecycle::Shadow),
        "promoted" => Some(SkillLifecycle::Promoted),
        "retired" => Some(SkillLifecycle::Retired),
        _ => None,
    }
}

fn unix_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
