//! Durable, non-destructive episode compression records.
//!
//! Compression never deletes the source episode. It stores a bounded summary
//! and an archived copy so later extraction, audit, and reconstruction remain
//! possible; failures are intentionally excluded from summarization by the
//! planning function.

use ekg_adapt::EpisodeCompressionPlan;
use ekg_core::{Episode, EpisodeId};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::EngineError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeCompressionRecord {
    pub episode_id: EpisodeId,
    pub summary: serde_json::Value,
    pub archived_episode: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeCompressionResult {
    pub plan: EpisodeCompressionPlan,
    pub archived_episode_ids: Vec<EpisodeId>,
}

pub(crate) struct CompressionStore {
    conn: Connection,
}

impl CompressionStore {
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
            "CREATE TABLE IF NOT EXISTS ekg_episode_compression_records (
                episode_id TEXT PRIMARY KEY,
                summary_json TEXT NOT NULL,
                archived_episode_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TRIGGER IF NOT EXISTS ekg_episode_compression_no_update
            BEFORE UPDATE ON ekg_episode_compression_records BEGIN
                SELECT RAISE(ABORT, 'episode compression records are immutable');
            END;
            CREATE TRIGGER IF NOT EXISTS ekg_episode_compression_no_delete
            BEFORE DELETE ON ekg_episode_compression_records BEGIN
                SELECT RAISE(ABORT, 'episode compression records are immutable');
            END;",
        )?;
        Ok(())
    }

    pub(crate) fn apply(
        &self,
        episodes: &[Episode],
        plan: EpisodeCompressionPlan,
    ) -> Result<EpisodeCompressionResult, EngineError> {
        let mut archived_episode_ids = Vec::new();
        for episode_id in &plan.summarize {
            let Some(episode) = episodes.iter().find(|episode| episode.id == *episode_id) else {
                return Err(EngineError::InvalidInput(format!(
                    "compression plan references unknown episode {episode_id}"
                )));
            };
            if episode.failed() {
                return Err(EngineError::InvalidInput(
                    "failed episodes cannot be compressed".into(),
                ));
            }
            let summary = serde_json::json!({
                "episodeId": episode.id,
                "situation": episode.situation,
                "action": episode.action,
                "evaluation": episode.evaluation,
                "observedResult": episode.observed_result,
                "rung": episode.cost.rung_reached,
                "steps": episode.cost.steps_taken,
            });
            let archived = serde_json::to_value(episode)?;
            self.conn.execute(
                "INSERT OR IGNORE INTO ekg_episode_compression_records
                 (episode_id, summary_json, archived_episode_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    episode.id.to_string(),
                    serde_json::to_string(&summary)?,
                    serde_json::to_string(&archived)?,
                    unix_time()
                ],
            )?;
            archived_episode_ids.push(episode.id);
        }
        Ok(EpisodeCompressionResult {
            plan,
            archived_episode_ids,
        })
    }

    pub(crate) fn list(&self, limit: u32) -> Result<Vec<EpisodeCompressionRecord>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT episode_id, summary_json, archived_episode_json, created_at
             FROM ekg_episode_compression_records ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit.clamp(1, 512)], |row| {
            let id: String = row.get(0)?;
            let episode_id = uuid::Uuid::parse_str(&id).map(EpisodeId).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    "invalid episode id".into(),
                )
            })?;
            let summary: String = row.get(1)?;
            let archived: String = row.get(2)?;
            Ok(EpisodeCompressionRecord {
                episode_id,
                summary: serde_json::from_str(&summary).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                archived_episode: serde_json::from_str(&archived).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                created_at: row.get(3)?,
            })
        })?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }
}

fn unix_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
