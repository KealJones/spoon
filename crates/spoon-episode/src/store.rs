use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use spoon_core::{
    Episode, EpisodeId, EscalationRung, ProcedureId, SpoonError, TestCase, VerifiabilityTier,
    concept::ConceptId,
};
use spoon_exec::ExecTrace;
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use spoon_core::{Value, episode::Evaluation};

type CreditProvenance = (Vec<EpisodeId>, Vec<Uuid>, Vec<Uuid>, Vec<EpisodeId>);

/// A version-pinned executable element used by the materialized credit index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CreditElementRef {
    pub procedure: ProcedureId,
    pub version: u32,
}

impl PartialOrd for CreditElementRef {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CreditElementRef {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.procedure.0, self.version).cmp(&(other.procedure.0, other.version))
    }
}

/// Exact sufficient statistics for one procedure version. These rows are
/// maintained when evidence is written so failure analysis never needs to
/// rescan historical traces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditElementAggregate {
    pub element: CreditElementRef,
    pub exposures: u32,
    pub failures: u32,
    pub weighted_exposure: f64,
    pub weighted_failures: f64,
    /// Exact number of contributing episodes. `episode_ids` is populated only
    /// by the explicit provenance snapshot; bounded analysis snapshots retain
    /// this count without materializing the history.
    pub provenance_count: u32,
    pub episode_ids: Vec<EpisodeId>,
    pub feedback_rows_scanned: u64,
    pub feedback_rows_used: u64,
    pub conflicts_excluded: u64,
    pub feedback_ids_scanned: Vec<Uuid>,
    pub feedback_ids_used: Vec<Uuid>,
    pub conflicting_episode_ids: Vec<EpisodeId>,
    pub revision: u64,
}

/// Materialized pairwise co-occurrence for correlation-aware ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditPairAggregate {
    pub left: CreditElementRef,
    pub right: CreditElementRef,
    pub together: u32,
    pub revision: u64,
}

/// Bounded index snapshot for exactly the elements present in a failed trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditAggregateSnapshot {
    pub elements: Vec<CreditElementAggregate>,
    pub pairs: Vec<CreditPairAggregate>,
    pub source_contribution: CreditEpisodeContribution,
    /// Transactional credit-index work already performed for the source
    /// episode. This is charged once by first analysis instead of disappearing
    /// behind materialization.
    pub source_index_maintenance_work_units: u64,
    /// Whether history-sized provenance identifiers were explicitly requested.
    pub provenance_materialized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CreditEpisodeContribution {
    pub included: bool,
    pub failed: bool,
    pub weight: f64,
}

#[derive(Debug, Clone)]
struct CreditEpisodeState {
    included: bool,
    failed: bool,
    weight: f64,
    feedback_rows_scanned: u64,
    feedback_rows_used: u64,
    conflict: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct FeedbackCreditSummary {
    total: u64,
    successes: u64,
    failures: u64,
    hard: u64,
    consensus: u64,
    deferred: u64,
}

/// Identifies where late evidence about an episode came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackSource {
    pub kind: String,
    pub actor: Option<String>,
}

impl FeedbackSource {
    pub fn new(kind: impl Into<String>, actor: Option<String>) -> Self {
        Self {
            kind: kind.into(),
            actor,
        }
    }
}

/// Append-only evidence received after an episode has completed.
///
/// Feedback is deliberately stored separately from [`Episode`]. Conflicting
/// reports remain available for attribution and the original episode remains
/// an immutable record of what the system knew at execution time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeFeedback {
    pub id: Uuid,
    pub episode_id: EpisodeId,
    pub observed_result: Value,
    pub evaluation: Evaluation,
    pub source: FeedbackSource,
    pub idempotency_key: String,
    pub created_at: i64,
}

impl PartialEq for EpisodeFeedback {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.episode_id == other.episode_id
            && self.observed_result == other.observed_result
            && self.evaluation.tier == other.evaluation.tier
            && self.evaluation.success == other.evaluation.success
            && self.evaluation.details == other.evaluation.details
            && self.evaluation.surprise == other.evaluation.surprise
            && self.source == other.source
            && self.idempotency_key == other.idempotency_key
            && self.created_at == other.created_at
    }
}

impl EpisodeFeedback {
    pub fn new(
        episode_id: EpisodeId,
        observed_result: Value,
        evaluation: Evaluation,
        source: FeedbackSource,
        idempotency_key: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            episode_id,
            observed_result,
            evaluation,
            source,
            idempotency_key: idempotency_key.into(),
            created_at: now_unix(),
        }
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeQuery {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub outcome: Option<bool>,
    pub rung: Option<EscalationRung>,
    pub concept: Option<ConceptId>,
    pub limit: u32,
}

/// A verified, version-pinned regression case. These rows are explicit
/// metadata: merely storing an episode never grants promotion authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedRegressionCase {
    pub episode_id: EpisodeId,
    pub procedure_id: ProcedureId,
    pub procedure_version: u32,
    pub test_case: TestCase,
}

/// Counts finalized episode outcomes with an explicit durable teacher request.
/// A request is used instead of merely checking for arbitrary interaction JSON,
/// because other engine features may retain non-teacher provenance there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeacherInteractionMetrics {
    pub teacher_interaction_episodes: u64,
    pub teacher_assisted_successes: u64,
    pub teacher_free_successes: u64,
}

impl Default for EpisodeQuery {
    fn default() -> Self {
        Self {
            since: None,
            until: None,
            outcome: None,
            rung: None,
            concept: None,
            limit: 100,
        }
    }
}

pub struct EpisodeStore {
    conn: Connection,
}

impl EpisodeStore {
    pub fn new(path: &str) -> Result<Self, SpoonError> {
        let conn = Connection::open(path).map_err(|e| SpoonError::Storage(e.to_string()))?;
        let store = Self { conn };
        store.create_schema()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, SpoonError> {
        let conn = Connection::open_in_memory().map_err(|e| SpoonError::Storage(e.to_string()))?;
        let store = Self { conn };
        store.create_schema()?;
        Ok(store)
    }

    fn create_schema(&self) -> Result<(), SpoonError> {
        self.conn
            .execute_batch(
                "PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS episodes (
                    id TEXT PRIMARY KEY,
                    situation TEXT NOT NULL,
                    data_json TEXT NOT NULL,
                    success INTEGER,
                    rung_reached TEXT,
                    finalized INTEGER NOT NULL DEFAULT 1,
                    created_at INTEGER NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_episodes_created
                    ON episodes(created_at);

                CREATE INDEX IF NOT EXISTS idx_episodes_success
                    ON episodes(success);

                CREATE INDEX IF NOT EXISTS idx_episodes_rung
                    ON episodes(rung_reached);

                CREATE TABLE IF NOT EXISTS verified_regression_cases (
                    episode_id TEXT NOT NULL,
                    procedure_id TEXT NOT NULL,
                    procedure_version INTEGER NOT NULL,
                    test_case_json TEXT NOT NULL,
                    PRIMARY KEY (episode_id, procedure_id, procedure_version),
                    FOREIGN KEY (episode_id) REFERENCES episodes(id)
                );

                CREATE INDEX IF NOT EXISTS idx_verified_regressions_procedure
                    ON verified_regression_cases(procedure_id, procedure_version);

                CREATE TABLE IF NOT EXISTS episode_concepts (
                    episode_id TEXT NOT NULL,
                    concept_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    PRIMARY KEY (episode_id, concept_id, role),
                    FOREIGN KEY (episode_id) REFERENCES episodes(id)
                );

                CREATE INDEX IF NOT EXISTS idx_episode_concepts_concept
                    ON episode_concepts(concept_id);

                CREATE TABLE IF NOT EXISTS episode_observed_facts (
                    episode_id TEXT NOT NULL,
                    fact_index INTEGER NOT NULL,
                    predicate TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    scope_json TEXT NOT NULL,
                    PRIMARY KEY (episode_id, fact_index),
                    FOREIGN KEY (episode_id) REFERENCES episodes(id)
                );

                CREATE INDEX IF NOT EXISTS idx_episode_observed_facts_predicate
                    ON episode_observed_facts(predicate, episode_id);

                CREATE TABLE IF NOT EXISTS episode_feedback (
                    id TEXT PRIMARY KEY,
                    episode_id TEXT NOT NULL,
                    observed_result_json TEXT NOT NULL,
                    evaluation_json TEXT NOT NULL,
                    source_json TEXT NOT NULL,
                    idempotency_key TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    UNIQUE (episode_id, idempotency_key),
                    FOREIGN KEY (episode_id) REFERENCES episodes(id)
                );

                CREATE INDEX IF NOT EXISTS idx_episode_feedback_episode_created
                    ON episode_feedback(episode_id, created_at, id);

                CREATE TABLE IF NOT EXISTS episode_feedback_credit_summary (
                    episode_id TEXT PRIMARY KEY,
                    total INTEGER NOT NULL,
                    successes INTEGER NOT NULL,
                    failures INTEGER NOT NULL,
                    hard INTEGER NOT NULL,
                    consensus INTEGER NOT NULL,
                    deferred INTEGER NOT NULL,
                    FOREIGN KEY (episode_id) REFERENCES episodes(id)
                );

                CREATE TABLE IF NOT EXISTS episode_credit_elements (
                    episode_id TEXT NOT NULL,
                    procedure_id TEXT NOT NULL,
                    procedure_version INTEGER NOT NULL,
                    included INTEGER NOT NULL,
                    failed INTEGER NOT NULL,
                    weight REAL NOT NULL,
                    feedback_rows_scanned INTEGER NOT NULL,
                    feedback_rows_used INTEGER NOT NULL,
                    conflict INTEGER NOT NULL,
                    evidence_revision INTEGER NOT NULL,
                    maintenance_work_units INTEGER NOT NULL,
                    PRIMARY KEY (episode_id, procedure_id, procedure_version),
                    FOREIGN KEY (episode_id) REFERENCES episodes(id)
                );

                CREATE INDEX IF NOT EXISTS idx_episode_credit_elements_version
                    ON episode_credit_elements(procedure_id, procedure_version, episode_id);

                CREATE TABLE IF NOT EXISTS credit_element_aggregates (
                    procedure_id TEXT NOT NULL,
                    procedure_version INTEGER NOT NULL,
                    exposures INTEGER NOT NULL,
                    failures INTEGER NOT NULL,
                    weighted_exposure REAL NOT NULL,
                    weighted_failures REAL NOT NULL,
                    provenance_count INTEGER NOT NULL,
                    feedback_rows_scanned INTEGER NOT NULL,
                    feedback_rows_used INTEGER NOT NULL,
                    conflicts_excluded INTEGER NOT NULL,
                    revision INTEGER NOT NULL,
                    PRIMARY KEY (procedure_id, procedure_version)
                );

                CREATE TABLE IF NOT EXISTS credit_pair_aggregates (
                    left_procedure_id TEXT NOT NULL,
                    left_version INTEGER NOT NULL,
                    right_procedure_id TEXT NOT NULL,
                    right_version INTEGER NOT NULL,
                    together INTEGER NOT NULL,
                    revision INTEGER NOT NULL,
                    PRIMARY KEY (
                        left_procedure_id, left_version,
                        right_procedure_id, right_version
                    )
                );",
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        if !self.has_episode_column("finalized")? {
            self.conn
                .execute(
                    "ALTER TABLE episodes ADD COLUMN finalized INTEGER NOT NULL DEFAULT 1",
                    [],
                )
                .map_err(|e| SpoonError::Storage(e.to_string()))?;
        }
        self.ensure_credit_index_v2()?;
        self.backfill_feedback_credit_summary()?;
        self.backfill_credit_index()?;
        self.backfill_observed_fact_index()?;
        Ok(())
    }

    fn has_episode_column(&self, name: &str) -> Result<bool, SpoonError> {
        let mut statement = self
            .conn
            .prepare("PRAGMA table_info(episodes)")
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        for column in names {
            if column.map_err(|e| SpoonError::Storage(e.to_string()))? == name {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn has_table_column(&self, table: &str, name: &str) -> Result<bool, SpoonError> {
        let mut statement = self
            .conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(storage)?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(storage)?;
        for column in names {
            if column.map_err(storage)? == name {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Migrates the first materialized index, whose aggregate rows contained
    /// history-sized JSON sets, to scalar sufficient statistics plus normalized
    /// per-episode contribution rows. Rebuilding is exact and happens once at
    /// database open; ordinary analysis never pays or conceals this migration.
    fn ensure_credit_index_v2(&self) -> Result<(), SpoonError> {
        if self.has_table_column("episode_credit_elements", "included")?
            && self.has_table_column("credit_element_aggregates", "provenance_count")?
        {
            return Ok(());
        }
        self.conn
            .execute_batch(
                "DROP TABLE IF EXISTS episode_credit_state;
                 DROP TABLE IF EXISTS episode_credit_elements;
                 DROP TABLE IF EXISTS credit_element_aggregates;
                 DROP TABLE IF EXISTS credit_pair_aggregates;

                 CREATE TABLE episode_credit_elements (
                    episode_id TEXT NOT NULL,
                    procedure_id TEXT NOT NULL,
                    procedure_version INTEGER NOT NULL,
                    included INTEGER NOT NULL,
                    failed INTEGER NOT NULL,
                    weight REAL NOT NULL,
                    feedback_rows_scanned INTEGER NOT NULL,
                    feedback_rows_used INTEGER NOT NULL,
                    conflict INTEGER NOT NULL,
                    evidence_revision INTEGER NOT NULL,
                    maintenance_work_units INTEGER NOT NULL,
                    PRIMARY KEY (episode_id, procedure_id, procedure_version),
                    FOREIGN KEY (episode_id) REFERENCES episodes(id)
                 );
                 CREATE INDEX idx_episode_credit_elements_version
                    ON episode_credit_elements(procedure_id, procedure_version, episode_id);

                 CREATE TABLE credit_element_aggregates (
                    procedure_id TEXT NOT NULL,
                    procedure_version INTEGER NOT NULL,
                    exposures INTEGER NOT NULL,
                    failures INTEGER NOT NULL,
                    weighted_exposure REAL NOT NULL,
                    weighted_failures REAL NOT NULL,
                    provenance_count INTEGER NOT NULL,
                    feedback_rows_scanned INTEGER NOT NULL,
                    feedback_rows_used INTEGER NOT NULL,
                    conflicts_excluded INTEGER NOT NULL,
                    revision INTEGER NOT NULL,
                    PRIMARY KEY (procedure_id, procedure_version)
                 );
                 CREATE TABLE credit_pair_aggregates (
                    left_procedure_id TEXT NOT NULL,
                    left_version INTEGER NOT NULL,
                    right_procedure_id TEXT NOT NULL,
                    right_version INTEGER NOT NULL,
                    together INTEGER NOT NULL,
                    revision INTEGER NOT NULL,
                    PRIMARY KEY (
                        left_procedure_id, left_version,
                        right_procedure_id, right_version
                    )
                 );",
            )
            .map_err(storage)
    }

    fn backfill_credit_index(&self) -> Result<(), SpoonError> {
        let pending = {
            let mut statement = self
                .conn
                .prepare(
                    "SELECT e.data_json FROM episodes AS e
                     WHERE e.finalized = 1 AND NOT EXISTS (
                        SELECT 1 FROM episode_credit_elements AS element
                        WHERE element.episode_id = e.id
                     )
                     ORDER BY e.created_at, e.id",
                )
                .map_err(storage)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage)?
                .map(|row| {
                    let json = row.map_err(storage)?;
                    serde_json::from_str::<Episode>(&json).map_err(serialization)
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        if pending.is_empty() {
            return Ok(());
        }
        let transaction = self.conn.unchecked_transaction().map_err(storage)?;
        for episode in pending {
            Self::index_new_episode(&transaction, &episode)?;
        }
        transaction.commit().map_err(storage)
    }

    fn index_new_episode(conn: &Connection, episode: &Episode) -> Result<(), SpoonError> {
        let elements = credit_elements(episode);
        if elements.is_empty() {
            return Ok(());
        }
        let feedback = Self::feedback_credit_summary(conn, episode.id)?;
        let state = effective_credit_state(episode, feedback, true);
        let maintenance_work_units =
            index_maintenance_work_units(elements.len(), feedback.total > 0);
        for element in &elements {
            conn.execute(
                "INSERT INTO episode_credit_elements
                    (episode_id, procedure_id, procedure_version, included, failed,
                     weight, feedback_rows_scanned, feedback_rows_used, conflict,
                     evidence_revision, maintenance_work_units)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10)",
                params![
                    episode.id.to_string(),
                    element.procedure.to_string(),
                    element.version,
                    state.included,
                    state.failed,
                    state.weight,
                    state.feedback_rows_scanned,
                    state.feedback_rows_used,
                    state.conflict,
                    maintenance_work_units,
                ],
            )
            .map_err(storage)?;
        }
        Self::adjust_credit_aggregates(conn, episode.id, &elements, None, &state)?;
        Ok(())
    }

    fn refresh_credit_episode(conn: &Connection, episode_id: EpisodeId) -> Result<(), SpoonError> {
        let indexed = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM episode_credit_elements WHERE episode_id = ?1
                 )",
                params![episode_id.to_string()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage)?;
        if !indexed {
            return Ok(());
        }
        let episode_json: String = conn
            .query_row(
                "SELECT data_json FROM episodes WHERE id = ?1 AND finalized = 1",
                params![episode_id.to_string()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let episode: Episode = serde_json::from_str(&episode_json).map_err(serialization)?;
        let (elements, old, prior_maintenance) = Self::credit_elements_and_state(conn, episode_id)?;
        let feedback = Self::feedback_credit_summary(conn, episode_id)?;
        let new = effective_credit_state(&episode, feedback, !elements.is_empty());
        Self::adjust_credit_aggregates(conn, episode_id, &elements, Some(&old), &new)?;
        let maintenance_work_units =
            prior_maintenance.saturating_add(index_maintenance_work_units(elements.len(), true));
        conn.execute(
            "UPDATE episode_credit_elements
             SET included = ?2, failed = ?3, weight = ?4,
                 feedback_rows_scanned = ?5, feedback_rows_used = ?6,
                 conflict = ?7, evidence_revision = evidence_revision + 1,
                 maintenance_work_units = ?8
             WHERE episode_id = ?1",
            params![
                episode_id.to_string(),
                new.included,
                new.failed,
                new.weight,
                new.feedback_rows_scanned,
                new.feedback_rows_used,
                new.conflict,
                maintenance_work_units,
            ],
        )
        .map_err(storage)?;
        Ok(())
    }

    fn credit_elements_and_state(
        conn: &Connection,
        episode_id: EpisodeId,
    ) -> Result<(Vec<CreditElementRef>, CreditEpisodeState, u64), SpoonError> {
        let mut statement = conn
            .prepare(
                "SELECT procedure_id, procedure_version, included, failed, weight,
                        feedback_rows_scanned, feedback_rows_used, conflict,
                        maintenance_work_units
                 FROM episode_credit_elements
                 WHERE episode_id = ?1
                 ORDER BY procedure_id, procedure_version",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map(params![episode_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, u64>(6)?,
                    row.get::<_, bool>(7)?,
                    row.get::<_, u64>(8)?,
                ))
            })
            .map_err(storage)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage)?;
        let first = rows.first().ok_or_else(|| {
            SpoonError::NotFound(format!("credit index for episode {episode_id}"))
        })?;
        let state = CreditEpisodeState {
            included: first.2,
            failed: first.3,
            weight: first.4,
            feedback_rows_scanned: first.5,
            feedback_rows_used: first.6,
            conflict: first.7,
        };
        let maintenance = first.8;
        let elements = rows
            .into_iter()
            .map(|row| {
                Ok(CreditElementRef {
                    procedure: ProcedureId(parse_uuid(&row.0)?),
                    version: row.1,
                })
            })
            .collect::<Result<Vec<_>, SpoonError>>()?;
        Ok((elements, state, maintenance))
    }

    fn adjust_credit_aggregates(
        conn: &Connection,
        _episode_id: EpisodeId,
        elements: &[CreditElementRef],
        old: Option<&CreditEpisodeState>,
        new: &CreditEpisodeState,
    ) -> Result<(), SpoonError> {
        for element in elements {
            Self::adjust_element_aggregate(conn, *element, old, new)?;
        }
        for left_index in 0..elements.len() {
            for right in &elements[(left_index + 1)..] {
                Self::adjust_pair_aggregate(
                    conn,
                    elements[left_index],
                    *right,
                    old.is_some_and(|state| state.included),
                    new.included,
                )?;
            }
        }
        Ok(())
    }

    fn backfill_observed_fact_index(&self) -> Result<(), SpoonError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT data_json FROM episodes
                 WHERE NOT EXISTS (
                    SELECT 1 FROM episode_observed_facts facts
                    WHERE facts.episode_id = episodes.id
                 )",
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| SpoonError::Storage(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        drop(statement);
        for json in rows {
            let episode: Episode = serde_json::from_str(&json)
                .map_err(|e| SpoonError::Serialization(e.to_string()))?;
            Self::insert_observed_fact_index(&self.conn, &episode)?;
        }
        Ok(())
    }

    fn adjust_element_aggregate(
        conn: &Connection,
        element: CreditElementRef,
        old: Option<&CreditEpisodeState>,
        new: &CreditEpisodeState,
    ) -> Result<(), SpoonError> {
        let stored = conn
            .query_row(
                "SELECT exposures, failures, weighted_exposure, weighted_failures,
                        provenance_count, feedback_rows_scanned,
                        feedback_rows_used, conflicts_excluded, revision
                 FROM credit_element_aggregates
                 WHERE procedure_id = ?1 AND procedure_version = ?2",
                params![element.procedure.to_string(), element.version],
                |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, u32>(4)?,
                        row.get::<_, u64>(5)?,
                        row.get::<_, u64>(6)?,
                        row.get::<_, u64>(7)?,
                        row.get::<_, u64>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(storage)?;
        let (
            mut exposures,
            mut failures,
            mut weighted_exposure,
            mut weighted_failures,
            mut provenance_count,
            mut feedback_rows_scanned,
            mut feedback_rows_used,
            mut conflicts_excluded,
            revision,
        ) = stored.unwrap_or((0, 0, 0.0, 0.0, 0, 0, 0, 0, 0));
        if let Some(old) = old {
            if old.included {
                exposures = exposures.saturating_sub(1);
                failures = failures.saturating_sub(u32::from(old.failed));
                weighted_exposure -= old.weight;
                weighted_failures -= if old.failed { old.weight } else { 0.0 };
                provenance_count = provenance_count.saturating_sub(1);
            }
            feedback_rows_scanned = feedback_rows_scanned.saturating_sub(old.feedback_rows_scanned);
            feedback_rows_used = feedback_rows_used.saturating_sub(old.feedback_rows_used);
            if old.conflict {
                conflicts_excluded = conflicts_excluded.saturating_sub(1);
            }
        }
        if new.included {
            exposures = exposures.saturating_add(1);
            failures = failures.saturating_add(u32::from(new.failed));
            weighted_exposure += new.weight;
            weighted_failures += if new.failed { new.weight } else { 0.0 };
            provenance_count = provenance_count.saturating_add(1);
        }
        feedback_rows_scanned = feedback_rows_scanned.saturating_add(new.feedback_rows_scanned);
        feedback_rows_used = feedback_rows_used.saturating_add(new.feedback_rows_used);
        if new.conflict {
            conflicts_excluded = conflicts_excluded.saturating_add(1);
        }
        conn.execute(
            "INSERT INTO credit_element_aggregates
                (procedure_id, procedure_version, exposures, failures,
                 weighted_exposure, weighted_failures, provenance_count,
                 feedback_rows_scanned, feedback_rows_used,
                 conflicts_excluded, revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(procedure_id, procedure_version) DO UPDATE SET
                exposures = excluded.exposures,
                failures = excluded.failures,
                weighted_exposure = excluded.weighted_exposure,
                weighted_failures = excluded.weighted_failures,
                provenance_count = excluded.provenance_count,
                feedback_rows_scanned = excluded.feedback_rows_scanned,
                feedback_rows_used = excluded.feedback_rows_used,
                conflicts_excluded = excluded.conflicts_excluded,
                revision = excluded.revision",
            params![
                element.procedure.to_string(),
                element.version,
                exposures,
                failures,
                weighted_exposure.max(0.0),
                weighted_failures.max(0.0),
                provenance_count,
                feedback_rows_scanned,
                feedback_rows_used,
                conflicts_excluded,
                revision.saturating_add(1),
            ],
        )
        .map_err(storage)?;
        Ok(())
    }

    fn adjust_pair_aggregate(
        conn: &Connection,
        left: CreditElementRef,
        right: CreditElementRef,
        old_included: bool,
        new_included: bool,
    ) -> Result<(), SpoonError> {
        let (left, right) = ordered_elements(left, right);
        let stored = conn
            .query_row(
                "SELECT together, revision FROM credit_pair_aggregates
                 WHERE left_procedure_id = ?1 AND left_version = ?2
                   AND right_procedure_id = ?3 AND right_version = ?4",
                params![
                    left.procedure.to_string(),
                    left.version,
                    right.procedure.to_string(),
                    right.version,
                ],
                |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u64>(1)?)),
            )
            .optional()
            .map_err(storage)?
            .unwrap_or((0, 0));
        let together = stored
            .0
            .saturating_sub(u32::from(old_included))
            .saturating_add(u32::from(new_included));
        conn.execute(
            "INSERT INTO credit_pair_aggregates
                (left_procedure_id, left_version, right_procedure_id, right_version,
                 together, revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(left_procedure_id, left_version, right_procedure_id, right_version)
             DO UPDATE SET together = excluded.together, revision = excluded.revision",
            params![
                left.procedure.to_string(),
                left.version,
                right.procedure.to_string(),
                right.version,
                together,
                stored.1.saturating_add(1),
            ],
        )
        .map_err(storage)?;
        Ok(())
    }

    fn feedback_for_conn(
        conn: &Connection,
        episode_id: EpisodeId,
    ) -> Result<Vec<EpisodeFeedback>, SpoonError> {
        let mut statement = conn
            .prepare(
                "SELECT id, episode_id, observed_result_json, evaluation_json, source_json,
                        idempotency_key, created_at
                 FROM episode_feedback
                 WHERE episode_id = ?1
                 ORDER BY created_at ASC, rowid ASC",
            )
            .map_err(storage)?;
        statement
            .query_map(params![episode_id.to_string()], Self::feedback_from_row)
            .map_err(storage)?
            .map(|row| row.map_err(storage)?)
            .collect()
    }

    fn feedback_credit_summary(
        conn: &Connection,
        episode_id: EpisodeId,
    ) -> Result<FeedbackCreditSummary, SpoonError> {
        conn.query_row(
            "SELECT total, successes, failures, hard, consensus, deferred
             FROM episode_feedback_credit_summary WHERE episode_id = ?1",
            params![episode_id.to_string()],
            |row| {
                Ok(FeedbackCreditSummary {
                    total: row.get(0)?,
                    successes: row.get(1)?,
                    failures: row.get(2)?,
                    hard: row.get(3)?,
                    consensus: row.get(4)?,
                    deferred: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(storage)
        .map(|summary| summary.unwrap_or_default())
    }

    fn increment_feedback_credit_summary(
        conn: &Connection,
        feedback: &EpisodeFeedback,
    ) -> Result<(), SpoonError> {
        let (hard, consensus, deferred) = match feedback.evaluation.tier {
            VerifiabilityTier::Hard => (1_u64, 0_u64, 0_u64),
            VerifiabilityTier::Consensus => (0_u64, 1_u64, 0_u64),
            VerifiabilityTier::Deferred => (0_u64, 0_u64, 1_u64),
        };
        conn.execute(
            "INSERT INTO episode_feedback_credit_summary
                (episode_id, total, successes, failures, hard, consensus, deferred)
             VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(episode_id) DO UPDATE SET
                total = total + 1,
                successes = successes + excluded.successes,
                failures = failures + excluded.failures,
                hard = hard + excluded.hard,
                consensus = consensus + excluded.consensus,
                deferred = deferred + excluded.deferred",
            params![
                feedback.episode_id.to_string(),
                u64::from(feedback.evaluation.success),
                u64::from(!feedback.evaluation.success),
                hard,
                consensus,
                deferred,
            ],
        )
        .map_err(storage)?;
        Ok(())
    }

    fn backfill_feedback_credit_summary(&self) -> Result<(), SpoonError> {
        let episode_ids = {
            let mut statement = self
                .conn
                .prepare(
                    "SELECT DISTINCT feedback.episode_id
                     FROM episode_feedback AS feedback
                     WHERE NOT EXISTS (
                        SELECT 1 FROM episode_feedback_credit_summary AS summary
                        WHERE summary.episode_id = feedback.episode_id
                     )",
                )
                .map_err(storage)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(storage)?
        };
        if episode_ids.is_empty() {
            return Ok(());
        }
        let transaction = self.conn.unchecked_transaction().map_err(storage)?;
        for raw_id in episode_ids {
            let episode_id = EpisodeId(parse_uuid(&raw_id)?);
            for feedback in Self::feedback_for_conn(&transaction, episode_id)? {
                Self::increment_feedback_credit_summary(&transaction, &feedback)?;
            }
        }
        transaction.commit().map_err(storage)
    }

    /// Inserts a completed episode. Completed episodes are immutable.
    pub fn insert(&self, episode: &Episode) -> Result<(), SpoonError> {
        self.insert_with_finalization(episode, true)
    }

    /// Inserts an explicitly incomplete episode which may be finalized once.
    pub fn insert_draft(&self, episode: &Episode) -> Result<(), SpoonError> {
        if episode.evaluation.is_some()
            || episode.observed_result.is_some()
            || !episode.observed_facts.is_empty()
        {
            return Err(SpoonError::Other(
                "draft episodes cannot contain evaluation evidence, an observed result, or observed facts"
                    .into(),
            ));
        }
        self.insert_with_finalization(episode, false)
    }

    fn insert_with_finalization(
        &self,
        episode: &Episode,
        finalized: bool,
    ) -> Result<(), SpoonError> {
        let data_json =
            serde_json::to_string(episode).map_err(|e| SpoonError::Serialization(e.to_string()))?;

        let success = episode.evaluation.as_ref().map(|e| e.success as i32);
        let rung = serde_json::to_value(episode.cost.rung_reached)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        let transaction = self
            .conn
            .unchecked_transaction()
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        transaction
            .execute(
                "INSERT INTO episodes
                    (id, situation, data_json, success, rung_reached, finalized, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    episode.id.to_string(),
                    episode.situation,
                    data_json,
                    success,
                    rung,
                    finalized,
                    episode.created_at,
                ],
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;

        Self::insert_concept_index(&transaction, episode)?;
        Self::insert_observed_fact_index(&transaction, episode)?;
        if finalized {
            Self::index_new_episode(&transaction, episode)?;
        }
        transaction
            .commit()
            .map_err(|e| SpoonError::Storage(e.to_string()))
    }

    fn insert_concept_index(conn: &Connection, episode: &Episode) -> Result<(), SpoonError> {
        for interp in &episode.interpretations {
            conn.execute(
                "INSERT OR IGNORE INTO episode_concepts (episode_id, concept_id, role)
                     VALUES (?1, ?2, 'interpretation')",
                params![episode.id.to_string(), interp.meaning.to_string()],
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        }

        for entity in &episode.context.entities {
            conn.execute(
                "INSERT OR IGNORE INTO episode_concepts (episode_id, concept_id, role)
                     VALUES (?1, ?2, 'context')",
                params![episode.id.to_string(), entity.to_string()],
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        }

        for candidate in &episode.knowledge_considered {
            conn.execute(
                "INSERT OR IGNORE INTO episode_concepts (episode_id, concept_id, role)
                     VALUES (?1, ?2, 'considered')",
                params![episode.id.to_string(), candidate.concept.to_string()],
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        }

        Ok(())
    }

    fn insert_observed_fact_index(conn: &Connection, episode: &Episode) -> Result<(), SpoonError> {
        for (index, fact) in episode.observed_facts.iter().enumerate() {
            conn.execute(
                "INSERT OR IGNORE INTO episode_observed_facts
                    (episode_id, fact_index, predicate, value_json, scope_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    episode.id.to_string(),
                    index as i64,
                    fact.predicate,
                    serde_json::to_string(&fact.value)
                        .map_err(|e| SpoonError::Serialization(e.to_string()))?,
                    serde_json::to_string(&fact.scope)
                        .map_err(|e| SpoonError::Serialization(e.to_string()))?,
                ],
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    pub fn get(&self, id: EpisodeId) -> Result<Episode, SpoonError> {
        self.get_with_finalization(id, true)
    }

    /// Records a regression case only for a successful Hard/Consensus episode.
    /// Deferred evidence is intentionally excluded from the promotion suite.
    pub fn record_verified_regression_case(
        &self,
        case: &VerifiedRegressionCase,
    ) -> Result<(), SpoonError> {
        if !matches!(
            case.test_case.tier,
            VerifiabilityTier::Hard | VerifiabilityTier::Consensus
        ) {
            return Err(SpoonError::Other(
                "regression cases require Hard or Consensus evidence".into(),
            ));
        }
        let episode = self.get(case.episode_id)?;
        if episode
            .evaluation
            .as_ref()
            .is_none_or(|evaluation| !evaluation.success)
        {
            return Err(SpoonError::Other(
                "regression cases require a successful episode".into(),
            ));
        }
        let json = serde_json::to_string(&case.test_case)
            .map_err(|e| SpoonError::Serialization(e.to_string()))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO verified_regression_cases
             (episode_id, procedure_id, procedure_version, test_case_json)
             VALUES (?1, ?2, ?3, ?4)",
                params![
                    case.episode_id.to_string(),
                    case.procedure_id.to_string(),
                    case.procedure_version,
                    json
                ],
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        Ok(())
    }

    pub fn list_verified_regression_cases(
        &self,
        procedure_id: ProcedureId,
        procedure_version: u32,
    ) -> Result<Vec<VerifiedRegressionCase>, SpoonError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT episode_id, test_case_json FROM verified_regression_cases
             WHERE procedure_id = ?1 AND procedure_version = ?2 ORDER BY episode_id",
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        statement
            .query_map(
                params![procedure_id.to_string(), procedure_version],
                |row| {
                    let episode_id: String = row.get(0)?;
                    let test_case_json: String = row.get(1)?;
                    Ok((episode_id, test_case_json))
                },
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?
            .map(|row| {
                let (episode_id, json) = row.map_err(|e| SpoonError::Storage(e.to_string()))?;
                Ok(VerifiedRegressionCase {
                    episode_id: EpisodeId(parse_uuid(&episode_id)?),
                    procedure_id,
                    procedure_version,
                    test_case: serde_json::from_str(&json)
                        .map_err(|e| SpoonError::Serialization(e.to_string()))?,
                })
            })
            .collect()
    }

    /// Returns an explicitly unfinished episode for recovery/finalization.
    /// Drafts are deliberately absent from normal evidence reads.
    pub fn get_draft(&self, id: EpisodeId) -> Result<Episode, SpoonError> {
        self.get_with_finalization(id, false)
    }

    fn get_with_finalization(&self, id: EpisodeId, finalized: bool) -> Result<Episode, SpoonError> {
        let json: String = self
            .conn
            .query_row(
                "SELECT data_json FROM episodes WHERE id = ?1 AND finalized = ?2",
                params![id.to_string(), finalized],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    SpoonError::NotFound(format!("episode {id}"))
                }
                _ => SpoonError::Storage(e.to_string()),
            })?;

        serde_json::from_str(&json).map_err(|e| SpoonError::Serialization(e.to_string()))
    }

    pub fn list_recent(&self, limit: u32) -> Result<Vec<Episode>, SpoonError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT data_json FROM episodes WHERE finalized = 1
                 ORDER BY created_at DESC LIMIT ?1",
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;

        let episodes = stmt
            .query_map(params![limit], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })
            .map_err(|e| SpoonError::Storage(e.to_string()))?
            .map(|row| {
                let json = row.map_err(|e| SpoonError::Storage(e.to_string()))?;
                serde_json::from_str(&json).map_err(|e| SpoonError::Serialization(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(episodes)
    }

    pub fn list_failures(&self, limit: u32) -> Result<Vec<Episode>, SpoonError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT data_json FROM episodes WHERE finalized = 1 AND success = 0
                 ORDER BY created_at DESC LIMIT ?1",
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;

        let episodes = stmt
            .query_map(params![limit], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })
            .map_err(|e| SpoonError::Storage(e.to_string()))?
            .map(|row| {
                let json = row.map_err(|e| SpoonError::Storage(e.to_string()))?;
                serde_json::from_str(&json).map_err(|e| SpoonError::Serialization(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(episodes)
    }

    /// Find episodes involving a specific concept (in any role).
    pub fn find_by_concept(&self, concept_id: ConceptId) -> Result<Vec<Episode>, SpoonError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT e.data_json FROM episodes e
                 INNER JOIN episode_concepts ec ON e.id = ec.episode_id
                 WHERE ec.concept_id = ?1 AND e.finalized = 1
                 ORDER BY e.created_at DESC",
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;

        let episodes = stmt
            .query_map(params![concept_id.to_string()], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })
            .map_err(|e| SpoonError::Storage(e.to_string()))?
            .map(|row| {
                let json = row.map_err(|e| SpoonError::Storage(e.to_string()))?;
                serde_json::from_str(&json).map_err(|e| SpoonError::Serialization(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(episodes)
    }

    /// Returns immutable completed episodes that carry an exact semantic
    /// observation for `predicate`. The fact itself remains inside the signed
    /// episode payload so a trust receipt covers predicate, value, and scope.
    pub fn find_by_observed_predicate(&self, predicate: &str) -> Result<Vec<Episode>, SpoonError> {
        if predicate.trim().is_empty() {
            return Err(SpoonError::Other(
                "observed-fact predicate must be non-empty".into(),
            ));
        }
        let mut statement = self
            .conn
            .prepare(
                "SELECT episodes.data_json
                 FROM episode_observed_facts facts
                 JOIN episodes ON episodes.id = facts.episode_id
                 WHERE facts.predicate = ?1 AND episodes.finalized = 1
                 GROUP BY episodes.id
                 ORDER BY episodes.created_at DESC, episodes.id",
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        statement
            .query_map(params![predicate], |row| row.get::<_, String>(0))
            .map_err(|e| SpoonError::Storage(e.to_string()))?
            .map(|row| {
                let json = row.map_err(|e| SpoonError::Storage(e.to_string()))?;
                serde_json::from_str(&json).map_err(|e| SpoonError::Serialization(e.to_string()))
            })
            .collect()
    }

    /// Count episodes by escalation rung. Used for section 38 metric 5
    /// (rung distribution drift).
    pub fn rung_distribution(&self) -> Result<Vec<(String, u32)>, SpoonError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT rung_reached, COUNT(*) FROM episodes
                 WHERE finalized = 1 AND rung_reached IS NOT NULL
                 GROUP BY rung_reached
                 ORDER BY COUNT(*) DESC",
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;

        let dist = stmt
            .query_map([], |row| {
                let rung: String = row.get(0)?;
                let count: u32 = row.get(1)?;
                Ok((rung, count))
            })
            .map_err(|e| SpoonError::Storage(e.to_string()))?
            .map(|row| row.map_err(|e| SpoonError::Storage(e.to_string())))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(dist)
    }

    pub fn count(&self) -> Result<u64, SpoonError> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM episodes WHERE finalized = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))
    }

    /// Aggregates only durable teacher requests and finalized outcomes. This
    /// supports an aggregate independence signal, not per-domain or temporal
    /// teacher-weaning claims, because neither domain labels nor cohorts are
    /// part of the episode schema.
    pub fn teacher_interaction_metrics(&self) -> Result<TeacherInteractionMetrics, SpoonError> {
        self.conn
            .query_row(
                "SELECT
                    COALESCE(SUM(CASE WHEN json_type(data_json, '$.teacher_interaction.request') IS NOT NULL THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN success = 1 AND json_type(data_json, '$.teacher_interaction.request') IS NOT NULL THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN success = 1 AND json_type(data_json, '$.teacher_interaction.request') IS NULL THEN 1 ELSE 0 END), 0)
                 FROM episodes WHERE finalized = 1",
                [],
                |row| {
                    Ok(TeacherInteractionMetrics {
                        teacher_interaction_episodes: row.get(0)?,
                        teacher_assisted_successes: row.get(1)?,
                        teacher_free_successes: row.get(2)?,
                    })
                },
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))
    }

    /// Reads materialized sufficient statistics for the requested trace
    /// elements. Work is bounded by unique elements and their pairs, never by
    /// the number or length of historical episodes.
    pub fn credit_aggregate_snapshot(
        &self,
        requested: &[CreditElementRef],
        source_episode: EpisodeId,
    ) -> Result<CreditAggregateSnapshot, SpoonError> {
        self.credit_aggregate_snapshot_inner(requested, source_episode, true)
    }

    /// Reads the same exact sufficient statistics without expanding normalized
    /// provenance identifiers. Engine analysis uses this bounded form; callers
    /// that explicitly need the complete audit trail use
    /// `credit_aggregate_snapshot` and knowingly pay for that history read.
    pub fn credit_aggregate_summary(
        &self,
        requested: &[CreditElementRef],
        source_episode: EpisodeId,
    ) -> Result<CreditAggregateSnapshot, SpoonError> {
        self.credit_aggregate_snapshot_inner(requested, source_episode, false)
    }

    fn credit_aggregate_snapshot_inner(
        &self,
        requested: &[CreditElementRef],
        source_episode: EpisodeId,
        materialize_provenance: bool,
    ) -> Result<CreditAggregateSnapshot, SpoonError> {
        let requested = requested.iter().copied().collect::<BTreeSet<_>>();
        let mut elements = Vec::with_capacity(requested.len());
        for element in &requested {
            let stored = self
                .conn
                .query_row(
                    "SELECT exposures, failures, weighted_exposure, weighted_failures,
                            provenance_count, feedback_rows_scanned,
                            feedback_rows_used, conflicts_excluded, revision
                     FROM credit_element_aggregates
                     WHERE procedure_id = ?1 AND procedure_version = ?2",
                    params![element.procedure.to_string(), element.version],
                    |row| {
                        Ok((
                            row.get::<_, u32>(0)?,
                            row.get::<_, u32>(1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, u32>(4)?,
                            row.get::<_, u64>(5)?,
                            row.get::<_, u64>(6)?,
                            row.get::<_, u64>(7)?,
                            row.get::<_, u64>(8)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage)?;
            let aggregate = if let Some((
                exposures,
                failures,
                weighted_exposure,
                weighted_failures,
                provenance_count,
                feedback_rows_scanned,
                feedback_rows_used,
                conflicts_excluded,
                revision,
            )) = stored
            {
                let (episode_ids, feedback_ids_scanned, feedback_ids_used, conflicting_episode_ids) =
                    if materialize_provenance {
                        self.credit_provenance(*element)?
                    } else {
                        (Vec::new(), Vec::new(), Vec::new(), Vec::new())
                    };
                CreditElementAggregate {
                    element: *element,
                    exposures,
                    failures,
                    weighted_exposure,
                    weighted_failures,
                    provenance_count,
                    episode_ids,
                    feedback_rows_scanned,
                    feedback_rows_used,
                    conflicts_excluded,
                    feedback_ids_scanned,
                    feedback_ids_used,
                    conflicting_episode_ids,
                    revision,
                }
            } else {
                CreditElementAggregate {
                    element: *element,
                    exposures: 0,
                    failures: 0,
                    weighted_exposure: 0.0,
                    weighted_failures: 0.0,
                    provenance_count: 0,
                    episode_ids: Vec::new(),
                    feedback_rows_scanned: 0,
                    feedback_rows_used: 0,
                    conflicts_excluded: 0,
                    feedback_ids_scanned: Vec::new(),
                    feedback_ids_used: Vec::new(),
                    conflicting_episode_ids: Vec::new(),
                    revision: 0,
                }
            };
            elements.push(aggregate);
        }
        let requested = requested.into_iter().collect::<Vec<_>>();
        let mut pairs = Vec::new();
        for left_index in 0..requested.len() {
            for right in &requested[(left_index + 1)..] {
                let (left, right) = ordered_elements(requested[left_index], *right);
                let stored = self
                    .conn
                    .query_row(
                        "SELECT together, revision FROM credit_pair_aggregates
                         WHERE left_procedure_id = ?1 AND left_version = ?2
                           AND right_procedure_id = ?3 AND right_version = ?4",
                        params![
                            left.procedure.to_string(),
                            left.version,
                            right.procedure.to_string(),
                            right.version,
                        ],
                        |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u64>(1)?)),
                    )
                    .optional()
                    .map_err(storage)?
                    .unwrap_or((0, 0));
                pairs.push(CreditPairAggregate {
                    left,
                    right,
                    together: stored.0,
                    revision: stored.1,
                });
            }
        }
        let (_, source, source_index_maintenance_work_units) =
            Self::credit_elements_and_state(&self.conn, source_episode)?;
        Ok(CreditAggregateSnapshot {
            elements,
            pairs,
            source_contribution: CreditEpisodeContribution {
                included: source.included,
                failed: source.failed,
                weight: source.weight,
            },
            source_index_maintenance_work_units,
            provenance_materialized: materialize_provenance,
        })
    }

    fn credit_provenance(&self, element: CreditElementRef) -> Result<CreditProvenance, SpoonError> {
        let episode_ids = self.credit_episode_ids(element, "included = 1")?;
        let conflicting_episode_ids = self.credit_episode_ids(element, "conflict = 1")?;
        let feedback_ids_scanned = self.credit_feedback_ids(element, false)?;
        let feedback_ids_used = self.credit_feedback_ids(element, true)?;
        Ok((
            episode_ids,
            feedback_ids_scanned,
            feedback_ids_used,
            conflicting_episode_ids,
        ))
    }

    fn credit_episode_ids(
        &self,
        element: CreditElementRef,
        predicate: &str,
    ) -> Result<Vec<EpisodeId>, SpoonError> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT episode_id FROM episode_credit_elements
                 WHERE procedure_id = ?1 AND procedure_version = ?2 AND {predicate}
                 ORDER BY episode_id"
            ))
            .map_err(storage)?;
        statement
            .query_map(
                params![element.procedure.to_string(), element.version],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage)?
            .map(|row| Ok(EpisodeId(parse_uuid(&row.map_err(storage)?)?)))
            .collect()
    }

    fn credit_feedback_ids(
        &self,
        element: CreditElementRef,
        used_only: bool,
    ) -> Result<Vec<Uuid>, SpoonError> {
        let used = if used_only {
            "AND element.feedback_rows_used > 0"
        } else {
            ""
        };
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT feedback.id
                 FROM episode_credit_elements AS element
                 JOIN episode_feedback AS feedback ON feedback.episode_id = element.episode_id
                 WHERE element.procedure_id = ?1 AND element.procedure_version = ?2 {used}
                 ORDER BY feedback.id"
            ))
            .map_err(storage)?;
        statement
            .query_map(
                params![element.procedure.to_string(), element.version],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage)?
            .map(|row| parse_uuid(&row.map_err(storage)?))
            .collect()
    }

    /// Appends late evidence without changing the original episode.
    ///
    /// The idempotency key is scoped to the episode. An exact semantic retry
    /// returns the first stored record; reusing the key for different evidence
    /// is rejected rather than silently discarding either payload.
    pub fn append_feedback(
        &self,
        feedback: &EpisodeFeedback,
    ) -> Result<EpisodeFeedback, SpoonError> {
        let transaction = self
            .conn
            .unchecked_transaction()
            .map_err(|e| SpoonError::Storage(e.to_string()))?;

        let finalized = transaction
            .query_row(
                "SELECT finalized FROM episodes WHERE id = ?1",
                params![feedback.episode_id.to_string()],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        match finalized {
            None => {
                return Err(SpoonError::NotFound(format!(
                    "episode {}",
                    feedback.episode_id
                )));
            }
            Some(false) => {
                return Err(SpoonError::Other(format!(
                    "episode {} is still a draft",
                    feedback.episode_id
                )));
            }
            Some(true) => {}
        }

        let inserted = transaction
            .execute(
                "INSERT INTO episode_feedback
                    (id, episode_id, observed_result_json, evaluation_json, source_json,
                     idempotency_key, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(episode_id, idempotency_key) DO NOTHING",
                params![
                    feedback.id.to_string(),
                    feedback.episode_id.to_string(),
                    serde_json::to_string(&feedback.observed_result)
                        .map_err(|e| SpoonError::Serialization(e.to_string()))?,
                    serde_json::to_string(&feedback.evaluation)
                        .map_err(|e| SpoonError::Serialization(e.to_string()))?,
                    serde_json::to_string(&feedback.source)
                        .map_err(|e| SpoonError::Serialization(e.to_string()))?,
                    feedback.idempotency_key,
                    feedback.created_at,
                ],
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;

        let stored = transaction
            .query_row(
                "SELECT id, episode_id, observed_result_json, evaluation_json, source_json,
                        idempotency_key, created_at
                 FROM episode_feedback
                 WHERE episode_id = ?1 AND idempotency_key = ?2",
                params![feedback.episode_id.to_string(), feedback.idempotency_key],
                Self::feedback_from_row,
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))??;
        if inserted == 0 && !Self::same_feedback_payload(&stored, feedback) {
            return Err(SpoonError::Other(format!(
                "feedback idempotency key '{}' was reused with a different payload for episode {}",
                feedback.idempotency_key, feedback.episode_id
            )));
        }
        if inserted != 0 {
            Self::increment_feedback_credit_summary(&transaction, feedback)?;
            Self::refresh_credit_episode(&transaction, feedback.episode_id)?;
        }
        transaction
            .commit()
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        Ok(stored)
    }

    fn same_feedback_payload(left: &EpisodeFeedback, right: &EpisodeFeedback) -> bool {
        left.episode_id == right.episode_id
            && left.observed_result == right.observed_result
            && left.evaluation.tier == right.evaluation.tier
            && left.evaluation.success == right.evaluation.success
            && left.evaluation.details == right.evaluation.details
            && left.evaluation.surprise == right.evaluation.surprise
            && left.source == right.source
            && left.idempotency_key == right.idempotency_key
    }

    /// Lists all late feedback for an episode in stable append order.
    pub fn list_feedback(&self, episode_id: EpisodeId) -> Result<Vec<EpisodeFeedback>, SpoonError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT id, episode_id, observed_result_json, evaluation_json, source_json,
                        idempotency_key, created_at
                 FROM episode_feedback
                 WHERE episode_id = ?1
                 ORDER BY created_at ASC, rowid ASC",
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        statement
            .query_map(params![episode_id.to_string()], Self::feedback_from_row)
            .map_err(|e| SpoonError::Storage(e.to_string()))?
            .map(|row| row.map_err(|e| SpoonError::Storage(e.to_string()))?)
            .collect()
    }

    fn feedback_from_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<EpisodeFeedback, SpoonError>> {
        Ok((|| {
            let id: String = row.get(0).map_err(|e| SpoonError::Storage(e.to_string()))?;
            let episode_id: String = row.get(1).map_err(|e| SpoonError::Storage(e.to_string()))?;
            let observed_result_json: String =
                row.get(2).map_err(|e| SpoonError::Storage(e.to_string()))?;
            let evaluation_json: String =
                row.get(3).map_err(|e| SpoonError::Storage(e.to_string()))?;
            let source_json: String = row.get(4).map_err(|e| SpoonError::Storage(e.to_string()))?;
            Ok(EpisodeFeedback {
                id: Uuid::parse_str(&id).map_err(|e| SpoonError::Serialization(e.to_string()))?,
                episode_id: EpisodeId(
                    Uuid::parse_str(&episode_id)
                        .map_err(|e| SpoonError::Serialization(e.to_string()))?,
                ),
                observed_result: serde_json::from_str(&observed_result_json)
                    .map_err(|e| SpoonError::Serialization(e.to_string()))?,
                evaluation: serde_json::from_str(&evaluation_json)
                    .map_err(|e| SpoonError::Serialization(e.to_string()))?,
                source: serde_json::from_str(&source_json)
                    .map_err(|e| SpoonError::Serialization(e.to_string()))?,
                idempotency_key: row.get(5).map_err(|e| SpoonError::Storage(e.to_string()))?,
                created_at: row.get(6).map_err(|e| SpoonError::Storage(e.to_string()))?,
            })
        })())
    }

    /// Query episodes using composable indexed filters.
    pub fn query(&self, query: &EpisodeQuery) -> Result<Vec<Episode>, SpoonError> {
        let rung = query.rung.and_then(|value| {
            serde_json::to_value(value)
                .ok()
                .and_then(|json| json.as_str().map(str::to_owned))
        });
        let concept = query.concept.map(|value| value.to_string());
        let outcome = query.outcome.map(i32::from);

        let mut stmt = self
            .conn
            .prepare(
                "SELECT e.data_json FROM episodes e
                 WHERE e.finalized = 1
                   AND (?1 IS NULL OR e.created_at >= ?1)
                   AND (?2 IS NULL OR e.created_at <= ?2)
                   AND (?3 IS NULL OR e.success = ?3)
                   AND (?4 IS NULL OR e.rung_reached = ?4)
                   AND (?5 IS NULL OR EXISTS (
                       SELECT 1 FROM episode_concepts ec
                       WHERE ec.episode_id = e.id AND ec.concept_id = ?5
                   ))
                 ORDER BY e.created_at DESC
                 LIMIT ?6",
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;

        let episodes = stmt
            .query_map(
                params![
                    query.since,
                    query.until,
                    outcome,
                    rung,
                    concept,
                    query.limit,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?
            .map(|row| {
                let json = row.map_err(|e| SpoonError::Storage(e.to_string()))?;
                serde_json::from_str(&json).map_err(|e| SpoonError::Serialization(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(episodes)
    }

    /// Completes an explicitly inserted draft exactly once.
    pub fn finalize_draft(&self, episode: &Episode) -> Result<(), SpoonError> {
        let data_json =
            serde_json::to_string(episode).map_err(|e| SpoonError::Serialization(e.to_string()))?;

        let success = episode.evaluation.as_ref().map(|e| e.success as i32);
        let rung = serde_json::to_value(episode.cost.rung_reached)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        let transaction = self
            .conn
            .unchecked_transaction()
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        let state = transaction
            .query_row(
                "SELECT finalized, created_at FROM episodes WHERE id = ?1",
                params![episode.id.to_string()],
                |row| Ok((row.get::<_, bool>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        let Some((finalized, created_at)) = state else {
            return Err(SpoonError::NotFound(format!("episode {}", episode.id)));
        };
        if finalized {
            return Err(SpoonError::Other(format!(
                "episode {} is finalized and immutable",
                episode.id
            )));
        }
        if episode.created_at != created_at {
            return Err(SpoonError::Other(format!(
                "episode {} cannot change immutable created_at",
                episode.id
            )));
        }
        let rows = transaction
            .execute(
                "UPDATE episodes
                 SET situation = ?1, data_json = ?2, success = ?3, rung_reached = ?4, finalized = 1
                 WHERE id = ?5 AND finalized = 0",
                params![
                    episode.situation,
                    data_json,
                    success,
                    rung,
                    episode.id.to_string()
                ],
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;

        if rows == 0 {
            return Err(SpoonError::Storage(format!(
                "episode {} draft could not be finalized",
                episode.id
            )));
        }

        transaction
            .execute(
                "DELETE FROM episode_concepts WHERE episode_id = ?1",
                params![episode.id.to_string()],
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        Self::insert_concept_index(&transaction, episode)?;
        transaction
            .execute(
                "DELETE FROM episode_observed_facts WHERE episode_id = ?1",
                params![episode.id.to_string()],
            )
            .map_err(|e| SpoonError::Storage(e.to_string()))?;
        Self::insert_observed_fact_index(&transaction, episode)?;
        Self::index_new_episode(&transaction, episode)?;

        transaction
            .commit()
            .map_err(|e| SpoonError::Storage(e.to_string()))
    }

    /// Deprecated compatibility alias for [`Self::finalize_draft`]. It cannot
    /// rewrite an episode inserted through [`Self::insert`].
    pub fn update(&self, episode: &Episode) -> Result<(), SpoonError> {
        self.finalize_draft(episode)
    }
}

fn credit_elements(episode: &Episode) -> Vec<CreditElementRef> {
    episode
        .execution_trace
        .as_ref()
        .and_then(|json| serde_json::from_value::<ExecTrace>(json.clone()).ok())
        .map(|trace| {
            trace
                .steps
                .into_iter()
                .filter_map(|step| {
                    Some(CreditElementRef {
                        procedure: step.procedure_called?,
                        version: step.procedure_version?,
                    })
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default()
}

fn effective_credit_state(
    episode: &Episode,
    feedback: FeedbackCreditSummary,
    has_elements: bool,
) -> CreditEpisodeState {
    debug_assert_eq!(
        feedback
            .hard
            .saturating_add(feedback.consensus)
            .saturating_add(feedback.deferred),
        feedback.total
    );
    if feedback.total == 0 {
        let evaluation = episode.evaluation.as_ref();
        return CreditEpisodeState {
            included: has_elements && evaluation.is_some(),
            failed: evaluation.is_some_and(|value| !value.success),
            weight: evaluation.map_or(0.0, |value| tier_weight(value.tier)),
            feedback_rows_scanned: 0,
            feedback_rows_used: 0,
            conflict: false,
        };
    }
    if feedback.successes > 0 && feedback.failures > 0 {
        return CreditEpisodeState {
            included: false,
            failed: false,
            weight: 0.0,
            feedback_rows_scanned: feedback.total,
            feedback_rows_used: 0,
            conflict: true,
        };
    }
    let tier = if feedback.deferred > 0 {
        VerifiabilityTier::Deferred
    } else if feedback.consensus > 0 {
        VerifiabilityTier::Consensus
    } else {
        VerifiabilityTier::Hard
    };
    CreditEpisodeState {
        included: has_elements,
        failed: feedback.failures > 0,
        weight: tier_weight(tier),
        feedback_rows_scanned: feedback.total,
        feedback_rows_used: feedback.total,
        conflict: false,
    }
}

fn index_maintenance_work_units(element_count: usize, feedback_changed: bool) -> u64 {
    let elements = element_count as u64;
    let pairs = elements.saturating_mul(elements.saturating_sub(1)) / 2;
    1_u64
        .saturating_add(u64::from(feedback_changed))
        .saturating_add(elements.saturating_mul(2))
        .saturating_add(pairs)
}

fn tier_weight(tier: VerifiabilityTier) -> f64 {
    match tier {
        VerifiabilityTier::Hard => 1.0,
        VerifiabilityTier::Consensus => 0.6,
        VerifiabilityTier::Deferred => 0.2,
    }
}

fn ordered_elements(
    left: CreditElementRef,
    right: CreditElementRef,
) -> (CreditElementRef, CreditElementRef) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, SpoonError> {
    Uuid::parse_str(value).map_err(serialization)
}

fn storage(error: impl std::fmt::Display) -> SpoonError {
    SpoonError::Storage(error.to_string())
}

fn serialization(error: impl std::fmt::Display) -> SpoonError {
    SpoonError::Serialization(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spoon_core::Value;
    use spoon_core::evidence::VerifiabilityTier;

    fn make_episode(situation: impl Into<String>) -> Episode {
        Episode::new(situation)
    }

    fn corrupt_episode_json(store: &EpisodeStore, id: EpisodeId, sql_value: &str) {
        store
            .conn
            .execute(
                &format!("UPDATE episodes SET data_json = {sql_value} WHERE id = ?1"),
                params![id.to_string()],
            )
            .unwrap();
    }

    #[test]
    fn insert_and_retrieve() {
        let store = EpisodeStore::in_memory().unwrap();
        let ep = make_episode("what is 2 + 2?");

        store.insert(&ep).unwrap();
        let retrieved = store.get(ep.id).unwrap();

        assert_eq!(retrieved.id, ep.id);
        assert_eq!(retrieved.situation, "what is 2 + 2?");
    }

    #[test]
    fn feedback_is_append_only_idempotent_and_does_not_rewrite_the_episode() {
        let store = EpisodeStore::in_memory().unwrap();
        let episode = make_episode("make pancakes");
        store.insert(&episode).unwrap();
        let original: String = store
            .conn
            .query_row(
                "SELECT data_json FROM episodes WHERE id = ?1",
                params![episode.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();

        let feedback = EpisodeFeedback::new(
            episode.id,
            Value::Text("flat pancakes".into()),
            Evaluation {
                tier: VerifiabilityTier::Deferred,
                success: false,
                details: "human reported flat pancakes".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("human", Some("kitchen-tester".into())),
            "feedback-key-1",
        );
        let first = store.append_feedback(&feedback).unwrap();
        let retried = store.append_feedback(&feedback).unwrap();

        assert_eq!(first, retried);
        assert_eq!(store.list_feedback(episode.id).unwrap(), vec![first]);
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT data_json FROM episodes WHERE id = ?1",
                    params![episode.id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            original
        );
    }

    #[test]
    fn idempotent_retry_rejects_a_different_feedback_payload() {
        let store = EpisodeStore::in_memory().unwrap();
        let episode = make_episode("make pancakes");
        store.insert(&episode).unwrap();
        let first = EpisodeFeedback::new(
            episode.id,
            Value::Text("flat pancakes".into()),
            Evaluation {
                tier: VerifiabilityTier::Deferred,
                success: false,
                details: "first report".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("human", None),
            "same-delivery",
        );
        let mut retry = EpisodeFeedback::new(
            episode.id,
            Value::Text("incorrect retry payload".into()),
            Evaluation {
                tier: VerifiabilityTier::Deferred,
                success: true,
                details: "this must not overwrite the first report".into(),
                surprise: None,
            },
            FeedbackSource::new("human", None),
            "same-delivery",
        );
        retry.id = Uuid::new_v4();

        let stored = store.append_feedback(&first).unwrap();
        let error = store.append_feedback(&retry).unwrap_err();
        assert!(error.to_string().contains("idempotency key"));
        assert_eq!(store.list_feedback(episode.id).unwrap(), vec![stored]);
    }

    #[test]
    fn semantic_feedback_retry_returns_original_identity_and_timestamp() {
        let store = EpisodeStore::in_memory().unwrap();
        let episode = make_episode("make pancakes");
        store.insert(&episode).unwrap();
        let first = EpisodeFeedback::new(
            episode.id,
            Value::Text("flat pancakes".into()),
            Evaluation {
                tier: VerifiabilityTier::Deferred,
                success: false,
                details: "same report".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("human", Some("kitchen-tester".into())),
            "semantic-retry",
        );
        let mut retry = EpisodeFeedback::new(
            episode.id,
            first.observed_result.clone(),
            first.evaluation.clone(),
            first.source.clone(),
            "semantic-retry",
        );
        retry.created_at = first.created_at.saturating_add(10);

        let stored = store.append_feedback(&first).unwrap();
        let retried = store.append_feedback(&retry).unwrap();

        assert_eq!(retried, stored);
        assert_ne!(retry.id, stored.id);
        assert_ne!(retry.created_at, stored.created_at);
        assert_eq!(store.list_feedback(episode.id).unwrap(), vec![stored]);
    }

    #[test]
    fn feedback_rejects_an_unknown_episode() {
        let store = EpisodeStore::in_memory().unwrap();
        let feedback = EpisodeFeedback::new(
            EpisodeId::new(),
            Value::Null,
            Evaluation {
                tier: VerifiabilityTier::Deferred,
                success: false,
                details: "unknown episode".into(),
                surprise: None,
            },
            FeedbackSource::new("human", None),
            "unknown",
        );

        assert!(matches!(
            store.append_feedback(&feedback),
            Err(SpoonError::NotFound(_))
        ));
    }

    #[test]
    fn feedback_rejects_an_unfinished_draft() {
        let store = EpisodeStore::in_memory().unwrap();
        let episode = make_episode("unfinished");
        store.insert_draft(&episode).unwrap();
        let feedback = EpisodeFeedback::new(
            episode.id,
            Value::Null,
            Evaluation {
                tier: VerifiabilityTier::Deferred,
                success: false,
                details: "too early".into(),
                surprise: None,
            },
            FeedbackSource::new("human", None),
            "draft-feedback",
        );

        assert!(matches!(
            store.append_feedback(&feedback),
            Err(SpoonError::Other(message)) if message.contains("draft")
        ));
    }

    #[test]
    fn evaluated_drafts_are_rejected_and_unfinished_drafts_are_hidden_from_normal_reads() {
        let store = EpisodeStore::in_memory().unwrap();
        let mut evaluated = make_episode("forged evaluated draft");
        evaluated.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: false,
            details: "must not enter evidence views before finalization".into(),
            surprise: Some(1.0),
        });
        assert!(store.insert_draft(&evaluated).is_err());

        let draft = make_episode("unfinished but valid draft");
        store.insert_draft(&draft).unwrap();

        assert!(matches!(store.get(draft.id), Err(SpoonError::NotFound(_))));
        assert_eq!(store.get_draft(draft.id).unwrap().id, draft.id);
        assert!(store.list_recent(10).unwrap().is_empty());
        assert!(store.list_failures(10).unwrap().is_empty());
        assert!(store.query(&EpisodeQuery::default()).unwrap().is_empty());
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn conflicting_late_feedback_is_retained_as_separate_evidence() {
        let store = EpisodeStore::in_memory().unwrap();
        let episode = make_episode("make pancakes");
        store.insert(&episode).unwrap();

        for (key, observed, success) in [
            ("feedback-flat", "flat", false),
            ("feedback-good", "good rise", true),
        ] {
            store
                .append_feedback(&EpisodeFeedback::new(
                    episode.id,
                    Value::Text(observed.into()),
                    Evaluation {
                        tier: VerifiabilityTier::Deferred,
                        success,
                        details: observed.into(),
                        surprise: None,
                    },
                    FeedbackSource::new("human", None),
                    key,
                ))
                .unwrap();
        }

        let feedback = store.list_feedback(episode.id).unwrap();
        assert_eq!(feedback.len(), 2);
        assert_ne!(
            feedback[0].evaluation.success,
            feedback[1].evaluation.success
        );
    }

    #[test]
    fn insert_rolls_back_episode_when_concept_indexing_fails() {
        let store = EpisodeStore::in_memory().unwrap();
        let mut episode = make_episode("must be atomic");
        episode.context.entities.push(ConceptId::new());
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_concept_insert
                 BEFORE INSERT ON episode_concepts
                 BEGIN
                     SELECT RAISE(ABORT, 'rejected concept insert');
                 END;",
            )
            .unwrap();

        assert!(matches!(
            store.insert(&episode),
            Err(SpoonError::Storage(_))
        ));
        assert_eq!(store.count().unwrap(), 0);
        assert!(matches!(
            store.get(episode.id),
            Err(SpoonError::NotFound(_))
        ));
    }

    #[test]
    fn list_recent() {
        let store = EpisodeStore::in_memory().unwrap();

        store.insert(&make_episode("first")).unwrap();
        store.insert(&make_episode("second")).unwrap();
        store.insert(&make_episode("third")).unwrap();

        let recent = store.list_recent(2).unwrap();
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn list_failures() {
        let store = EpisodeStore::in_memory().unwrap();

        let mut success = make_episode("success");
        success.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "correct".into(),
            surprise: None,
        });

        let mut failure = make_episode("failure");
        failure.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: false,
            details: "wrong".into(),
            surprise: Some(1.0),
        });

        store.insert(&success).unwrap();
        store.insert(&failure).unwrap();

        let failures = store.list_failures(10).unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].situation, "failure");
    }

    #[test]
    fn find_by_concept() {
        let store = EpisodeStore::in_memory().unwrap();
        let concept = ConceptId::new();

        let mut ep = make_episode("involves a concept");
        ep.context.entities.push(concept);

        store.insert(&ep).unwrap();

        let found = store.find_by_concept(concept).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, ep.id);
    }

    #[test]
    fn observed_fact_lookup_requires_an_exact_predicate_and_indexes_finalization() {
        let store = EpisodeStore::in_memory().unwrap();
        let mut episode = make_episode("semantic observation");
        store.insert_draft(&episode).unwrap();
        episode.observed_result = Some(Value::Bool(true));
        episode.observed_facts.push(spoon_core::ObservedFact::new(
            "concept:rise",
            Value::Bool(true),
            Default::default(),
        ));
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "direct observation".into(),
            surprise: None,
        });
        store.finalize_draft(&episode).unwrap();

        assert_eq!(
            store.find_by_observed_predicate("concept:rise").unwrap()[0].id,
            episode.id
        );
        assert!(
            store
                .find_by_observed_predicate("concept:different")
                .unwrap()
                .is_empty()
        );
        assert!(store.find_by_observed_predicate(" ").is_err());
    }

    #[test]
    fn update_episode() {
        let store = EpisodeStore::in_memory().unwrap();
        let mut ep = make_episode("pending evaluation");

        store.insert_draft(&ep).unwrap();

        ep.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "verified".into(),
            surprise: None,
        });
        ep.observed_result = Some(Value::Int(42));

        store.finalize_draft(&ep).unwrap();

        let retrieved = store.get(ep.id).unwrap();
        assert!(retrieved.succeeded());
        assert_eq!(retrieved.observed_result, Some(Value::Int(42)));
    }

    #[test]
    fn finalized_episode_cannot_be_rewritten() {
        let store = EpisodeStore::in_memory().unwrap();
        let mut episode = make_episode("immutable outcome");
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "original".into(),
            surprise: None,
        });
        store.insert(&episode).unwrap();
        let original: String = store
            .conn
            .query_row(
                "SELECT data_json FROM episodes WHERE id = ?1",
                params![episode.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();

        episode.evaluation.as_mut().unwrap().success = false;
        episode.evaluation.as_mut().unwrap().details = "rewritten".into();
        assert!(matches!(
            store.update(&episode),
            Err(SpoonError::Other(message)) if message.contains("finalized")
        ));
        let current: String = store
            .conn
            .query_row(
                "SELECT data_json FROM episodes WHERE id = ?1",
                params![episode.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(current, original);
    }

    #[test]
    fn draft_can_only_be_finalized_once() {
        let store = EpisodeStore::in_memory().unwrap();
        let mut episode = make_episode("one-way transition");
        store.insert_draft(&episode).unwrap();
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "final".into(),
            surprise: None,
        });

        store.finalize_draft(&episode).unwrap();
        assert!(store.finalize_draft(&episode).is_err());
    }

    #[test]
    fn update_rebuilds_concept_index() {
        let store = EpisodeStore::in_memory().unwrap();
        let removed = ConceptId::new();
        let added = ConceptId::new();
        let mut episode = make_episode("changing concepts");
        episode.context.entities.push(removed);
        store.insert_draft(&episode).unwrap();

        episode.context.entities.clear();
        episode.context.entities.push(added);
        store.finalize_draft(&episode).unwrap();

        assert!(store.find_by_concept(removed).unwrap().is_empty());
        assert_eq!(store.find_by_concept(added).unwrap()[0].id, episode.id);
    }

    #[test]
    fn update_rolls_back_episode_and_concept_index_when_rebuild_fails() {
        let store = EpisodeStore::in_memory().unwrap();
        let original = ConceptId::new();
        let replacement = ConceptId::new();
        let mut episode = make_episode("original situation");
        episode.context.entities.push(original);
        store.insert_draft(&episode).unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_concept_insert
                 BEFORE INSERT ON episode_concepts
                 BEGIN
                     SELECT RAISE(ABORT, 'rejected concept insert');
                 END;",
            )
            .unwrap();

        episode.situation = "updated situation".into();
        episode.context.entities.clear();
        episode.context.entities.push(replacement);

        assert!(matches!(
            store.update(&episode),
            Err(SpoonError::Storage(_))
        ));
        assert_eq!(
            store.get_draft(episode.id).unwrap().situation,
            "original situation"
        );
        let original_index_count: u32 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM episode_concepts
                 WHERE episode_id = ?1 AND concept_id = ?2",
                params![episode.id.to_string(), original.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(original_index_count, 1);
        assert!(store.find_by_concept(replacement).unwrap().is_empty());
    }

    #[test]
    fn count() {
        let store = EpisodeStore::in_memory().unwrap();
        assert_eq!(store.count().unwrap(), 0);

        store.insert(&make_episode("one")).unwrap();
        store.insert(&make_episode("two")).unwrap();
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn not_found() {
        let store = EpisodeStore::in_memory().unwrap();
        let result = store.get(EpisodeId::new());
        assert!(result.is_err());
    }

    #[test]
    fn query_filters_by_time_outcome_and_rung() {
        let store = EpisodeStore::in_memory().unwrap();

        let mut old_success = make_episode("old success");
        old_success.created_at = 100;
        old_success.cost.rung_reached = EscalationRung::Recall;
        old_success.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "verified".into(),
            surprise: None,
        });

        let mut recent_failure = make_episode("recent failure");
        recent_failure.created_at = 200;
        recent_failure.cost.rung_reached = EscalationRung::Run;
        recent_failure.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: false,
            details: "wrong".into(),
            surprise: Some(1.0),
        });

        let mut recent_success = make_episode("recent success");
        recent_success.created_at = 300;
        recent_success.cost.rung_reached = EscalationRung::Run;
        recent_success.evaluation = old_success.evaluation.clone();

        store.insert(&old_success).unwrap();
        store.insert(&recent_failure).unwrap();
        store.insert(&recent_success).unwrap();

        let found = store
            .query(&EpisodeQuery {
                since: Some(150),
                until: Some(350),
                outcome: Some(true),
                rung: Some(EscalationRung::Run),
                ..EpisodeQuery::default()
            })
            .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, recent_success.id);
    }

    #[test]
    fn query_filters_by_concept_and_limit() {
        let store = EpisodeStore::in_memory().unwrap();
        let concept = ConceptId::new();

        for created_at in [100, 200, 300] {
            let mut episode = make_episode(format!("episode {created_at}"));
            episode.created_at = created_at;
            episode.context.entities.push(concept);
            store.insert(&episode).unwrap();
        }
        store.insert(&make_episode("unrelated")).unwrap();

        let found = store
            .query(&EpisodeQuery {
                concept: Some(concept),
                limit: 2,
                ..EpisodeQuery::default()
            })
            .unwrap();

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].created_at, 300);
        assert_eq!(found[1].created_at, 200);
    }

    #[test]
    fn episode_collection_reads_propagate_malformed_json() {
        let store = EpisodeStore::in_memory().unwrap();
        let concept = ConceptId::new();
        let mut episode = make_episode("corrupt JSON");
        episode.context.entities.push(concept);
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: false,
            details: "failed".into(),
            surprise: None,
        });
        store.insert(&episode).unwrap();
        corrupt_episode_json(&store, episode.id, "'{'");

        assert!(matches!(
            store.list_recent(10),
            Err(SpoonError::Serialization(_))
        ));
        assert!(matches!(
            store.list_failures(10),
            Err(SpoonError::Serialization(_))
        ));
        assert!(matches!(
            store.find_by_concept(concept),
            Err(SpoonError::Serialization(_))
        ));
        assert!(matches!(
            store.query(&EpisodeQuery {
                concept: Some(concept),
                ..EpisodeQuery::default()
            }),
            Err(SpoonError::Serialization(_))
        ));
    }

    #[test]
    fn episode_collection_reads_propagate_sqlite_row_errors() {
        let store = EpisodeStore::in_memory().unwrap();
        let concept = ConceptId::new();
        let mut episode = make_episode("invalid SQLite type");
        episode.context.entities.push(concept);
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: false,
            details: "failed".into(),
            surprise: None,
        });
        store.insert(&episode).unwrap();
        corrupt_episode_json(&store, episode.id, "x'FF'");

        assert!(matches!(store.list_recent(10), Err(SpoonError::Storage(_))));
        assert!(matches!(
            store.list_failures(10),
            Err(SpoonError::Storage(_))
        ));
        assert!(matches!(
            store.find_by_concept(concept),
            Err(SpoonError::Storage(_))
        ));
        assert!(matches!(
            store.query(&EpisodeQuery {
                concept: Some(concept),
                ..EpisodeQuery::default()
            }),
            Err(SpoonError::Storage(_))
        ));
    }

    #[test]
    fn rung_distribution_propagates_sqlite_row_errors() {
        let store = EpisodeStore::in_memory().unwrap();
        let episode = make_episode("invalid rung type");
        store.insert(&episode).unwrap();
        store
            .conn
            .execute(
                "UPDATE episodes SET rung_reached = x'FF' WHERE id = ?1",
                params![episode.id.to_string()],
            )
            .unwrap();

        assert!(matches!(
            store.rung_distribution(),
            Err(SpoonError::Storage(_))
        ));
    }

    #[test]
    fn credit_aggregates_update_transactionally_for_feedback_and_idempotent_retries() {
        use spoon_exec::{ContractChecks, ExecStep, ExecTrace};

        let store = EpisodeStore::in_memory().unwrap();
        let left = CreditElementRef {
            procedure: ProcedureId::new(),
            version: 1,
        };
        let right = CreditElementRef {
            procedure: ProcedureId::new(),
            version: 2,
        };
        let mut episode = make_episode("indexed evidence");
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "initial success".into(),
            surprise: None,
        });
        episode.execution_trace = Some(
            serde_json::to_value(ExecTrace {
                steps: vec![
                    ExecStep::for_versioned_call(
                        left.procedure,
                        "left",
                        &[],
                        Value::Null,
                        Some(left.version),
                        ContractChecks::default(),
                    ),
                    ExecStep::for_versioned_call(
                        right.procedure,
                        "right",
                        &[],
                        Value::Null,
                        Some(right.version),
                        ContractChecks::default(),
                    ),
                ],
            })
            .unwrap(),
        );
        store.insert(&episode).unwrap();

        let initial = store
            .credit_aggregate_snapshot(&[left, right], episode.id)
            .unwrap();
        assert!(
            initial
                .elements
                .iter()
                .all(|aggregate| (aggregate.exposures, aggregate.failures) == (1, 0))
        );
        assert_eq!(initial.pairs[0].together, 1);

        let failed = EpisodeFeedback::new(
            episode.id,
            Value::Text("failed".into()),
            Evaluation {
                tier: VerifiabilityTier::Consensus,
                success: false,
                details: "late failure".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("test", None),
            "indexed-failure",
        );
        store.append_feedback(&failed).unwrap();
        let after_failure = store
            .credit_aggregate_snapshot(&[left, right], episode.id)
            .unwrap();
        assert!(after_failure.elements.iter().all(|aggregate| {
            (aggregate.exposures, aggregate.failures) == (1, 1)
                && (aggregate.weighted_exposure - 0.6).abs() < f64::EPSILON
        }));
        let revisions = after_failure
            .elements
            .iter()
            .map(|aggregate| aggregate.revision)
            .collect::<Vec<_>>();
        store.append_feedback(&failed).unwrap();
        let retried = store
            .credit_aggregate_snapshot(&[left, right], episode.id)
            .unwrap();
        assert_eq!(
            retried
                .elements
                .iter()
                .map(|aggregate| aggregate.revision)
                .collect::<Vec<_>>(),
            revisions
        );

        store
            .append_feedback(&EpisodeFeedback::new(
                episode.id,
                Value::Text("passed".into()),
                Evaluation {
                    tier: VerifiabilityTier::Hard,
                    success: true,
                    details: "conflicts with late failure".into(),
                    surprise: None,
                },
                FeedbackSource::new("test", None),
                "indexed-conflict",
            ))
            .unwrap();
        let conflicted = store
            .credit_aggregate_snapshot(&[left, right], episode.id)
            .unwrap();
        assert!(
            conflicted
                .elements
                .iter()
                .all(|aggregate| aggregate.exposures == 0
                    && aggregate.conflicting_episode_ids == [episode.id])
        );
        assert_eq!(conflicted.pairs[0].together, 0);
    }
}
