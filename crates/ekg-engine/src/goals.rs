use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::EngineError;

const MAX_GOAL_TEXT: usize = 2_048;
const MAX_GAPS: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalKind {
    Task,
    Standing,
    Instrumental,
    Learning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub id: String,
    pub kind: GoalKind,
    pub statement: String,
    pub parent_id: Option<String>,
    pub immutable: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    Structural,
    Functional,
    RepeatedImpass,
    Contradiction,
    FailedPrediction,
    Ungrounded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CuriosityGap {
    pub id: String,
    pub kind: GapKind,
    pub statement: String,
    pub blast_radius: f64,
    pub goal_relevance: f64,
    pub learning_progress: f64,
    pub cost_to_close: f64,
    pub value_score: f64,
    pub source_episode: Option<String>,
    pub resolved: bool,
    pub created_at: i64,
}

pub struct GoalStore {
    conn: Connection,
}

impl GoalStore {
    pub fn open(path: &str) -> Result<Self, EngineError> {
        let store = Self {
            conn: Connection::open(path)?,
        };
        store.schema()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.schema()?;
        Ok(store)
    }

    fn schema(&self) -> Result<(), EngineError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ekg_goals (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                statement TEXT NOT NULL,
                parent_id TEXT,
                immutable INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ekg_curiosity_gaps (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                statement TEXT NOT NULL,
                blast_radius REAL NOT NULL,
                goal_relevance REAL NOT NULL,
                learning_progress REAL NOT NULL,
                cost_to_close REAL NOT NULL,
                value_score REAL NOT NULL,
                source_episode TEXT,
                resolved INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_ekg_gaps_rank
                ON ekg_curiosity_gaps(resolved, value_score DESC, created_at ASC);",
        )?;
        Ok(())
    }

    pub fn create_goal(
        &self,
        kind: GoalKind,
        statement: &str,
        parent_id: Option<&str>,
    ) -> Result<Goal, EngineError> {
        let statement = bounded_text(statement)?;
        if matches!(kind, GoalKind::Standing) && parent_id.is_some() {
            return Err(EngineError::InvalidInput(
                "standing goals cannot be instrumental children".into(),
            ));
        }
        let goal = Goal {
            id: Uuid::new_v4().to_string(),
            kind,
            statement,
            parent_id: parent_id.map(str::to_owned),
            immutable: matches!(kind, GoalKind::Standing),
            created_at: unix_time(),
        };
        self.conn.execute(
            "INSERT INTO ekg_goals (id, kind, statement, parent_id, immutable, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                goal.id,
                serde_json::to_string(&goal.kind)?,
                goal.statement,
                goal.parent_id,
                goal.immutable,
                goal.created_at
            ],
        )?;
        Ok(goal)
    }

    pub fn list_goals(&self) -> Result<Vec<Goal>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT id, kind, statement, parent_id, immutable, created_at
             FROM ekg_goals ORDER BY created_at ASC, id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let kind: String = row.get(1)?;
            Ok(Goal {
                id: row.get(0)?,
                kind: serde_json::from_str(&kind).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                statement: row.get(2)?,
                parent_id: row.get(3)?,
                immutable: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }

    pub fn record_gap(&self, gap: &CuriosityGap) -> Result<(), EngineError> {
        let statement = bounded_text(&gap.statement)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO ekg_curiosity_gaps
             (id, kind, statement, blast_radius, goal_relevance, learning_progress,
              cost_to_close, value_score, source_episode, resolved, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                gap.id,
                serde_json::to_string(&gap.kind)?,
                statement,
                finite_nonnegative(gap.blast_radius)?,
                finite_nonnegative(gap.goal_relevance)?,
                finite_nonnegative(gap.learning_progress)?,
                finite_positive(gap.cost_to_close)?,
                finite_nonnegative(gap.value_score)?,
                gap.source_episode,
                gap.resolved,
                gap.created_at
            ],
        )?;
        Ok(())
    }

    pub fn rank_gaps(&self, limit: u32) -> Result<Vec<CuriosityGap>, EngineError> {
        let limit = limit.clamp(1, MAX_GAPS);
        let mut statement = self.conn.prepare(
            "SELECT id, kind, statement, blast_radius, goal_relevance, learning_progress,
                    cost_to_close, value_score, source_episode, resolved, created_at
             FROM ekg_curiosity_gaps WHERE resolved = 0
             ORDER BY value_score DESC, created_at ASC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit], |row| {
            let kind: String = row.get(1)?;
            Ok(CuriosityGap {
                id: row.get(0)?,
                kind: serde_json::from_str(&kind).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                statement: row.get(2)?,
                blast_radius: row.get(3)?,
                goal_relevance: row.get(4)?,
                learning_progress: row.get(5)?,
                cost_to_close: row.get(6)?,
                value_score: row.get(7)?,
                source_episode: row.get(8)?,
                resolved: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }
}

fn bounded_text(value: &str) -> Result<String, EngineError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_GOAL_TEXT {
        return Err(EngineError::InvalidInput(
            "goal or curiosity statement is empty or too long".into(),
        ));
    }
    Ok(value.to_owned())
}

fn finite_nonnegative(value: f64) -> Result<f64, EngineError> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(EngineError::InvalidInput(
            "score must be finite and non-negative".into(),
        ))
    }
}

fn finite_positive(value: f64) -> Result<f64, EngineError> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(EngineError::InvalidInput(
            "cost must be finite and positive".into(),
        ))
    }
}

fn unix_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::{CuriosityGap, GapKind, GoalKind, GoalStore};

    #[test]
    fn standing_goals_are_immutable_and_gaps_are_ranked() {
        let store = GoalStore::in_memory().unwrap();
        let standing = store
            .create_goal(GoalKind::Standing, "stay accurate", None)
            .unwrap();
        assert!(standing.immutable);
        let low = CuriosityGap {
            id: "low".into(),
            kind: GapKind::Structural,
            statement: "low".into(),
            blast_radius: 1.0,
            goal_relevance: 1.0,
            learning_progress: 0.1,
            cost_to_close: 2.0,
            value_score: 0.5,
            source_episode: None,
            resolved: false,
            created_at: 1,
        };
        let high = CuriosityGap {
            id: "high".into(),
            kind: GapKind::FailedPrediction,
            statement: "high".into(),
            blast_radius: 3.0,
            goal_relevance: 2.0,
            learning_progress: 1.0,
            cost_to_close: 1.0,
            value_score: 6.0,
            source_episode: None,
            resolved: false,
            created_at: 2,
        };
        store.record_gap(&low).unwrap();
        store.record_gap(&high).unwrap();
        assert_eq!(store.rank_gaps(2).unwrap()[0].id, "high");
    }
}
