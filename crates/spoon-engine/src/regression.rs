//! Append-only verified answer history used by the Phase 4 regression gate.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use spoon_core::{Episode, EpisodeId, EscalationRung, ProcedureId, Value, VerifiabilityTier};
use std::collections::BTreeMap;

use crate::EngineError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedAnswerRecord {
    pub episode_id: spoon_core::EpisodeId,
    pub situation: String,
    pub environment: BTreeMap<String, Value>,
    pub observed_result: Value,
    pub tier: VerifiabilityTier,
    pub rung: EscalationRung,
    pub created_at: i64,
}

/// The smallest durable suite that may authorize a broad mutation. A broad
/// change with no independently verified behavior to preserve is deliberately
/// not promoted; callers must make a narrow/local correction instead.
pub const MIN_BROAD_REGRESSION_CASES: u32 = 1;

/// Outcome for one immutable, verified regression case replayed against a
/// candidate broad change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegressionSuiteCaseResult {
    pub episode_id: EpisodeId,
    pub procedure_id: ProcedureId,
    pub procedure_version: u32,
    pub expected_output: Value,
    pub actual_output: Option<Value>,
    pub status: RegressionSuiteCaseStatus,
    pub details: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegressionSuiteCaseStatus {
    Passed,
    Failed,
    Inapplicable,
}

/// A report of replaying the durable verified regression suite. The verdict
/// is stored against the adaptation plan even when it rejects the mutation,
/// so a failed gate cannot be hidden by retrying the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegressionSuiteVerdict {
    pub required_minimum: u32,
    pub applicable: u32,
    pub passed: u32,
    pub failed: u32,
    pub inapplicable: u32,
    pub accepted: bool,
    pub cases: Vec<RegressionSuiteCaseResult>,
}

impl RegressionSuiteVerdict {
    pub(crate) fn empty() -> Self {
        Self {
            required_minimum: MIN_BROAD_REGRESSION_CASES,
            applicable: 0,
            passed: 0,
            failed: 0,
            inapplicable: 0,
            accepted: false,
            cases: Vec::new(),
        }
    }

    pub(crate) fn finalize(&mut self) {
        self.applicable = self.passed.saturating_add(self.failed);
        self.accepted = self.applicable >= self.required_minimum && self.failed == 0;
    }
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
            "CREATE TABLE IF NOT EXISTS spoon_verified_answers (
                episode_id TEXT PRIMARY KEY,
                situation TEXT NOT NULL,
                environment_json TEXT NOT NULL,
                observed_json TEXT NOT NULL,
                tier_json TEXT NOT NULL,
                rung_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_spoon_verified_answers_situation
                ON spoon_verified_answers(situation, created_at DESC);",
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
            "INSERT OR IGNORE INTO spoon_verified_answers
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
             FROM spoon_verified_answers ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit.clamp(1, 512)], |row| {
            let id: String = row.get(0)?;
            let episode_id = uuid::Uuid::parse_str(&id)
                .map(spoon_core::EpisodeId)
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

    pub(crate) fn count(&self) -> Result<u64, EngineError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM spoon_verified_answers", [], |row| {
                row.get::<_, i64>(0)
            })? as u64)
    }
}
