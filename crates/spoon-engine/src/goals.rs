use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spoon_core::{Episode, VerifiabilityTier};
use uuid::Uuid;

use crate::EngineError;

const MAX_GOAL_TEXT: usize = 2_048;
const MAX_GAPS: u32 = 256;
const MAX_SCHEDULED_ACTIONS: u32 = 64;
const MAX_LEARNING_ACTION_STEPS: u32 = 32;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalLearningRecord {
    pub learning_goal_id: String,
    pub standing_goal_id: String,
    pub source_gap_id: String,
    pub derivation_reason: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalDerivationRecord {
    pub goal_id: String,
    pub parent_goal_id: String,
    pub derivation_reason: String,
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

/// The only actions the curiosity scheduler may propose.  These are requests
/// for bounded evidence work, not permissions to change knowledge, procedures,
/// or capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningActionKind {
    ReviewPredictionEvidence,
    InspectRepeatedImpass,
    ResolveHeldContradiction,
    GatherStructuralEvidence,
    GatherFunctionalEvidence,
    GroundObservation,
}

/// A durable, idempotent proposal for the next bounded learning action.
/// Scheduling is intentionally separated from execution: this record carries
/// no authority to mutate the graph or capability store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledLearningAction {
    pub id: String,
    pub source_goal_id: String,
    pub source_goal_kind: GoalKind,
    pub source_gap_id: String,
    pub kind: LearningActionKind,
    pub instruction: String,
    pub max_steps: u32,
    pub value_score: f64,
    pub allows_graph_mutation: bool,
    pub allows_capability_mutation: bool,
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
            "PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS spoon_goals (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                statement TEXT NOT NULL,
                parent_id TEXT,
                immutable INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS spoon_curiosity_gaps (
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
            CREATE INDEX IF NOT EXISTS idx_spoon_gaps_rank
                ON spoon_curiosity_gaps(resolved, value_score DESC, created_at ASC);
            CREATE TABLE IF NOT EXISTS spoon_scheduled_learning_actions (
                id TEXT PRIMARY KEY,
                source_goal_id TEXT NOT NULL REFERENCES spoon_goals(id),
                source_goal_kind TEXT NOT NULL,
                source_gap_id TEXT NOT NULL UNIQUE REFERENCES spoon_curiosity_gaps(id),
                kind TEXT NOT NULL,
                instruction TEXT NOT NULL,
                max_steps INTEGER NOT NULL CHECK(max_steps > 0 AND max_steps <= 32),
                value_score REAL NOT NULL,
                allows_graph_mutation INTEGER NOT NULL CHECK(allows_graph_mutation = 0),
                allows_capability_mutation INTEGER NOT NULL CHECK(allows_capability_mutation = 0),
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_spoon_scheduled_learning_actions_goal
                ON spoon_scheduled_learning_actions(source_goal_id, created_at ASC);
            CREATE TABLE IF NOT EXISTS spoon_goal_learning_records (
                learning_goal_id TEXT PRIMARY KEY NOT NULL
                    REFERENCES spoon_goals(id),
                standing_goal_id TEXT NOT NULL
                    REFERENCES spoon_goals(id),
                source_gap_id TEXT NOT NULL
                    REFERENCES spoon_curiosity_gaps(id),
                derivation_reason TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_spoon_goal_learning_standing
                ON spoon_goal_learning_records(standing_goal_id, created_at ASC);
            CREATE TABLE IF NOT EXISTS spoon_goal_derivation_records (
                goal_id TEXT PRIMARY KEY NOT NULL REFERENCES spoon_goals(id),
                parent_goal_id TEXT NOT NULL REFERENCES spoon_goals(id),
                derivation_reason TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_spoon_goal_derivation_parent
                ON spoon_goal_derivation_records(parent_goal_id, created_at ASC);
            CREATE TRIGGER IF NOT EXISTS spoon_goals_immutable_update
            BEFORE UPDATE OF kind, statement, parent_id, immutable ON spoon_goals
            WHEN OLD.immutable = 1
            BEGIN
                SELECT RAISE(ABORT, 'immutable standing goals cannot be mutated');
            END;
            CREATE TRIGGER IF NOT EXISTS spoon_goals_immutable_delete
            BEFORE DELETE ON spoon_goals
            WHEN OLD.immutable = 1
            BEGIN
                SELECT RAISE(ABORT, 'immutable standing goals cannot be deleted');
            END;",
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
        if matches!(kind, GoalKind::Instrumental | GoalKind::Learning) {
            return Err(EngineError::InvalidInput(
                "derived goals require a goal-bound derivation API".into(),
            ));
        }
        if parent_id.is_some() {
            return Err(EngineError::InvalidInput(
                "externally supplied task and standing goals cannot have parents".into(),
            ));
        }
        // Standing goals are persistent user intent.  Repeating the exact
        // declaration must not create competing roots or weaken immutability.
        if kind == GoalKind::Standing
            && let Some(existing) = self.find_standing_goal(&statement)?
        {
            return Ok(existing);
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
            "INSERT INTO spoon_goals (id, kind, statement, parent_id, immutable, created_at)
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

    pub fn create_learning_goal(
        &self,
        statement: &str,
        standing_goal_id: &str,
        source_gap_id: &str,
        derivation_reason: &str,
    ) -> Result<Goal, EngineError> {
        let statement = bounded_text(statement)?;
        let derivation_reason = bounded_text(derivation_reason)?;
        let transaction = self.conn.unchecked_transaction()?;

        if let Some(existing) = transaction
            .query_row(
                "SELECT id, kind, statement, parent_id, immutable, created_at
                 FROM spoon_goals WHERE id IN (
                    SELECT learning_goal_id FROM spoon_goal_learning_records
                    WHERE standing_goal_id = ?1 AND source_gap_id = ?2
                 )",
                params![standing_goal_id, source_gap_id],
                decode_goal_row,
            )
            .optional()?
        {
            transaction.commit()?;
            return Ok(existing);
        }

        let standing_kind = transaction
            .query_row(
                "SELECT kind FROM spoon_goals
                 WHERE id = ?1 AND immutable = 1",
                params![standing_goal_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(standing_kind) = standing_kind else {
            return Err(EngineError::InvalidInput(
                "learning goals require an existing immutable standing goal".into(),
            ));
        };
        if serde_json::from_str::<GoalKind>(&standing_kind)? != GoalKind::Standing {
            return Err(EngineError::InvalidInput(
                "learning goals require an existing immutable standing goal".into(),
            ));
        }

        let gap_resolved = transaction
            .query_row(
                "SELECT resolved FROM spoon_curiosity_gaps WHERE id = ?1",
                params![source_gap_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?;
        match gap_resolved {
            Some(false) => {}
            Some(true) => {
                return Err(EngineError::InvalidInput(
                    "resolved curiosity gaps cannot authorize learning goals".into(),
                ));
            }
            None => {
                return Err(EngineError::InvalidInput(
                    "learning goals require an existing curiosity gap".into(),
                ));
            }
        }

        let created_at = unix_time();
        let goal = Goal {
            id: Uuid::new_v4().to_string(),
            kind: GoalKind::Learning,
            statement,
            parent_id: Some(standing_goal_id.to_owned()),
            immutable: false,
            created_at,
        };
        transaction.execute(
            "INSERT INTO spoon_goals (id, kind, statement, parent_id, immutable, created_at)
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
        transaction.execute(
            "INSERT INTO spoon_goal_learning_records
             (learning_goal_id, standing_goal_id, source_gap_id, derivation_reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                goal.id,
                standing_goal_id,
                source_gap_id,
                derivation_reason,
                created_at
            ],
        )?;
        transaction.commit()?;
        Ok(goal)
    }

    pub fn create_instrumental_goal(
        &self,
        statement: &str,
        parent_goal_id: &str,
        derivation_reason: &str,
    ) -> Result<Goal, EngineError> {
        let statement = bounded_text(statement)?;
        let derivation_reason = bounded_text(derivation_reason)?;
        let transaction = self.conn.unchecked_transaction()?;
        let parent_exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM spoon_goals WHERE id = ?1)",
            params![parent_goal_id],
            |row| row.get(0),
        )?;
        if !parent_exists {
            return Err(EngineError::InvalidInput(
                "instrumental goals require an existing parent goal".into(),
            ));
        }
        let created_at = unix_time();
        let goal = Goal {
            id: Uuid::new_v4().to_string(),
            kind: GoalKind::Instrumental,
            statement,
            parent_id: Some(parent_goal_id.to_owned()),
            immutable: false,
            created_at,
        };
        transaction.execute(
            "INSERT INTO spoon_goals (id, kind, statement, parent_id, immutable, created_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            params![
                goal.id,
                serde_json::to_string(&goal.kind)?,
                goal.statement,
                goal.parent_id,
                goal.created_at
            ],
        )?;
        transaction.execute(
            "INSERT INTO spoon_goal_derivation_records
             (goal_id, parent_goal_id, derivation_reason, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![goal.id, parent_goal_id, derivation_reason, created_at],
        )?;
        transaction.commit()?;
        Ok(goal)
    }

    pub fn list_goal_derivation_records(&self) -> Result<Vec<GoalDerivationRecord>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT goal_id, parent_goal_id, derivation_reason, created_at
             FROM spoon_goal_derivation_records ORDER BY created_at ASC, goal_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(GoalDerivationRecord {
                goal_id: row.get(0)?,
                parent_goal_id: row.get(1)?,
                derivation_reason: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }

    pub fn list_learning_goal_records(&self) -> Result<Vec<GoalLearningRecord>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT learning_goal_id, standing_goal_id, source_gap_id,
                    derivation_reason, created_at
             FROM spoon_goal_learning_records
             ORDER BY created_at ASC, learning_goal_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(GoalLearningRecord {
                learning_goal_id: row.get(0)?,
                standing_goal_id: row.get(1)?,
                source_gap_id: row.get(2)?,
                derivation_reason: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }

    pub fn list_goals(&self) -> Result<Vec<Goal>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT id, kind, statement, parent_id, immutable, created_at
             FROM spoon_goals ORDER BY created_at ASC, id ASC",
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
        let blast_radius = finite_nonnegative(gap.blast_radius)?;
        let cost_to_close = finite_positive(gap.cost_to_close)?;
        let goal_relevance = self.goal_relevance(&statement)?;
        let learning_progress = self.learning_progress(gap, &statement)?;
        // Value is never caller authority.  The caller/evidence can describe
        // blast radius and cost, but the stored score is recomputed from the
        // durable goal and gap history below.
        let value_score = curiosity_value_score(
            blast_radius,
            goal_relevance,
            learning_progress,
            cost_to_close,
        );
        self.conn.execute(
            "INSERT INTO spoon_curiosity_gaps
             (id, kind, statement, blast_radius, goal_relevance, learning_progress,
              cost_to_close, value_score, source_episode, resolved, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                kind = excluded.kind,
                statement = excluded.statement,
                blast_radius = excluded.blast_radius,
                goal_relevance = excluded.goal_relevance,
                learning_progress = excluded.learning_progress,
                cost_to_close = excluded.cost_to_close,
                value_score = excluded.value_score,
                source_episode = excluded.source_episode,
                resolved = excluded.resolved,
                created_at = excluded.created_at",
            params![
                gap.id,
                serde_json::to_string(&gap.kind)?,
                statement,
                blast_radius,
                goal_relevance,
                learning_progress,
                cost_to_close,
                value_score,
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
             FROM spoon_curiosity_gaps WHERE resolved = 0
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

    /// Stores the highest-value unsolved gap as one bounded next action.  A
    /// source gap may have exactly one scheduled action, so retries are
    /// idempotent and retain their original goal derivation.
    pub fn schedule_next_learning_action(
        &self,
    ) -> Result<Option<ScheduledLearningAction>, EngineError> {
        let Some(gap) = self.rank_gaps(1)?.into_iter().next() else {
            return Ok(None);
        };
        if let Some(existing) = self.scheduled_for_gap(&gap.id)? {
            return Ok(Some(existing));
        }
        let Some(goal) = self.best_authorizing_goal(&gap.statement)? else {
            // Curiosity can be recorded without a goal, but acting on it
            // autonomously cannot.  This is an intentional no-op.
            return Ok(None);
        };
        let kind = action_kind_for_gap(gap.kind);
        let action = ScheduledLearningAction {
            id: stable_id("learning-action", &format!("{}:{}", goal.id, gap.id)),
            source_goal_id: goal.id.clone(),
            source_goal_kind: goal.kind,
            source_gap_id: gap.id.clone(),
            kind,
            instruction: learning_instruction(&goal, &gap, kind),
            max_steps: estimated_steps(gap.cost_to_close),
            value_score: gap.value_score,
            allows_graph_mutation: false,
            allows_capability_mutation: false,
            created_at: unix_time(),
        };
        self.conn.execute(
            "INSERT INTO spoon_scheduled_learning_actions
             (id, source_goal_id, source_goal_kind, source_gap_id, kind, instruction,
              max_steps, value_score, allows_graph_mutation, allows_capability_mutation,
              created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, ?9)
             ON CONFLICT(source_gap_id) DO NOTHING",
            params![
                action.id,
                action.source_goal_id,
                serde_json::to_string(&action.source_goal_kind)?,
                action.source_gap_id,
                serde_json::to_string(&action.kind)?,
                action.instruction,
                action.max_steps,
                action.value_score,
                action.created_at,
            ],
        )?;
        // A concurrent/recovery call may have inserted it after our read.
        Ok(self.scheduled_for_gap(&gap.id)?.or(Some(action)))
    }

    pub fn list_scheduled_learning_actions(
        &self,
        limit: u32,
    ) -> Result<Vec<ScheduledLearningAction>, EngineError> {
        let limit = limit.clamp(1, MAX_SCHEDULED_ACTIONS);
        let mut statement = self.conn.prepare(
            "SELECT id, source_goal_id, source_goal_kind, source_gap_id, kind,
                    instruction, max_steps, value_score, allows_graph_mutation,
                    allows_capability_mutation, created_at
             FROM spoon_scheduled_learning_actions
             ORDER BY created_at ASC, id ASC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit], decode_scheduled_action_row)?;
        rows.map(|row| row.map_err(EngineError::from)).collect()
    }

    fn find_standing_goal(&self, statement: &str) -> Result<Option<Goal>, EngineError> {
        self.conn
            .query_row(
                "SELECT id, kind, statement, parent_id, immutable, created_at
                 FROM spoon_goals WHERE kind = ?1 AND statement = ?2",
                params![serde_json::to_string(&GoalKind::Standing)?, statement],
                decode_goal_row,
            )
            .optional()
            .map_err(EngineError::from)
    }

    fn goal_relevance(&self, gap_statement: &str) -> Result<f64, EngineError> {
        let goals = self.list_goals()?;
        let mut best: f64 = 0.0;
        for goal in goals
            .into_iter()
            .filter(|goal| matches!(goal.kind, GoalKind::Standing | GoalKind::Task))
        {
            let lexical = text_relevance(gap_statement, &goal.statement);
            // A declared goal has minimum relevance to bounded curiosity. A
            // standing goal carries a small policy premium, while lexical
            // overlap makes that relationship visible in the resulting score.
            let base = if goal.kind == GoalKind::Standing {
                0.5
            } else {
                0.35
            };
            best = best.max(base + lexical);
        }
        Ok(best)
    }

    fn learning_progress(&self, gap: &CuriosityGap, statement: &str) -> Result<f64, EngineError> {
        let prior: u32 = self.conn.query_row(
            "SELECT COUNT(*) FROM spoon_curiosity_gaps
             WHERE kind = ?1 AND id != ?2 AND resolved = 0",
            params![serde_json::to_string(&gap.kind)?, gap.id],
            |row| row.get(0),
        )?;
        let corroborating: u32 = self.conn.query_row(
            "SELECT COUNT(*) FROM spoon_curiosity_gaps
             WHERE id != ?1 AND resolved = 0 AND statement = ?2",
            params![gap.id, statement],
            |row| row.get(0),
        )?;
        let evidence_progress = finite_nonnegative(gap.learning_progress)?.min(1.0);
        let history_progress =
            (f64::from(prior.min(8)) * 0.08) + (f64::from(corroborating.min(4)) * 0.12);
        Ok((evidence_progress + history_progress).min(1.0))
    }

    fn best_authorizing_goal(&self, gap_statement: &str) -> Result<Option<Goal>, EngineError> {
        let mut candidates = self
            .list_goals()?
            .into_iter()
            .filter(|goal| matches!(goal.kind, GoalKind::Standing | GoalKind::Task))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            let right_score = text_relevance(gap_statement, &right.statement);
            let left_score = text_relevance(gap_statement, &left.statement);
            right_score
                .total_cmp(&left_score)
                .then_with(|| {
                    matches!(right.kind, GoalKind::Standing)
                        .cmp(&matches!(left.kind, GoalKind::Standing))
                })
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(candidates.into_iter().next())
    }

    fn scheduled_for_gap(
        &self,
        gap_id: &str,
    ) -> Result<Option<ScheduledLearningAction>, EngineError> {
        self.conn
            .query_row(
                "SELECT id, source_goal_id, source_goal_kind, source_gap_id, kind,
                        instruction, max_steps, value_score, allows_graph_mutation,
                        allows_capability_mutation, created_at
                 FROM spoon_scheduled_learning_actions WHERE source_gap_id = ?1",
                params![gap_id],
                decode_scheduled_action_row,
            )
            .optional()
            .map_err(EngineError::from)
    }
}

/// Derives bounded gap candidates only from finalized episode material.  The
/// caller persists them through `record_gap`, which recomputes value against
/// durable goals and prior gap history.
pub(crate) fn derive_episode_curiosity_gaps(
    episode: &Episode,
    recent_episodes: &[Episode],
) -> Vec<CuriosityGap> {
    let mut gaps = Vec::new();
    let source_episode = Some(episode.id.to_string());
    let situation_key = normalized_text(&episode.situation);
    let cost = estimated_evidence_cost(episode);

    if episode.failed() && episode.prediction.is_some() {
        gaps.push(evidence_gap(
            format!("episode:{}:failed-prediction", episode.id),
            GapKind::FailedPrediction,
            format!("failed prediction in {}", episode.situation),
            episode,
            1.0,
            cost,
            source_episode.clone(),
        ));
    }

    let impasse_count = recent_episodes
        .iter()
        .filter(|candidate| {
            candidate.failed() && normalized_text(&candidate.situation) == situation_key
        })
        .count();
    if episode.failed() && impasse_count >= 2 {
        gaps.push(evidence_gap(
            stable_id("impasse", &situation_key),
            GapKind::RepeatedImpass,
            format!(
                "{} immutable episodes remain impassed on {}",
                impasse_count, episode.situation
            ),
            episode,
            1.0 + impasse_count as f64,
            cost,
            source_episode.clone(),
        ));
    }

    let has_structural_material = !episode.knowledge_considered.is_empty()
        || !episode.context.relevant_knowledge.is_empty()
        || !episode.context.relevant_procedures.is_empty();
    if episode.failed() && !has_structural_material && episode.action.is_none() {
        gaps.push(evidence_gap(
            format!("episode:{}:structural-evidence", episode.id),
            GapKind::Structural,
            format!("missing structural evidence for {}", episode.situation),
            episode,
            2.0,
            cost,
            source_episode.clone(),
        ));
    }

    if episode.failed()
        && episode.prediction.is_none()
        && episode.action.is_some()
        && episode.observed_result.is_none()
        && episode.observed_facts.is_empty()
    {
        gaps.push(evidence_gap(
            format!("episode:{}:functional-evidence", episode.id),
            GapKind::Functional,
            format!("missing functional observation for {}", episode.situation),
            episode,
            2.0,
            cost,
            source_episode.clone(),
        ));
    }

    let strong_observation = episode.observed_result.is_some()
        && !episode.observed_facts.is_empty()
        && episode.evaluation.as_ref().is_some_and(|evaluation| {
            matches!(
                evaluation.tier,
                VerifiabilityTier::Hard | VerifiabilityTier::Consensus
            )
        });
    let grounding_distance = f64::from(episode.cost.rung_reached as u8)
        + if episode.teacher_interaction.is_some() {
            2.0
        } else {
            0.0
        }
        + if episode.observed_result.is_none() {
            1.0
        } else {
            0.0
        };
    if episode.succeeded() && !strong_observation && grounding_distance > 1.0 {
        gaps.push(evidence_gap(
            format!("episode:{}:grounding-distance", episode.id),
            GapKind::Ungrounded,
            format!(
                "successful result for {} remains {} steps from strong grounding",
                episode.situation, grounding_distance
            ),
            episode,
            grounding_distance,
            grounding_distance.max(1.0),
            source_episode,
        ));
    }

    gaps
}

pub(crate) fn held_contradiction_gap(contradiction: &spoon_adapt::Contradiction) -> CuriosityGap {
    let source_episode = contradiction
        .right
        .supporting_episodes
        .last()
        .or_else(|| contradiction.left.supporting_episodes.last())
        .map(ToString::to_string);
    let support = contradiction.left.supporting_episodes.len()
        + contradiction.right.supporting_episodes.len();
    CuriosityGap {
        id: format!("held-contradiction:{}", contradiction.id.0),
        kind: GapKind::Contradiction,
        statement: format!(
            "held contradiction {} for predicate {} requires scoped evidence",
            contradiction.id.0, contradiction.left.implication.predicate
        ),
        blast_radius: 2.0 + support as f64,
        goal_relevance: 0.0,
        learning_progress: (support as f64 / 8.0).min(1.0),
        cost_to_close: 4.0,
        value_score: 0.0,
        source_episode,
        resolved: false,
        created_at: contradiction.updated_at,
    }
}

fn decode_goal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Goal> {
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
}

fn decode_scheduled_action_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ScheduledLearningAction> {
    let goal_kind: String = row.get(2)?;
    let kind: String = row.get(4)?;
    Ok(ScheduledLearningAction {
        id: row.get(0)?,
        source_goal_id: row.get(1)?,
        source_goal_kind: serde_json::from_str(&goal_kind).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        source_gap_id: row.get(3)?,
        kind: serde_json::from_str(&kind).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        instruction: row.get(5)?,
        max_steps: row.get(6)?,
        value_score: row.get(7)?,
        allows_graph_mutation: row.get(8)?,
        allows_capability_mutation: row.get(9)?,
        created_at: row.get(10)?,
    })
}

/// The score is deliberately small and inspectable:
/// `(1 + blast_radius) * (goal_relevance + 0.25) *
///  (1 + learning_progress) / estimated_cost`.
///
/// The offset lets an evidence gap remain visible before a user declares an
/// active root goal, but scheduling still refuses to act without one.
fn curiosity_value_score(
    blast_radius: f64,
    goal_relevance: f64,
    learning_progress: f64,
    cost_to_close: f64,
) -> f64 {
    ((1.0 + blast_radius) * (0.25 + goal_relevance) * (1.0 + learning_progress) / cost_to_close)
        .max(0.0)
}

fn evidence_gap(
    id: String,
    kind: GapKind,
    statement: String,
    episode: &Episode,
    blast_radius: f64,
    cost_to_close: f64,
    source_episode: Option<String>,
) -> CuriosityGap {
    CuriosityGap {
        id,
        kind,
        statement,
        blast_radius,
        goal_relevance: 0.0,
        // A completed episode is one unit of evidence.  `record_gap` folds
        // this with durable prior gap history rather than trusting a score.
        learning_progress: 0.25,
        cost_to_close,
        value_score: 0.0,
        source_episode,
        resolved: false,
        created_at: episode.created_at,
    }
}

fn estimated_evidence_cost(episode: &Episode) -> f64 {
    let steps = f64::from(episode.cost.steps_taken.max(1));
    let budget = if episode.cost.budget_spent.is_finite() && episode.cost.budget_spent > 0.0 {
        episode.cost.budget_spent
    } else {
        0.0
    };
    // Rung expresses escalation cost even when a trace has no measured steps.
    (steps + budget + f64::from(episode.cost.rung_reached as u8)).max(1.0)
}

fn action_kind_for_gap(kind: GapKind) -> LearningActionKind {
    match kind {
        GapKind::FailedPrediction => LearningActionKind::ReviewPredictionEvidence,
        GapKind::RepeatedImpass => LearningActionKind::InspectRepeatedImpass,
        GapKind::Contradiction => LearningActionKind::ResolveHeldContradiction,
        GapKind::Structural => LearningActionKind::GatherStructuralEvidence,
        GapKind::Functional => LearningActionKind::GatherFunctionalEvidence,
        GapKind::Ungrounded => LearningActionKind::GroundObservation,
    }
}

fn learning_instruction(goal: &Goal, gap: &CuriosityGap, kind: LearningActionKind) -> String {
    let action = match kind {
        LearningActionKind::ReviewPredictionEvidence => {
            "review the prediction and its failure evidence"
        }
        LearningActionKind::InspectRepeatedImpass => {
            "compare the repeated immutable impasse episodes"
        }
        LearningActionKind::ResolveHeldContradiction => {
            "collect a discriminating observation for the held contradiction"
        }
        LearningActionKind::GatherStructuralEvidence => {
            "identify missing concepts, relationships, or procedures"
        }
        LearningActionKind::GatherFunctionalEvidence => "obtain one bounded functional observation",
        LearningActionKind::GroundObservation => "independently ground the provisional result",
    };
    format!(
        "For {:?} goal {:?}, {}: {}",
        goal.kind, goal.statement, action, gap.statement
    )
}

fn estimated_steps(cost: f64) -> u32 {
    let raw = if cost.is_finite() { cost.ceil() } else { 1.0 };
    raw.clamp(1.0, f64::from(MAX_LEARNING_ACTION_STEPS)) as u32
}

fn stable_id(prefix: &str, material: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update([0]);
    hasher.update(material.as_bytes());
    format!("{prefix}:{:x}", hasher.finalize())
}

fn normalized_text(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn text_relevance(left: &str, right: &str) -> f64 {
    let left = text_terms(left);
    let right = text_terms(right);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let overlap = left.intersection(&right).count() as f64;
    overlap / left.len().min(right.len()) as f64
}

fn text_terms(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .map(str::trim)
        .filter(|term| term.chars().count() >= 3)
        .map(|term| term.to_ascii_lowercase())
        .collect()
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
    use spoon_core::{Episode, EscalationRung, Evaluation, Value, VerifiabilityTier};

    use super::{
        CuriosityGap, GapKind, GoalKind, GoalStore, LearningActionKind,
        derive_episode_curiosity_gaps,
    };

    fn failed_episode(situation: &str, predicted: bool) -> Episode {
        let mut episode = Episode::new(situation);
        episode.prediction = predicted.then_some(Value::Int(42));
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: false,
            details: "immutable failure".into(),
            surprise: Some(1.0),
        });
        episode.cost.steps_taken = 2;
        episode
    }

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

    #[test]
    fn episode_evidence_derives_distinct_gap_kinds() {
        let first = failed_episode("missing temperature conversion", true);
        let second = failed_episode("missing temperature conversion", true);
        let repeated = derive_episode_curiosity_gaps(&second, &[second.clone(), first]);
        assert!(
            repeated
                .iter()
                .any(|gap| gap.kind == GapKind::FailedPrediction)
        );
        assert!(
            repeated
                .iter()
                .any(|gap| gap.kind == GapKind::RepeatedImpass)
        );

        let structural = failed_episode("unknown domain", false);
        let structural = derive_episode_curiosity_gaps(&structural, &[]);
        assert!(structural.iter().any(|gap| gap.kind == GapKind::Structural));

        let mut functional = failed_episode("run an unavailable thing", false);
        functional.action = Some("procedure:missing@1".into());
        let functional = derive_episode_curiosity_gaps(&functional, &[]);
        assert!(functional.iter().any(|gap| gap.kind == GapKind::Functional));

        let mut ungrounded = Episode::new("provisional answer");
        ungrounded.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Deferred,
            success: true,
            details: "only a teacher report".into(),
            surprise: None,
        });
        ungrounded.cost.rung_reached = EscalationRung::Ask;
        ungrounded.teacher_interaction = Some(serde_json::json!({"request": "ask"}));
        let ungrounded = derive_episode_curiosity_gaps(&ungrounded, &[]);
        assert!(ungrounded.iter().any(|gap| gap.kind == GapKind::Ungrounded));
    }

    #[test]
    fn value_is_recomputed_and_scheduling_is_bounded_and_idempotent() {
        let store = GoalStore::in_memory().unwrap();
        let standing = store
            .create_goal(GoalKind::Standing, "keep predictions accurate", None)
            .unwrap();
        let duplicate = store
            .create_goal(GoalKind::Standing, "keep predictions accurate", None)
            .unwrap();
        assert_eq!(standing.id, duplicate.id);

        let gap = CuriosityGap {
            id: "prediction-evidence".into(),
            kind: GapKind::FailedPrediction,
            statement: "prediction evidence is missing".into(),
            blast_radius: 3.0,
            // These two values are deliberately untrusted caller input.
            goal_relevance: 0.0,
            learning_progress: 0.0,
            cost_to_close: 2.0,
            value_score: 9_999_999.0,
            source_episode: Some("episode-immutable".into()),
            resolved: false,
            created_at: 1,
        };
        store.record_gap(&gap).unwrap();
        let stored = store.rank_gaps(1).unwrap().remove(0);
        assert!(stored.value_score < 100.0);
        assert!(stored.goal_relevance > 0.0);

        let first = store.schedule_next_learning_action().unwrap().unwrap();
        let second = store.schedule_next_learning_action().unwrap().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.source_goal_id, standing.id);
        assert_eq!(first.kind, LearningActionKind::ReviewPredictionEvidence);
        assert!(first.max_steps <= 32);
        assert!(!first.allows_graph_mutation);
        assert!(!first.allows_capability_mutation);
        assert_eq!(
            store.list_scheduled_learning_actions(8).unwrap(),
            vec![first]
        );
    }
}
