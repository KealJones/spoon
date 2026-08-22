//! Append-only verified answer history used by the Phase 4 regression gate.

use ekg_core::{Episode, EscalationRung, Value, VerifiabilityTier};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::EngineError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedAnswerRecord {
    pub episode_id: ekg_core::EpisodeId,
    pub situation: String,
    pub environment: BTreeMap<String, Value>,
    pub observed_result: Value,
    pub tier: VerifiabilityTier,
    pub rung: EscalationRung,
    pub created_at: i64,
}

pub(crate) struct RegressionStore {
    conn: Connection,
}

impl RegressionStore {
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
            "CREATE TABLE IF NOT EXISTS ekg_verified_answers (
                episode_id TEXT PRIMARY KEY,
                situation TEXT NOT NULL,
                environment_json TEXT NOT NULL,
                observed_json TEXT NOT NULL,
                tier_json TEXT NOT NULL,
                rung_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_ekg_verified_answers_situation
                ON ekg_verified_answers(situation, created_at DESC);",
        )?;
        Ok(())
    }

    pub(crate) fn record(&self, episode: &Episode) -> Result<(), EngineError> {
        let Some(evaluation) = episode.evaluation.as_ref() else {
            return Ok(());
        };
        let Some(observed_result) = episode.observed_result.as_ref() else {
            return Ok(());
        };
        if !evaluation.success
            || !matches!(
                evaluation.tier,
                VerifiabilityTier::Hard | VerifiabilityTier::Consensus
            )
        {
            return Ok(());
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO ekg_verified_answers
             (episode_id, situation, environment_json, observed_json, tier_json, rung_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                episode.id.to_string(),
                episode.situation,
                serde_json::to_string(&episode.context.environment)?,
                serde_json::to_string(observed_result)?,
                serde_json::to_string(&evaluation.tier)?,
                serde_json::to_string(&episode.cost.rung_reached)?,
                episode.created_at,
            ],
        )?;
        Ok(())
    }

    pub(crate) fn list(&self, limit: u32) -> Result<Vec<VerifiedAnswerRecord>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT episode_id, situation, environment_json, observed_json,
                    tier_json, rung_json, created_at
             FROM ekg_verified_answers ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit.clamp(1, 512)], |row| {
            let id: String = row.get(0)?;
            let episode_id = uuid::Uuid::parse_str(&id)
                .map(ekg_core::EpisodeId)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(VerifiedAnswerRecord {
                episode_id,
                situation: row.get(1)?,
                environment: serde_json::from_str(&row.get::<_, String>(2)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                observed_result: serde_json::from_str(&row.get::<_, String>(3)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                tier: serde_json::from_str(&row.get::<_, String>(4)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                rung: serde_json::from_str(&row.get::<_, String>(5)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }
}
