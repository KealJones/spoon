//! Phase 3 intuition primitives.
//!
//! This crate deliberately keeps retrieval and ranking separate from belief
//! mutation. It can change candidate order and representation statistics, but
//! it cannot promote a claim, mint trust, or alter graph lifecycle state.

use std::collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const VECTOR_DIMENSIONS: usize = 32;
const MAX_QUERY_TERMS: usize = 64;
const MAX_SEMANTIC_EXPANSION_TERMS: usize = 12;
const MAX_SEMANTIC_SEED_DOCUMENTS: usize = 256;
const MAX_SEMANTIC_EVALUATION_EXAMPLES: usize = 4_096;
const SEMANTIC_EXPANSION_WEIGHT: f64 = 0.65;
const MAX_RANKING_EVALUATION_HOLDOUT: usize = 256;
const MAX_RANKING_MODEL_EXAMPLES: usize = 4_096;
const MAX_REPRESENTATION_TRAINING_HOLDOUT: usize = 256;
const RANKING_FEATURE_COUNT: usize = 5;
const RANKING_MODEL_REGULARIZATION: f64 = 0.02;
/// A deliberately small lifetime budget for locally replayed supervision.
/// This prevents a successful trace from being amplified into an unbounded
/// self-training corpus merely by repeatedly asking for challenges.
pub const MAX_AUTO_GROUNDED_SUPERVISION_TASKS: u64 = 32;
const VERIFIED_TRACE_REPLAY_KIND: &str = "verified_trace_replay";

#[derive(Debug, Error)]
pub enum IntuitionError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallKind {
    Concept,
    Procedure,
    Episode,
}

impl RecallKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Concept => "concept",
            Self::Procedure => "procedure",
            Self::Episode => "episode",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallDocument {
    pub id: String,
    pub kind: RecallKind,
    pub text: String,
    #[serde(default)]
    pub concept_ids: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallCandidate {
    pub id: String,
    pub kind: RecallKind,
    pub text: String,
    pub similarity: f64,
    pub recency: f64,
    pub frequency: f64,
    pub activation: f64,
    pub learned_score: f64,
    pub terms_matched: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingExample {
    pub query: String,
    pub candidate_id: String,
    pub used: bool,
    pub succeeded: bool,
    pub rung: u8,
}

/// An inspectable, bounded linear ranking policy fitted from persisted
/// retrieval outcomes. It is deliberately a search-policy artifact: its
/// inputs are retrieval features and prior use outcomes, never graph beliefs
/// or trust state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FittedRankingModel {
    /// Number of persisted examples that actually contributed a retrievable
    /// candidate feature vector.
    pub training_examples: u64,
    pub positive_examples: u64,
    /// A model with only one class has no useful fitted signal and falls back
    /// to the bounded baseline score.
    pub fitted: bool,
    pub intercept: f64,
    /// Feature names and their bounded fitted coefficients, kept public so a
    /// caller can inspect why local ranking changed.
    pub feature_weights: BTreeMap<String, f64>,
    pub feature_means: BTreeMap<String, f64>,
}

/// A time-split, query-conditioned evaluation of the ranker.  It compares
/// the learned ordering with the activation-only ordering on examples that
/// were withheld from the ranking evidence.  It is an evaluation of search
/// policy, never evidence that a retrieved claim is true.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankingEvaluation {
    pub id: i64,
    pub query: String,
    pub candidate_limit: usize,
    pub training_examples: u64,
    pub held_out_examples: u64,
    pub held_out_successes: u64,
    /// Successful held-out choices that were present in the bounded candidate
    /// pool and therefore could be scored by both policies.
    pub scored_successes: u64,
    pub baseline_mean_rank: Option<f64>,
    pub learned_mean_rank: Option<f64>,
    pub baseline_mean_reciprocal_rank: Option<f64>,
    pub learned_mean_reciprocal_rank: Option<f64>,
    /// True only when the held-out evidence shows a strictly lower average
    /// successful-candidate rank.  Missing or unscorable evidence is never a
    /// win.
    pub learned_improves_search: bool,
    pub created_at: i64,
}

/// A cross-query, time-split evaluation of local semantic candidate
/// generation. The newest query groups are withheld as complete groups, and
/// successful outcomes are used only to measure candidate-pool coverage.
/// This measures recall availability, never claim truth or ranking quality.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticRecallEvaluation {
    pub id: i64,
    pub candidate_limit: usize,
    pub training_queries: u64,
    pub held_out_queries: u64,
    pub held_out_successes: u64,
    pub lexical_scored_successes: u64,
    pub semantic_scored_successes: u64,
    /// True only when the semantic candidate pool covers strictly more
    /// successful held-out choices than lexical matching, with at least one
    /// disjoint training query group present.
    pub semantic_improves_recall: bool,
    pub created_at: i64,
}

/// An offline representation-training artifact. It is derived only from
/// immutable supervision rows and never mutates graph truth or trust state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepresentationModel {
    pub id: i64,
    pub model_version: String,
    pub training_tasks: u64,
    pub held_out_tasks: u64,
    pub held_out_coverage: f64,
    pub activated: bool,
    pub term_weights: BTreeMap<String, f64>,
    pub created_at: i64,
}

/// Offline evidence used before a representation/search policy may become
/// active.  This is deliberately separate from the model artifact: producing
/// a model is not permission to change retrieval behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepresentationRegressionEvaluation {
    pub id: i64,
    pub model_id: i64,
    pub held_out_queries: u64,
    pub held_out_successes: u64,
    pub baseline_scored_successes: u64,
    pub candidate_scored_successes: u64,
    pub baseline_mean_rank: Option<f64>,
    pub candidate_mean_rank: Option<f64>,
    /// True only when the candidate does not lose either held-out coverage or
    /// successful-candidate rank against the unmodified policy.
    pub preserves_search: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisionTask {
    pub id: i64,
    pub kind: String,
    pub input: serde_json::Value,
    pub target: serde_json::Value,
    pub grounded: bool,
    pub source_episode: Option<String>,
    /// A challenge can be generated without being completed. `grounded` is
    /// counted as actual grounding only after this becomes true following a
    /// persisted verifier outcome.
    #[serde(default)]
    pub completed: bool,
    /// The bounded executor or verifier outcome. This is evidence about the
    /// challenge only; it is never a graph fact or a trust receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicChallengeKind {
    HiddenComputation,
    InverseRoundTrip,
    ContractBoundary,
    ConsequencePrediction,
}

impl EpistemicChallengeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::HiddenComputation => "hidden_computation",
            Self::InverseRoundTrip => "inverse_round_trip",
            Self::ContractBoundary => "contract_boundary",
            Self::ConsequencePrediction => "consequence_prediction",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntuitionMetrics {
    pub indexed_documents: u64,
    pub inverted_term_rows: u64,
    pub retrieval_queries: u64,
    pub candidates_examined: u64,
    pub ranking_examples: u64,
    pub ranking_evaluations: u64,
    pub ranking_search_wins: u64,
    pub semantic_recall_evaluations: u64,
    pub semantic_recall_wins: u64,
    /// Every persisted supervision task, including ones that have not yet
    /// reached a terminating verifier.
    pub generated_tasks: u64,
    /// Tasks for which a bounded executor or verifier recorded an outcome.
    pub completed_tasks: u64,
    pub supervision_tasks: u64,
    pub grounded_tasks: u64,
    /// The observed share of supervision tasks that have an external
    /// grounder. Exposing the derived value prevents consumers from silently
    /// treating a raw grounded-task count as a health metric.
    pub grounding_ratio: f64,
}

pub struct IntuitionStore {
    conn: Connection,
}

impl IntuitionStore {
    pub fn open(path: &str) -> Result<Self, IntuitionError> {
        let store = Self {
            conn: Connection::open(path)?,
        };
        store.create_schema()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, IntuitionError> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.create_schema()?;
        Ok(store)
    }

    fn create_schema(&self) -> Result<(), IntuitionError> {
        self.conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS recall_documents (
                 id TEXT PRIMARY KEY,
                 kind TEXT NOT NULL,
                 text TEXT NOT NULL,
                 vector_json TEXT NOT NULL,
                 concept_ids_json TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 retrieval_count INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS recall_terms (
                 term TEXT NOT NULL,
                 document_id TEXT NOT NULL,
                 PRIMARY KEY(term, document_id),
                 FOREIGN KEY(document_id) REFERENCES recall_documents(id) ON DELETE CASCADE
             );
             CREATE INDEX IF NOT EXISTS idx_recall_terms_term ON recall_terms(term);
             CREATE TABLE IF NOT EXISTS recall_semantic_terms (
                 term TEXT NOT NULL,
                 document_id TEXT NOT NULL,
                 PRIMARY KEY(term, document_id),
                 FOREIGN KEY(document_id) REFERENCES recall_documents(id) ON DELETE CASCADE
             );
             CREATE INDEX IF NOT EXISTS idx_recall_semantic_terms_term
                 ON recall_semantic_terms(term);
             CREATE TABLE IF NOT EXISTS recall_ranking_examples (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 query TEXT NOT NULL,
                 candidate_id TEXT NOT NULL,
                 used INTEGER NOT NULL,
                 succeeded INTEGER NOT NULL,
                 rung INTEGER NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_recall_ranking_query_candidate
                 ON recall_ranking_examples(query, candidate_id);
             CREATE TABLE IF NOT EXISTS recall_ranking_evaluations (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 query TEXT NOT NULL,
                 candidate_limit INTEGER NOT NULL,
                 training_examples INTEGER NOT NULL,
                 held_out_examples INTEGER NOT NULL,
                 held_out_successes INTEGER NOT NULL,
                 scored_successes INTEGER NOT NULL,
                 baseline_mean_rank REAL,
                 learned_mean_rank REAL,
                 baseline_mean_reciprocal_rank REAL,
                 learned_mean_reciprocal_rank REAL,
                 learned_improves_search INTEGER NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_recall_ranking_evaluations_query
                 ON recall_ranking_evaluations(query, id DESC);
             CREATE TABLE IF NOT EXISTS recall_semantic_evaluations (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 candidate_limit INTEGER NOT NULL,
                 training_queries INTEGER NOT NULL,
                 held_out_queries INTEGER NOT NULL,
                 held_out_successes INTEGER NOT NULL,
                 lexical_scored_successes INTEGER NOT NULL,
                 semantic_scored_successes INTEGER NOT NULL,
                 semantic_improves_recall INTEGER NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_recall_semantic_evaluations_created
                 ON recall_semantic_evaluations(created_at DESC);
             CREATE TABLE IF NOT EXISTS supervision_tasks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 kind TEXT NOT NULL,
                 input_json TEXT NOT NULL,
                 target_json TEXT NOT NULL,
                 grounded INTEGER NOT NULL,
                 source_episode TEXT,
                 completed INTEGER NOT NULL DEFAULT 0,
                 outcome_json TEXT,
                 verifier TEXT,
                 completed_at INTEGER,
                 created_at INTEGER NOT NULL
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_supervision_verified_trace_source
                 ON supervision_tasks(source_episode, kind)
                 WHERE kind = 'verified_trace_replay';
             CREATE TABLE IF NOT EXISTS representation_models (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 model_version TEXT NOT NULL,
                 training_tasks INTEGER NOT NULL,
                 held_out_tasks INTEGER NOT NULL,
                 held_out_coverage REAL NOT NULL,
                 activated INTEGER NOT NULL DEFAULT 0,
                 term_weights_json TEXT NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_representation_models_created
                 ON representation_models(created_at DESC);
             CREATE TABLE IF NOT EXISTS representation_regression_evaluations (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 model_id INTEGER NOT NULL,
                 held_out_queries INTEGER NOT NULL,
                 held_out_successes INTEGER NOT NULL,
                 baseline_scored_successes INTEGER NOT NULL,
                 candidate_scored_successes INTEGER NOT NULL,
                 baseline_mean_rank REAL,
                 candidate_mean_rank REAL,
                 preserves_search INTEGER NOT NULL,
                 created_at INTEGER NOT NULL,
                 FOREIGN KEY(model_id) REFERENCES representation_models(id)
             );
             CREATE INDEX IF NOT EXISTS idx_representation_regressions_model
                 ON representation_regression_evaluations(model_id, id DESC);
             CREATE TABLE IF NOT EXISTS intuition_stats (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 retrieval_queries INTEGER NOT NULL DEFAULT 0,
                 candidates_examined INTEGER NOT NULL DEFAULT 0
             );",
        )?;
        self.ensure_supervision_task_columns()?;
        Ok(())
    }

    /// Add fields introduced after the original Phase 3 schema without
    /// rewriting historical task rows. Historical caller-declared grounding
    /// remains incomplete until a verifier outcome is recorded, so it cannot
    /// inflate the actual-grounding metric.
    fn ensure_supervision_task_columns(&self) -> Result<(), IntuitionError> {
        let mut statement = self.conn.prepare("PRAGMA table_info(supervision_tasks)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<BTreeSet<_>, _>>()?;
        for (column, definition) in [
            ("completed", "INTEGER NOT NULL DEFAULT 0"),
            ("outcome_json", "TEXT"),
            ("verifier", "TEXT"),
            ("completed_at", "INTEGER"),
        ] {
            if !columns.contains(column) {
                self.conn.execute(
                    &format!("ALTER TABLE supervision_tasks ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }
        self.conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_supervision_verified_trace_source
             ON supervision_tasks(source_episode, kind)
             WHERE kind = 'verified_trace_replay'",
            [],
        )?;
        Ok(())
    }

    /// Upsert a representation document and its inverted-token postings.
    /// Re-indexing changes only retrieval data, never graph or belief state.
    pub fn index_document(&self, document: &RecallDocument) -> Result<(), IntuitionError> {
        if document.id.trim().is_empty() || document.text.trim().is_empty() {
            return Err(IntuitionError::Invalid(
                "recall documents need non-empty id and text".into(),
            ));
        }
        let vector = embed(&document.text);
        let terms = tokenize(&document.text);
        let semantic_terms = semantic_tokenize(&document.text);
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO recall_documents
                (id, kind, text, vector_json, concept_ids_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                kind = excluded.kind,
                text = excluded.text,
                vector_json = excluded.vector_json,
                concept_ids_json = excluded.concept_ids_json,
                created_at = excluded.created_at",
            params![
                document.id,
                document.kind.as_str(),
                document.text,
                serde_json::to_string(&vector)?,
                serde_json::to_string(&document.concept_ids)?,
                document.created_at,
            ],
        )?;
        transaction.execute(
            "DELETE FROM recall_terms WHERE document_id = ?1",
            params![document.id],
        )?;
        for term in terms {
            transaction.execute(
                "INSERT OR IGNORE INTO recall_terms(term, document_id) VALUES (?1, ?2)",
                params![term, document.id],
            )?;
        }
        transaction.execute(
            "DELETE FROM recall_semantic_terms WHERE document_id = ?1",
            params![document.id],
        )?;
        for term in semantic_terms {
            transaction.execute(
                "INSERT OR IGNORE INTO recall_semantic_terms(term, document_id) VALUES (?1, ?2)",
                params![term, document.id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn remove_document(&self, id: &str) -> Result<(), IntuitionError> {
        self.conn
            .execute("DELETE FROM recall_documents WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn remove_documents_with_prefix(&self, prefix: &str) -> Result<(), IntuitionError> {
        self.conn.execute(
            "DELETE FROM recall_documents WHERE id LIKE ?1 || '%'",
            params![prefix],
        )?;
        Ok(())
    }

    pub fn clear_documents(&self) -> Result<(), IntuitionError> {
        self.conn.execute("DELETE FROM recall_documents", [])?;
        Ok(())
    }

    /// Candidate generation is inverted-index bounded: only documents
    /// sharing at least one query term are examined, and the candidate pool is
    /// capped before similarity/ranking is calculated.
    pub fn retrieve(
        &self,
        query: &str,
        candidate_limit: usize,
    ) -> Result<Vec<RecallCandidate>, IntuitionError> {
        self.candidates_for_query(query, candidate_limit, true)
    }

    fn candidates_for_query(
        &self,
        query: &str,
        candidate_limit: usize,
        track_retrieval: bool,
    ) -> Result<Vec<RecallCandidate>, IntuitionError> {
        self.candidates_for_query_mode(query, candidate_limit, track_retrieval, true)
    }

    fn candidates_for_query_mode(
        &self,
        query: &str,
        candidate_limit: usize,
        track_retrieval: bool,
        include_semantic_expansion: bool,
    ) -> Result<Vec<RecallCandidate>, IntuitionError> {
        if candidate_limit == 0 || candidate_limit > 1_024 {
            return Err(IntuitionError::Invalid(
                "candidate limit must be 1..=1024".into(),
            ));
        }
        let semantic_features = if include_semantic_expansion {
            self.semantic_features(query)?
        } else {
            direct_semantic_features(query)
        };
        self.candidates_for_query_with_features(
            query,
            candidate_limit,
            track_retrieval,
            semantic_features,
        )
    }

    fn candidates_for_query_with_features(
        &self,
        _query: &str,
        candidate_limit: usize,
        track_retrieval: bool,
        semantic_features: BTreeMap<String, f64>,
    ) -> Result<Vec<RecallCandidate>, IntuitionError> {
        if semantic_features.is_empty() {
            return Ok(Vec::new());
        }
        let mut terms = BTreeSet::new();
        for term in semantic_features.keys() {
            terms.extend(tokenize(term));
        }
        let terms_json = terms
            .iter()
            .map(|term| format!("'{term}'"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT d.id, d.kind, d.text, d.created_at,
                    d.retrieval_count, COUNT(DISTINCT t.term)
             FROM recall_terms t
             JOIN recall_documents d ON d.id = t.document_id
             WHERE t.term IN ({terms_json})
             GROUP BY d.id
             ORDER BY COUNT(DISTINCT t.term) DESC, d.retrieval_count DESC, d.created_at DESC
             LIMIT ?1"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params![candidate_limit as i64], |row| {
            let kind: String = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                parse_kind(&kind),
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;
        let now = unix_time();
        let mut candidates = Vec::new();
        for row in rows {
            let (id, kind, text, created_at, retrieval_count, matched) = row?;
            let similarity = semantic_similarity(&semantic_features, &semantic_tokenize(&text));
            let age = now.saturating_sub(created_at).max(0) as f64;
            let recency = 1.0 / (1.0 + age / 86_400.0);
            let frequency = (retrieval_count.max(0) as f64 + 1.0).ln_1p();
            candidates.push(RecallCandidate {
                id,
                kind,
                text,
                similarity,
                recency,
                frequency,
                activation: similarity * 0.7 + recency * 0.2 + frequency * 0.1,
                learned_score: 0.0,
                terms_matched: matched as usize,
            });
        }
        candidates.sort_by(compare_activation);
        if track_retrieval {
            for candidate in &candidates {
                self.conn.execute(
                    "UPDATE recall_documents SET retrieval_count = retrieval_count + 1 WHERE id = ?1",
                    params![candidate.id],
                )?;
            }
            self.conn.execute(
                "INSERT INTO intuition_stats(id, retrieval_queries, candidates_examined)
                 VALUES (1, 1, ?1)
                 ON CONFLICT(id) DO UPDATE SET
                     retrieval_queries = intuition_stats.retrieval_queries + 1,
                     candidates_examined = intuition_stats.candidates_examined + excluded.candidates_examined",
                params![candidates.len() as i64],
            )?;
        }
        Ok(candidates)
    }

    /// Online ranker trained only on retrieval/use outcomes. It changes
    /// ordering, not knowledge truth or lifecycle.
    pub fn rank(
        &self,
        query: &str,
        candidate_limit: usize,
    ) -> Result<Vec<RecallCandidate>, IntuitionError> {
        let mut candidates = self.candidates_for_query(query, candidate_limit, true)?;
        let model = self.fitted_ranking_model_before(i64::MAX)?;
        for candidate in &mut candidates {
            let outcome = self.candidate_success_rate(query, &candidate.id)?;
            let features = ranking_features(candidate, outcome);
            candidate.learned_score = score_candidate(&model, features);
        }
        candidates.sort_by(compare_learned_score);
        Ok(candidates)
    }

    /// Returns the deterministic, bounded ranker fitted from the persisted
    /// outcome rows currently available locally. This is intentionally an
    /// inspection API rather than a promotion or belief-update mechanism.
    pub fn fitted_ranking_model(&self) -> Result<FittedRankingModel, IntuitionError> {
        self.fitted_ranking_model_before(i64::MAX)
    }

    pub fn record_ranking_example(&self, example: &RankingExample) -> Result<(), IntuitionError> {
        let query = canonical_query(&example.query)?;
        if example.candidate_id.trim().is_empty() {
            return Err(IntuitionError::Invalid(
                "ranking examples need a candidate id".into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO recall_ranking_examples
                (query, candidate_id, used, succeeded, rung, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                query,
                example.candidate_id,
                i64::from(example.used),
                i64::from(example.succeeded),
                i64::from(example.rung),
                unix_time(),
            ],
        )?;
        Ok(())
    }

    /// Evaluates the query-specific ranking policy using a chronological
    /// holdout.  The newest `holdout_examples` outcomes are never consulted
    /// when scoring candidates, so this cannot report a training-set win as a
    /// search improvement.  Evaluation is bounded by both the candidate pool
    /// and a small fixed holdout limit, and does not mutate retrieval counts.
    pub fn evaluate_ranking(
        &self,
        query: &str,
        candidate_limit: usize,
        holdout_examples: usize,
    ) -> Result<RankingEvaluation, IntuitionError> {
        if candidate_limit == 0 || candidate_limit > 1_024 {
            return Err(IntuitionError::Invalid(
                "candidate limit must be 1..=1024".into(),
            ));
        }
        if holdout_examples == 0 || holdout_examples > MAX_RANKING_EVALUATION_HOLDOUT {
            return Err(IntuitionError::Invalid(format!(
                "ranking evaluation holdout must be 1..={MAX_RANKING_EVALUATION_HOLDOUT}"
            )));
        }
        let query = canonical_query(query)?;
        let mut statement = self.conn.prepare(
            "SELECT id, candidate_id, used, succeeded
             FROM recall_ranking_examples
             WHERE query = ?1
             ORDER BY id DESC
             LIMIT ?2",
        )?;
        let mut held_out = statement
            .query_map(params![query, holdout_examples as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, i64>(3)? != 0,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        held_out.reverse();
        let first_held_out_id = held_out.first().map(|example| example.0);
        let training_examples = match first_held_out_id {
            Some(cutoff) => self.conn.query_row(
                "SELECT COUNT(*) FROM recall_ranking_examples
                 WHERE query = ?1 AND id < ?2",
                params![query, cutoff],
                |row| row.get::<_, i64>(0),
            )? as u64,
            None => 0,
        };
        let candidates = self.candidates_for_query(&query, candidate_limit, false)?;
        let mut baseline = candidates.clone();
        baseline.sort_by(compare_activation);
        let mut learned = candidates;
        let cutoff = first_held_out_id.unwrap_or(i64::MAX);
        // The ranker is fitted with exactly the rows before the chronological
        // cutoff. The held-out rows below are therefore only scored, never
        // used to fit a coefficient or candidate-success feature.
        let model = self.fitted_ranking_model_before(cutoff)?;
        for candidate in &mut learned {
            let outcome = self.candidate_success_rate_before(&query, &candidate.id, cutoff)?;
            let features = ranking_features(candidate, outcome);
            candidate.learned_score = score_candidate(&model, features);
        }
        learned.sort_by(compare_learned_score);

        let held_out_successes = held_out
            .iter()
            .filter(|(_, _, used, succeeded)| *used && *succeeded)
            .count() as u64;
        let mut scored_successes = 0u64;
        let mut baseline_rank_total = 0u64;
        let mut learned_rank_total = 0u64;
        let mut baseline_reciprocal_rank_total = 0.0;
        let mut learned_reciprocal_rank_total = 0.0;
        for (_, candidate_id, used, succeeded) in &held_out {
            if !used || !succeeded {
                continue;
            }
            let Some(baseline_rank) = rank_of(&baseline, candidate_id) else {
                continue;
            };
            let Some(learned_rank) = rank_of(&learned, candidate_id) else {
                continue;
            };
            scored_successes += 1;
            baseline_rank_total += baseline_rank as u64;
            learned_rank_total += learned_rank as u64;
            baseline_reciprocal_rank_total += 1.0 / baseline_rank as f64;
            learned_reciprocal_rank_total += 1.0 / learned_rank as f64;
        }
        let denominator = scored_successes as f64;
        let baseline_mean_rank =
            (scored_successes > 0).then(|| baseline_rank_total as f64 / denominator);
        let learned_mean_rank =
            (scored_successes > 0).then(|| learned_rank_total as f64 / denominator);
        let baseline_mean_reciprocal_rank =
            (scored_successes > 0).then(|| baseline_reciprocal_rank_total / denominator);
        let learned_mean_reciprocal_rank =
            (scored_successes > 0).then(|| learned_reciprocal_rank_total / denominator);
        let learned_improves_search = training_examples > 0
            && learned_mean_rank
                .zip(baseline_mean_rank)
                .is_some_and(|(learned, baseline)| learned < baseline);
        let created_at = unix_time();
        self.conn.execute(
            "INSERT INTO recall_ranking_evaluations
                (query, candidate_limit, training_examples, held_out_examples,
                 held_out_successes, scored_successes, baseline_mean_rank,
                 learned_mean_rank, baseline_mean_reciprocal_rank,
                 learned_mean_reciprocal_rank, learned_improves_search, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                query,
                candidate_limit as i64,
                training_examples as i64,
                held_out.len() as i64,
                held_out_successes as i64,
                scored_successes as i64,
                baseline_mean_rank,
                learned_mean_rank,
                baseline_mean_reciprocal_rank,
                learned_mean_reciprocal_rank,
                i64::from(learned_improves_search),
                created_at,
            ],
        )?;
        Ok(RankingEvaluation {
            id: self.conn.last_insert_rowid(),
            query,
            candidate_limit,
            training_examples,
            held_out_examples: held_out.len() as u64,
            held_out_successes,
            scored_successes,
            baseline_mean_rank,
            learned_mean_rank,
            baseline_mean_reciprocal_rank,
            learned_mean_reciprocal_rank,
            learned_improves_search,
            created_at,
        })
    }

    pub fn latest_ranking_evaluation(
        &self,
        query: &str,
    ) -> Result<Option<RankingEvaluation>, IntuitionError> {
        let query = canonical_query(query)?;
        self.conn
            .query_row(
                "SELECT id, query, candidate_limit, training_examples,
                        held_out_examples, held_out_successes, scored_successes,
                        baseline_mean_rank, learned_mean_rank,
                        baseline_mean_reciprocal_rank,
                        learned_mean_reciprocal_rank, learned_improves_search,
                        created_at
                 FROM recall_ranking_evaluations
                 WHERE query = ?1
                 ORDER BY id DESC LIMIT 1",
                params![query],
                row_to_ranking_evaluation,
            )
            .optional()
            .map_err(IntuitionError::from)
    }

    /// Measures lexical and semantic candidate-pool coverage across complete,
    /// chronologically held-out query groups. The local co-occurrence
    /// representation is derived from indexed documents, never from these
    /// outcome rows, and the inspected success count is hard bounded.
    pub fn evaluate_semantic_recall(
        &self,
        candidate_limit: usize,
        holdout_queries: usize,
    ) -> Result<SemanticRecallEvaluation, IntuitionError> {
        if candidate_limit == 0 || candidate_limit > 1_024 {
            return Err(IntuitionError::Invalid(
                "candidate limit must be 1..=1024".into(),
            ));
        }
        if holdout_queries == 0 || holdout_queries > MAX_RANKING_EVALUATION_HOLDOUT {
            return Err(IntuitionError::Invalid(format!(
                "semantic evaluation holdout must be 1..={MAX_RANKING_EVALUATION_HOLDOUT}"
            )));
        }
        let mut statement = self.conn.prepare(
            "SELECT query FROM recall_ranking_examples
             GROUP BY query
             ORDER BY MAX(id) DESC, query ASC
             LIMIT ?1",
        )?;
        let mut held_out_queries = statement
            .query_map(params![holdout_queries as i64], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if held_out_queries.is_empty() {
            return Err(IntuitionError::Invalid(
                "semantic evaluation requires ranking examples".into(),
            ));
        }
        held_out_queries.sort();
        let held_out_sql = quoted_terms(&held_out_queries);
        let training_queries = self.conn.query_row(
            &format!(
                "SELECT COUNT(DISTINCT query) FROM recall_ranking_examples
                 WHERE query NOT IN ({held_out_sql})"
            ),
            [],
            |row| row.get::<_, i64>(0),
        )? as u64;
        let mut successes = self.conn.prepare(&format!(
            "SELECT query, candidate_id FROM recall_ranking_examples
             WHERE query IN ({held_out_sql}) AND used = 1 AND succeeded = 1
             ORDER BY id DESC LIMIT {MAX_SEMANTIC_EVALUATION_EXAMPLES}"
        ))?;
        let successes = successes
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut pools = BTreeMap::new();
        for query in &held_out_queries {
            let lexical = self
                .candidates_for_query_mode(query, candidate_limit, false, false)?
                .into_iter()
                .map(|candidate| candidate.id)
                .collect::<BTreeSet<_>>();
            let semantic = self
                .candidates_for_query_mode(query, candidate_limit, false, true)?
                .into_iter()
                .map(|candidate| candidate.id)
                .collect::<BTreeSet<_>>();
            pools.insert(query.clone(), (lexical, semantic));
        }
        let held_out_successes = successes.len() as u64;
        let mut lexical_scored_successes = 0u64;
        let mut semantic_scored_successes = 0u64;
        for (query, candidate_id) in successes {
            let Some((lexical, semantic)) = pools.get(&query) else {
                continue;
            };
            lexical_scored_successes += u64::from(lexical.contains(&candidate_id));
            semantic_scored_successes += u64::from(semantic.contains(&candidate_id));
        }
        let semantic_improves_recall =
            training_queries > 0 && semantic_scored_successes > lexical_scored_successes;
        let created_at = unix_time();
        self.conn.execute(
            "INSERT INTO recall_semantic_evaluations
                (candidate_limit, training_queries, held_out_queries,
                 held_out_successes, lexical_scored_successes,
                 semantic_scored_successes, semantic_improves_recall, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                candidate_limit as i64,
                training_queries as i64,
                held_out_queries.len() as i64,
                held_out_successes as i64,
                lexical_scored_successes as i64,
                semantic_scored_successes as i64,
                i64::from(semantic_improves_recall),
                created_at,
            ],
        )?;
        Ok(SemanticRecallEvaluation {
            id: self.conn.last_insert_rowid(),
            candidate_limit,
            training_queries,
            held_out_queries: held_out_queries.len() as u64,
            held_out_successes,
            lexical_scored_successes,
            semantic_scored_successes,
            semantic_improves_recall,
            created_at,
        })
    }

    /// Train a bounded, immutable representation artifact from supervision
    /// data. The simple term model is intentionally separate from retrieval
    /// activation: callers must explicitly opt into an artifact after
    /// inspecting its held-out coverage.
    pub fn train_representation_model(
        &self,
        holdout_tasks: usize,
    ) -> Result<RepresentationModel, IntuitionError> {
        if holdout_tasks == 0 || holdout_tasks > MAX_REPRESENTATION_TRAINING_HOLDOUT {
            return Err(IntuitionError::Invalid(format!(
                "representation holdout must be 1..={MAX_REPRESENTATION_TRAINING_HOLDOUT}"
            )));
        }
        let mut statement = self
            .conn
            .prepare("SELECT id, input_json FROM supervision_tasks ORDER BY id DESC LIMIT ?1")?;
        let mut tasks = statement
            .query_map(params![((holdout_tasks + 4096) as i64)], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if tasks.is_empty() {
            return Err(IntuitionError::Invalid(
                "representation training requires supervision tasks".into(),
            ));
        }
        tasks.sort_by_key(|(id, _)| *id);
        let split = tasks
            .len()
            .saturating_sub(holdout_tasks)
            .max(1)
            .min(tasks.len());
        let (training, held_out) = tasks.split_at(split);
        let mut counts = BTreeMap::<String, u64>::new();
        for (_, input) in training {
            for term in tokenize(input) {
                *counts.entry(term).or_default() += 1;
            }
        }
        let total = training.len().max(1) as f64;
        let term_weights = counts
            .into_iter()
            .map(|(term, count)| (term, count as f64 / total))
            .collect::<BTreeMap<_, _>>();
        let covered = held_out
            .iter()
            .filter(|(_, input)| {
                tokenize(input)
                    .iter()
                    .any(|term| term_weights.contains_key(term))
            })
            .count();
        let held_out_coverage = if held_out.is_empty() {
            0.0
        } else {
            covered as f64 / held_out.len() as f64
        };
        let mut hasher = DefaultHasher::new();
        training.hash(&mut hasher);
        for (term, weight) in &term_weights {
            term.hash(&mut hasher);
            weight.to_bits().hash(&mut hasher);
        }
        let model_version = format!("representation-v1-{:016x}", hasher.finish());
        let created_at = unix_time();
        self.conn.execute(
            "INSERT INTO representation_models
             (model_version, training_tasks, held_out_tasks, held_out_coverage,
              activated, term_weights_json, created_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
            params![
                model_version,
                training.len() as i64,
                held_out.len() as i64,
                held_out_coverage,
                serde_json::to_string(&term_weights)?,
                created_at,
            ],
        )?;
        self.latest_representation_model()?.ok_or_else(|| {
            IntuitionError::Invalid("representation model insert was not readable".into())
        })
    }

    pub fn activate_representation_model(
        &self,
        id: i64,
    ) -> Result<RepresentationModel, IntuitionError> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM representation_models WHERE id = ?1)",
            params![id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(IntuitionError::Invalid(
                "representation model not found".into(),
            ));
        }
        let Some(regression) = self.latest_representation_regression(id)? else {
            return Err(IntuitionError::Invalid(
                "representation model requires an offline held-out regression evaluation before activation".into(),
            ));
        };
        if !regression.preserves_search {
            return Err(IntuitionError::Invalid(format!(
                "representation model failed held-out search regression (evaluation {})",
                regression.id
            )));
        }
        self.conn
            .execute("UPDATE representation_models SET activated = 0", [])?;
        self.conn.execute(
            "UPDATE representation_models SET activated = 1 WHERE id = ?1",
            params![id],
        )?;
        self.representation_model(id)?.ok_or_else(|| {
            IntuitionError::Invalid("representation model activation was not readable".into())
        })
    }

    /// Compare a proposed representation artifact with the currently active
    /// policy on a chronological held-out suffix of successful retrieval
    /// outcomes. This is intentionally offline and bounded; activation is
    /// refused unless this evidence has been persisted and shows no loss.
    pub fn evaluate_representation_model(
        &self,
        model_id: i64,
        holdout_queries: usize,
    ) -> Result<RepresentationRegressionEvaluation, IntuitionError> {
        if holdout_queries == 0 || holdout_queries > MAX_RANKING_EVALUATION_HOLDOUT {
            return Err(IntuitionError::Invalid(format!(
                "representation regression holdout must be 1..={MAX_RANKING_EVALUATION_HOLDOUT}"
            )));
        }
        let model = self
            .representation_model(model_id)?
            .ok_or_else(|| IntuitionError::Invalid("representation model not found".into()))?;
        let mut statement = self.conn.prepare(
            "SELECT query FROM recall_ranking_examples
             GROUP BY query ORDER BY MAX(id) DESC, query ASC LIMIT ?1",
        )?;
        let mut queries = statement
            .query_map(params![holdout_queries as i64], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if queries.is_empty() {
            return Err(IntuitionError::Invalid(
                "representation regression requires ranking examples".into(),
            ));
        }
        queries.sort();
        let query_set = queries
            .iter()
            .map(|query| format!("'{}'", query.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(",");
        let mut successes = self.conn.prepare(&format!(
            "SELECT query, candidate_id FROM recall_ranking_examples
             WHERE query IN ({query_set}) AND used = 1 AND succeeded = 1
             ORDER BY id DESC LIMIT {MAX_SEMANTIC_EVALUATION_EXAMPLES}"
        ))?;
        let successes = successes
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut baseline_scored_successes = 0u64;
        let mut candidate_scored_successes = 0u64;
        let mut baseline_rank_total = 0u64;
        let mut candidate_rank_total = 0u64;
        for (query, candidate_id) in &successes {
            let mut baseline_features = self.unmodified_semantic_features(query)?;
            self.apply_active_representation_model(&mut baseline_features)?;
            let mut candidate_features = self.unmodified_semantic_features(query)?;
            self.apply_representation_model(&mut candidate_features, &model);
            let baseline =
                self.candidates_for_query_with_features(query, 128, false, baseline_features)?;
            let candidate =
                self.candidates_for_query_with_features(query, 128, false, candidate_features)?;
            if let Some(rank) = baseline.iter().position(|item| &item.id == candidate_id) {
                baseline_scored_successes += 1;
                baseline_rank_total += rank as u64 + 1;
            }
            if let Some(rank) = candidate.iter().position(|item| &item.id == candidate_id) {
                candidate_scored_successes += 1;
                candidate_rank_total += rank as u64 + 1;
            }
        }
        let baseline_mean_rank = (baseline_scored_successes > 0)
            .then(|| baseline_rank_total as f64 / baseline_scored_successes as f64);
        let candidate_mean_rank = (candidate_scored_successes > 0)
            .then(|| candidate_rank_total as f64 / candidate_scored_successes as f64);
        let preserves_search = candidate_scored_successes >= baseline_scored_successes
            && candidate_mean_rank
                .zip(baseline_mean_rank)
                .is_some_and(|(candidate, baseline)| candidate <= baseline)
            && candidate_scored_successes > 0;
        let created_at = unix_time();
        self.conn.execute(
            "INSERT INTO representation_regression_evaluations
             (model_id, held_out_queries, held_out_successes,
              baseline_scored_successes, candidate_scored_successes,
              baseline_mean_rank, candidate_mean_rank, preserves_search, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                model_id,
                queries.len() as i64,
                successes.len() as i64,
                baseline_scored_successes as i64,
                candidate_scored_successes as i64,
                baseline_mean_rank,
                candidate_mean_rank,
                i64::from(preserves_search),
                created_at,
            ],
        )?;
        self.latest_representation_regression(model_id)?
            .ok_or_else(|| {
                IntuitionError::Invalid("representation regression insert was not readable".into())
            })
    }

    pub fn latest_representation_regression(
        &self,
        model_id: i64,
    ) -> Result<Option<RepresentationRegressionEvaluation>, IntuitionError> {
        self.conn
            .query_row(
                "SELECT id, model_id, held_out_queries, held_out_successes,
                        baseline_scored_successes, candidate_scored_successes,
                        baseline_mean_rank, candidate_mean_rank, preserves_search,
                        created_at
                 FROM representation_regression_evaluations
                 WHERE model_id = ?1 ORDER BY id DESC LIMIT 1",
                params![model_id],
                row_to_representation_regression,
            )
            .optional()
            .map_err(IntuitionError::from)
    }

    pub fn latest_representation_model(
        &self,
    ) -> Result<Option<RepresentationModel>, IntuitionError> {
        self.conn
            .query_row(
                "SELECT id, model_version, training_tasks, held_out_tasks,
                        held_out_coverage, activated, term_weights_json, created_at
                 FROM representation_models ORDER BY id DESC LIMIT 1",
                [],
                row_to_representation_model,
            )
            .optional()
            .map_err(IntuitionError::from)
    }

    fn representation_model(&self, id: i64) -> Result<Option<RepresentationModel>, IntuitionError> {
        self.conn
            .query_row(
                "SELECT id, model_version, training_tasks, held_out_tasks,
                        held_out_coverage, activated, term_weights_json, created_at
                 FROM representation_models WHERE id = ?1",
                params![id],
                row_to_representation_model,
            )
            .optional()
            .map_err(IntuitionError::from)
    }

    pub fn generate_self_supervision(
        &self,
        source_episode: Option<&str>,
        input: serde_json::Value,
        target: serde_json::Value,
        kind: &str,
        grounded: bool,
    ) -> Result<SupervisionTask, IntuitionError> {
        if kind.trim().is_empty() {
            return Err(IntuitionError::Invalid(
                "supervision task kind is required".into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO supervision_tasks
                (kind, input_json, target_json, grounded, source_episode,
                 completed, outcome_json, verifier, completed_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL, NULL, NULL, ?6)",
            params![
                kind,
                serde_json::to_string(&input)?,
                serde_json::to_string(&target)?,
                i64::from(grounded),
                source_episode,
                unix_time(),
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(SupervisionTask {
            id,
            kind: kind.into(),
            input,
            target,
            grounded,
            source_episode: source_episode.map(str::to_owned),
            completed: false,
            outcome: None,
            verifier: None,
            completed_at: None,
        })
    }

    /// Persists a challenge derived from a trusted execution before it is
    /// replayed. The source can produce at most one such challenge and the
    /// store has a small durable budget, which prevents repeated callers from
    /// turning the same evidence into unlimited supervision.
    pub fn begin_verified_trace_replay(
        &self,
        source_episode: &str,
        input: serde_json::Value,
        target: serde_json::Value,
    ) -> Result<SupervisionTask, IntuitionError> {
        if source_episode.trim().is_empty() {
            return Err(IntuitionError::Invalid(
                "verified trace replay requires a source episode".into(),
            ));
        }
        let transaction = self.conn.unchecked_transaction()?;
        let generated: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM supervision_tasks WHERE kind = ?1",
            params![VERIFIED_TRACE_REPLAY_KIND],
            |row| row.get(0),
        )?;
        if generated as u64 >= MAX_AUTO_GROUNDED_SUPERVISION_TASKS {
            return Err(IntuitionError::Invalid(format!(
                "verified trace replay budget exhausted ({MAX_AUTO_GROUNDED_SUPERVISION_TASKS} tasks)"
            )));
        }
        let existing: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM supervision_tasks
                WHERE source_episode = ?1 AND kind = ?2
            )",
            params![source_episode, VERIFIED_TRACE_REPLAY_KIND],
            |row| row.get(0),
        )?;
        if existing {
            return Err(IntuitionError::Invalid(
                "a verified trace replay challenge already exists for this source episode".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO supervision_tasks
                (kind, input_json, target_json, grounded, source_episode,
                 completed, outcome_json, verifier, completed_at, created_at)
             VALUES (?1, ?2, ?3, 0, ?4, 0, NULL, NULL, NULL, ?5)",
            params![
                VERIFIED_TRACE_REPLAY_KIND,
                serde_json::to_string(&input)?,
                serde_json::to_string(&target)?,
                source_episode,
                unix_time(),
            ],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(SupervisionTask {
            id,
            kind: VERIFIED_TRACE_REPLAY_KIND.into(),
            input,
            target,
            grounded: false,
            source_episode: Some(source_episode.to_owned()),
            completed: false,
            outcome: None,
            verifier: None,
            completed_at: None,
        })
    }

    /// Records the immutable result of a local verifier. Only this method
    /// transitions an automatically-derived replay task into the grounded
    /// state; callers cannot create a completed grounded task by setting a
    /// boolean at generation time.
    pub fn complete_verified_trace_replay(
        &self,
        id: i64,
        grounded: bool,
        verifier: &str,
        outcome: serde_json::Value,
    ) -> Result<SupervisionTask, IntuitionError> {
        if verifier.trim().is_empty() {
            return Err(IntuitionError::Invalid(
                "verified trace replay requires a verifier identity".into(),
            ));
        }
        let completed_at = unix_time();
        let changed = self.conn.execute(
            "UPDATE supervision_tasks
             SET grounded = ?1, completed = 1, outcome_json = ?2,
                 verifier = ?3, completed_at = ?4
             WHERE id = ?5 AND kind = ?6 AND completed = 0",
            params![
                i64::from(grounded),
                serde_json::to_string(&outcome)?,
                verifier,
                completed_at,
                id,
                VERIFIED_TRACE_REPLAY_KIND,
            ],
        )?;
        if changed != 1 {
            return Err(IntuitionError::Invalid(
                "verified trace replay task is missing or already completed".into(),
            ));
        }
        self.supervision_task(id)?.ok_or_else(|| {
            IntuitionError::Invalid("completed verified trace replay was not readable".into())
        })
    }

    fn supervision_task(&self, id: i64) -> Result<Option<SupervisionTask>, IntuitionError> {
        self.conn
            .query_row(
                "SELECT id, kind, input_json, target_json, grounded, source_episode,
                        completed, outcome_json, verifier, completed_at
                 FROM supervision_tasks WHERE id = ?1",
                params![id],
                row_to_supervision_task,
            )
            .optional()
            .map_err(IntuitionError::from)
    }

    /// Epistemic challenges are never represented as a self-justifying graph
    /// update. `grounded` records only a caller's declared eventual grounder;
    /// it is not counted as verified grounding until a terminating verifier
    /// outcome is persisted by the dedicated replay lifecycle.
    pub fn generate_epistemic_challenge(
        &self,
        source_episode: Option<&str>,
        kind: EpistemicChallengeKind,
        input: serde_json::Value,
        expected: serde_json::Value,
        grounded: bool,
    ) -> Result<SupervisionTask, IntuitionError> {
        if !grounded {
            return Err(IntuitionError::Invalid(
                "epistemic challenges require an execution, test, or observation grounder".into(),
            ));
        }
        self.generate_self_supervision(source_episode, input, expected, kind.as_str(), true)
    }

    pub fn grounding_ratio(&self) -> Result<f64, IntuitionError> {
        let (grounded, total): (i64, i64) = self.conn.query_row(
            "SELECT COALESCE(SUM(CASE WHEN completed = 1 AND grounded = 1 THEN 1 ELSE 0 END), 0),
                    COUNT(*)
             FROM supervision_tasks",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(if total == 0 {
            0.0
        } else {
            grounded as f64 / total as f64
        })
    }

    pub fn metrics(&self) -> Result<IntuitionMetrics, IntuitionError> {
        let indexed_documents =
            self.conn
                .query_row("SELECT COUNT(*) FROM recall_documents", [], |row| {
                    row.get::<_, i64>(0)
                })?;
        let inverted_term_rows =
            self.conn
                .query_row("SELECT COUNT(*) FROM recall_terms", [], |row| {
                    row.get::<_, i64>(0)
                })?;
        let (retrieval_queries, candidates_examined): (i64, i64) = self
            .conn
            .query_row(
                "SELECT retrieval_queries, candidates_examined FROM intuition_stats WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .unwrap_or((0, 0));
        let ranking_examples =
            self.conn
                .query_row("SELECT COUNT(*) FROM recall_ranking_examples", [], |row| {
                    row.get::<_, i64>(0)
                })?;
        let ranking_evaluations = self.conn.query_row(
            "SELECT COUNT(*) FROM recall_ranking_evaluations",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        let ranking_search_wins = self.conn.query_row(
            "SELECT COUNT(*) FROM recall_ranking_evaluations
             WHERE learned_improves_search = 1",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        let semantic_recall_evaluations = self.conn.query_row(
            "SELECT COUNT(*) FROM recall_semantic_evaluations",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        let semantic_recall_wins = self.conn.query_row(
            "SELECT COUNT(*) FROM recall_semantic_evaluations
             WHERE semantic_improves_recall = 1",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        let supervision_tasks =
            self.conn
                .query_row("SELECT COUNT(*) FROM supervision_tasks", [], |row| {
                    row.get::<_, i64>(0)
                })?;
        let completed_tasks = self.conn.query_row(
            "SELECT COALESCE(SUM(completed), 0) FROM supervision_tasks",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        let grounded_tasks = self.conn.query_row(
            "SELECT COALESCE(SUM(CASE WHEN completed = 1 AND grounded = 1 THEN 1 ELSE 0 END), 0)
             FROM supervision_tasks",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        let grounding_ratio = if supervision_tasks == 0 {
            0.0
        } else {
            grounded_tasks as f64 / supervision_tasks as f64
        };
        Ok(IntuitionMetrics {
            indexed_documents: indexed_documents as u64,
            inverted_term_rows: inverted_term_rows as u64,
            retrieval_queries: retrieval_queries as u64,
            candidates_examined: candidates_examined as u64,
            ranking_examples: ranking_examples as u64,
            ranking_evaluations: ranking_evaluations as u64,
            ranking_search_wins: ranking_search_wins as u64,
            semantic_recall_evaluations: semantic_recall_evaluations as u64,
            semantic_recall_wins: semantic_recall_wins as u64,
            generated_tasks: supervision_tasks as u64,
            completed_tasks: completed_tasks as u64,
            supervision_tasks: supervision_tasks as u64,
            grounded_tasks: grounded_tasks as u64,
            grounding_ratio,
        })
    }

    fn semantic_features(&self, query: &str) -> Result<BTreeMap<String, f64>, IntuitionError> {
        let mut features = self.unmodified_semantic_features(query)?;
        self.apply_active_representation_model(&mut features)?;
        Ok(features)
    }

    /// Builds the bounded local-semantic query representation without reading
    /// the active artifact. Offline candidate evaluation uses this to compare
    /// a proposed artifact with the incumbent policy in the same process.
    fn unmodified_semantic_features(
        &self,
        query: &str,
    ) -> Result<BTreeMap<String, f64>, IntuitionError> {
        let mut features = direct_semantic_features(query);
        if features.is_empty() {
            return Ok(features);
        }
        let query_terms = features.keys().cloned().collect::<Vec<_>>();
        let query_sql = quoted_terms(&query_terms);
        let mut statement = self.conn.prepare(&format!(
            "WITH seed_documents AS (
                 SELECT DISTINCT document_id FROM recall_semantic_terms
                 WHERE term IN ({query_sql})
                 ORDER BY document_id ASC
                 LIMIT {MAX_SEMANTIC_SEED_DOCUMENTS}
             )
             SELECT term, COUNT(DISTINCT document_id) AS occurrences
             FROM recall_semantic_terms
             WHERE document_id IN (SELECT document_id FROM seed_documents)
               AND term NOT IN ({query_sql})
             GROUP BY term
             ORDER BY occurrences DESC, term ASC
             LIMIT {MAX_SEMANTIC_EXPANSION_TERMS}"
        ))?;
        let expansions = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let maximum_occurrences = expansions
            .first()
            .map(|(_, occurrences)| *occurrences as f64)
            .unwrap_or(1.0);
        for (term, occurrences) in expansions {
            let weight = SEMANTIC_EXPANSION_WEIGHT * occurrences as f64 / maximum_occurrences;
            features.entry(term).or_insert(weight);
        }
        Ok(features)
    }

    /// Fit a small ridge-like linear policy from historical ranking examples.
    /// Every row's prior-success feature is calculated strictly before its own
    /// id. A time-split evaluation passes the first held-out id as the cutoff,
    /// keeping the complete held-out suffix out of both fitting and scoring.
    fn fitted_ranking_model_before(
        &self,
        cutoff_exclusive: i64,
    ) -> Result<FittedRankingModel, IntuitionError> {
        let mut statement = self.conn.prepare(
            "SELECT id, query, candidate_id, used, succeeded FROM recall_ranking_examples
             WHERE id < ?1
             ORDER BY id DESC LIMIT ?2",
        )?;
        let mut examples = statement
            .query_map(
                params![cutoff_exclusive, MAX_RANKING_MODEL_EXAMPLES as i64],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)? != 0,
                        row.get::<_, i64>(4)? != 0,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        examples.reverse();

        let mut rows = Vec::with_capacity(examples.len());
        for (id, query, candidate_id, used, succeeded) in examples {
            let candidates = self.candidates_for_query(&query, 1_024, false)?;
            let Some(candidate) = candidates
                .into_iter()
                .find(|candidate| candidate.id == candidate_id)
            else {
                // A removed or no-longer-retrievable document has no honest
                // current feature vector. Do not make one up for fitting.
                continue;
            };
            let outcome = self.candidate_success_rate_before(&query, &candidate_id, id)?;
            rows.push((
                ranking_features(&candidate, outcome),
                f64::from(used && succeeded),
            ));
        }

        let training_examples = rows.len() as u64;
        let positive_examples = rows.iter().filter(|(_, target)| *target > 0.0).count() as u64;
        let mut means = [0.0; RANKING_FEATURE_COUNT];
        let target_mean = if rows.is_empty() {
            0.0
        } else {
            for (features, _) in &rows {
                for (index, value) in features.iter().enumerate() {
                    means[index] += value;
                }
            }
            for mean in &mut means {
                *mean /= rows.len() as f64;
            }
            rows.iter().map(|(_, target)| target).sum::<f64>() / rows.len() as f64
        };
        let fitted = training_examples >= 4
            && positive_examples > 0
            && positive_examples < training_examples;
        let mut weights = [0.0; RANKING_FEATURE_COUNT];
        if fitted {
            for feature_index in 0..RANKING_FEATURE_COUNT {
                let mut covariance = 0.0;
                let mut variance = 0.0;
                for (features, target) in &rows {
                    let centered = features[feature_index] - means[feature_index];
                    covariance += centered * (target - target_mean);
                    variance += centered * centered;
                }
                weights[feature_index] = (covariance
                    / (variance + RANKING_MODEL_REGULARIZATION * rows.len() as f64))
                    .clamp(-2.0, 2.0);
            }
        }
        Ok(FittedRankingModel {
            training_examples,
            positive_examples,
            fitted,
            intercept: target_mean,
            feature_weights: ranking_feature_map(weights),
            feature_means: ranking_feature_map(means),
        })
    }

    fn active_representation_model(&self) -> Result<Option<RepresentationModel>, IntuitionError> {
        self.conn
            .query_row(
                "SELECT id, model_version, training_tasks, held_out_tasks,
                        held_out_coverage, activated, term_weights_json, created_at
                 FROM representation_models
                 WHERE activated = 1
                 ORDER BY id DESC LIMIT 1",
                [],
                row_to_representation_model,
            )
            .optional()
            .map_err(IntuitionError::from)
    }

    fn apply_active_representation_model(
        &self,
        features: &mut BTreeMap<String, f64>,
    ) -> Result<(), IntuitionError> {
        let Some(model) = self.active_representation_model()? else {
            return Ok(());
        };
        self.apply_representation_model(features, &model);
        Ok(())
    }

    fn apply_representation_model(
        &self,
        features: &mut BTreeMap<String, f64>,
        model: &RepresentationModel,
    ) {
        for (term, feature_weight) in features {
            let learned_term_weight = model.term_weights.get(term).copied().unwrap_or(0.0);
            // Activation may only reweight already bounded local features; it
            // cannot introduce remote terms, documents, beliefs, or trust.
            *feature_weight *= 1.0 + learned_term_weight.clamp(0.0, 2.0);
        }
    }

    fn candidate_success_rate(
        &self,
        query: &str,
        candidate_id: &str,
    ) -> Result<f64, IntuitionError> {
        self.candidate_success_rate_before(query, candidate_id, i64::MAX)
    }

    fn candidate_success_rate_before(
        &self,
        query: &str,
        candidate_id: &str,
        cutoff_exclusive: i64,
    ) -> Result<f64, IntuitionError> {
        let query = canonical_query(query)?;
        let stats: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(used * succeeded), 0), COALESCE(SUM(used), 0)
                 FROM (
                     SELECT used, succeeded FROM recall_ranking_examples
                     WHERE query = ?1 AND candidate_id = ?2 AND id < ?3
                     ORDER BY id DESC LIMIT 256
                 )",
                params![query, candidate_id, cutoff_exclusive],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((successes, uses)) = stats else {
            return Ok(0.5);
        };
        Ok((successes.max(0) as f64 + 1.0) / (uses.max(0) as f64 + 2.0))
    }
}

fn compare_activation(left: &RecallCandidate, right: &RecallCandidate) -> std::cmp::Ordering {
    right
        .activation
        .total_cmp(&left.activation)
        .then_with(|| left.id.cmp(&right.id))
}

fn compare_learned_score(left: &RecallCandidate, right: &RecallCandidate) -> std::cmp::Ordering {
    right
        .learned_score
        .total_cmp(&left.learned_score)
        .then_with(|| left.id.cmp(&right.id))
}

fn ranking_feature_names() -> [&'static str; RANKING_FEATURE_COUNT] {
    [
        "similarity",
        "recency",
        "frequency",
        "activation",
        "prior_success",
    ]
}

fn ranking_feature_map(values: [f64; RANKING_FEATURE_COUNT]) -> BTreeMap<String, f64> {
    ranking_feature_names()
        .into_iter()
        .zip(values)
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

fn ranking_features(candidate: &RecallCandidate, outcome: f64) -> [f64; RANKING_FEATURE_COUNT] {
    [
        candidate.similarity,
        candidate.recency,
        candidate.frequency,
        candidate.activation,
        outcome,
    ]
}

/// The fallback score keeps cold-start retrieval stable. Once the local model
/// has both positive and negative persisted outcomes, its bounded prediction
/// is blended in and changes ordering using only those outcomes.
fn score_candidate(model: &FittedRankingModel, features: [f64; RANKING_FEATURE_COUNT]) -> f64 {
    let baseline = 0.7 * features[0] + 0.2 * features[1] + 0.1 * features[2] + 0.35 * features[4];
    if !model.fitted {
        return baseline;
    }
    let prediction = ranking_feature_names()
        .into_iter()
        .enumerate()
        .fold(model.intercept, |score, (index, name)| {
            let coefficient = model.feature_weights.get(name).copied().unwrap_or(0.0);
            let mean = model.feature_means.get(name).copied().unwrap_or(0.0);
            score + coefficient * (features[index] - mean)
        })
        .clamp(0.0, 1.0);
    0.45 * baseline + 0.55 * prediction
}

fn rank_of(candidates: &[RecallCandidate], candidate_id: &str) -> Option<usize> {
    candidates
        .iter()
        .position(|candidate| candidate.id == candidate_id)
        .map(|index| index + 1)
}

fn row_to_ranking_evaluation(row: &rusqlite::Row<'_>) -> rusqlite::Result<RankingEvaluation> {
    Ok(RankingEvaluation {
        id: row.get(0)?,
        query: row.get(1)?,
        candidate_limit: row.get::<_, i64>(2)? as usize,
        training_examples: row.get::<_, i64>(3)? as u64,
        held_out_examples: row.get::<_, i64>(4)? as u64,
        held_out_successes: row.get::<_, i64>(5)? as u64,
        scored_successes: row.get::<_, i64>(6)? as u64,
        baseline_mean_rank: row.get(7)?,
        learned_mean_rank: row.get(8)?,
        baseline_mean_reciprocal_rank: row.get(9)?,
        learned_mean_reciprocal_rank: row.get(10)?,
        learned_improves_search: row.get::<_, i64>(11)? != 0,
        created_at: row.get(12)?,
    })
}

fn row_to_representation_model(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepresentationModel> {
    let weights: String = row.get(6)?;
    Ok(RepresentationModel {
        id: row.get(0)?,
        model_version: row.get(1)?,
        training_tasks: row.get::<_, i64>(2)? as u64,
        held_out_tasks: row.get::<_, i64>(3)? as u64,
        held_out_coverage: row.get(4)?,
        activated: row.get::<_, i64>(5)? != 0,
        term_weights: serde_json::from_str(&weights).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        created_at: row.get(7)?,
    })
}

fn row_to_representation_regression(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RepresentationRegressionEvaluation> {
    Ok(RepresentationRegressionEvaluation {
        id: row.get(0)?,
        model_id: row.get(1)?,
        held_out_queries: row.get::<_, i64>(2)? as u64,
        held_out_successes: row.get::<_, i64>(3)? as u64,
        baseline_scored_successes: row.get::<_, i64>(4)? as u64,
        candidate_scored_successes: row.get::<_, i64>(5)? as u64,
        baseline_mean_rank: row.get(6)?,
        candidate_mean_rank: row.get(7)?,
        preserves_search: row.get::<_, i64>(8)? != 0,
        created_at: row.get(9)?,
    })
}

fn row_to_supervision_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<SupervisionTask> {
    let input: String = row.get(2)?;
    let target: String = row.get(3)?;
    let outcome: Option<String> = row.get(7)?;
    let parse_json = |index, value: String| {
        serde_json::from_str(&value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    };
    Ok(SupervisionTask {
        id: row.get(0)?,
        kind: row.get(1)?,
        input: parse_json(2, input)?,
        target: parse_json(3, target)?,
        grounded: row.get::<_, i64>(4)? != 0,
        source_episode: row.get(5)?,
        completed: row.get::<_, i64>(6)? != 0,
        outcome: outcome.map(|value| parse_json(7, value)).transpose()?,
        verifier: row.get(8)?,
        completed_at: row.get(9)?,
    })
}

/// A deterministic query key makes ranking evidence query-conditioned without
/// treating case, punctuation, or token order as different situations.
fn canonical_query(query: &str) -> Result<String, IntuitionError> {
    let terms = tokenize(query);
    if terms.is_empty() {
        return Err(IntuitionError::Invalid(
            "ranking query must contain at least one meaningful term".into(),
        ));
    }
    Ok(terms.join(" "))
}

fn parse_kind(kind: &str) -> RecallKind {
    match kind {
        "procedure" => RecallKind::Procedure,
        "episode" => RecallKind::Episode,
        _ => RecallKind::Concept,
    }
}

fn tokenize(text: &str) -> Vec<String> {
    let mut terms = BTreeSet::new();
    for token in text
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|token| token.chars().count() >= 2)
    {
        terms.insert(token.clone());
        // Short character prefixes let bounded retrieval bridge common
        // inflections ("double"/"doubling") without a corpus scan. They
        // remain deterministic postings, not a fuzzy authority signal.
        for length in [3, 4, 5] {
            if token.chars().count() >= length {
                terms.insert(token.chars().take(length).collect());
            }
        }
        if terms.len() >= MAX_QUERY_TERMS {
            break;
        }
    }
    terms.into_iter().collect()
}

/// Exact terms are kept separately from the prefix postings used for lexical
/// recall. Their document co-occurrence is a compact local semantic signal:
/// a query can reach a related document without a process-global model or
/// network call.
fn semantic_tokenize(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|term| term.chars().count() >= 2)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_QUERY_TERMS)
        .collect()
}

fn direct_semantic_features(query: &str) -> BTreeMap<String, f64> {
    semantic_tokenize(query)
        .into_iter()
        .map(|term| (term, 1.0))
        .collect()
}

fn semantic_similarity(query_features: &BTreeMap<String, f64>, document_terms: &[String]) -> f64 {
    let query_norm = query_features
        .values()
        .map(|weight| weight * weight)
        .sum::<f64>()
        .sqrt();
    let document_norm = (document_terms.len() as f64).sqrt();
    if query_norm == 0.0 || document_norm == 0.0 {
        return 0.0;
    }
    let dot = document_terms
        .iter()
        .filter_map(|term| query_features.get(term))
        .sum::<f64>();
    dot / (query_norm * document_norm)
}

/// Terms originate from tokenizers that retain only alphanumeric characters,
/// so this is safe SQL literal construction for bounded local query lists.
fn quoted_terms(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| format!("'{term}'"))
        .collect::<Vec<_>>()
        .join(",")
}

fn embed(text: &str) -> Vec<f64> {
    let mut vector = vec![0.0; VECTOR_DIMENSIONS];
    for term in tokenize(text) {
        let mut hash = 2_166_136_261u32;
        for byte in term.as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(16_777_619);
        }
        let index = (hash as usize) % VECTOR_DIMENSIONS;
        vector[index] += 1.0;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn unix_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(id: &str, kind: RecallKind, text: &str, created_at: i64) -> RecallDocument {
        RecallDocument {
            id: id.into(),
            kind,
            text: text.into(),
            concept_ids: Vec::new(),
            created_at,
        }
    }

    #[test]
    fn inverted_recall_returns_relevant_candidates_with_a_bounded_pool() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .index_document(&document(
                "double",
                RecallKind::Procedure,
                "double integer by two",
                1,
            ))
            .unwrap();
        store
            .index_document(&document(
                "weather",
                RecallKind::Concept,
                "weather and rainfall",
                1,
            ))
            .unwrap();
        let results = store.retrieve("double integer", 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "double");
        assert!(results[0].similarity > 0.5);
    }

    #[test]
    fn local_semantic_cooccurrence_reaches_related_terms_without_hash_collisions() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .index_document(&document(
                "bridge",
                RecallKind::Concept,
                "feline cat companion",
                1,
            ))
            .unwrap();
        store
            .index_document(&document(
                "cat-care",
                RecallKind::Procedure,
                "cat grooming routine",
                1,
            ))
            .unwrap();
        store
            .index_document(&document(
                "weather",
                RecallKind::Concept,
                "weather rainfall forecast",
                1,
            ))
            .unwrap();

        let lexical = store
            .candidates_for_query_mode("feline", 4, false, false)
            .unwrap();
        assert!(lexical.iter().all(|candidate| candidate.id != "cat-care"));
        let semantic = store.retrieve("feline", 4).unwrap();
        assert!(semantic.iter().any(|candidate| candidate.id == "cat-care"));
        assert!(semantic.len() <= 4);
    }

    #[test]
    fn semantic_evaluation_holds_out_complete_query_groups() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .index_document(&document(
                "bridge",
                RecallKind::Concept,
                "feline cat companion",
                1,
            ))
            .unwrap();
        store
            .index_document(&document(
                "cat-care",
                RecallKind::Procedure,
                "cat grooming routine",
                1,
            ))
            .unwrap();
        store
            .index_document(&document(
                "weather",
                RecallKind::Concept,
                "weather rainfall forecast",
                1,
            ))
            .unwrap();
        store
            .record_ranking_example(&RankingExample {
                query: "weather".into(),
                candidate_id: "weather".into(),
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();
        store
            .record_ranking_example(&RankingExample {
                query: "feline".into(),
                candidate_id: "cat-care".into(),
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();

        let evaluation = store.evaluate_semantic_recall(4, 1).unwrap();
        assert_eq!(evaluation.training_queries, 1);
        assert_eq!(evaluation.held_out_queries, 1);
        assert_eq!(evaluation.held_out_successes, 1);
        assert_eq!(evaluation.lexical_scored_successes, 0);
        assert_eq!(evaluation.semantic_scored_successes, 1);
        assert!(evaluation.semantic_improves_recall);
        let metrics = store.metrics().unwrap();
        assert_eq!(metrics.semantic_recall_evaluations, 1);
        assert_eq!(metrics.semantic_recall_wins, 1);
    }

    #[test]
    fn learned_ranking_uses_outcomes_without_mutating_documents() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .index_document(&document("a", RecallKind::Concept, "math arithmetic", 1))
            .unwrap();
        store
            .index_document(&document("b", RecallKind::Concept, "math arithmetic", 1))
            .unwrap();
        store
            .record_ranking_example(&RankingExample {
                query: "math".into(),
                candidate_id: "a".into(),
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();
        let ranked = store.rank("math", 2).unwrap();
        assert_eq!(ranked.len(), 2);
        assert!(
            ranked
                .iter()
                .all(|candidate| candidate.learned_score.is_finite())
        );
        assert_eq!(store.metrics().unwrap().ranking_examples, 1);
    }

    #[test]
    fn fitted_local_ranker_changes_order_only_after_persisted_outcomes() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .index_document(&document("a", RecallKind::Concept, "math arithmetic", 1))
            .unwrap();
        store
            .index_document(&document("b", RecallKind::Concept, "math arithmetic", 1))
            .unwrap();
        // With no local episode outcomes, deterministic activation tie-breaks
        // preserve document id order.
        assert_eq!(store.rank("math", 2).unwrap()[0].id, "a");
        for (candidate_id, succeeded) in [("b", true), ("b", true), ("a", false), ("a", false)] {
            store
                .record_ranking_example(&RankingExample {
                    query: "math".into(),
                    candidate_id: candidate_id.into(),
                    used: true,
                    succeeded,
                    rung: 1,
                })
                .unwrap();
        }

        let model = store.fitted_ranking_model().unwrap();
        assert!(model.fitted);
        assert_eq!(model.training_examples, 4);
        assert_eq!(model.positive_examples, 2);
        assert!(model.feature_weights["prior_success"] > 0.0);
        // The only changed input is persisted local usage evidence; indexing
        // never writes graph truth and rank only returns a reordered pool.
        assert_eq!(store.rank("math", 2).unwrap()[0].id, "b");
    }

    #[test]
    fn ranking_evaluation_fits_no_coefficients_from_its_held_out_suffix() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .index_document(&document("a", RecallKind::Concept, "math arithmetic", 1))
            .unwrap();
        store
            .index_document(&document("b", RecallKind::Concept, "math arithmetic", 1))
            .unwrap();
        for (candidate_id, succeeded) in [("b", true), ("b", true), ("a", false), ("a", false)] {
            store
                .record_ranking_example(&RankingExample {
                    query: "math".into(),
                    candidate_id: candidate_id.into(),
                    used: true,
                    succeeded,
                    rung: 1,
                })
                .unwrap();
        }
        // This newest outcome is deliberately contrary to the learned policy.
        // It must be scored, but must not be allowed to reshape the model.
        store
            .record_ranking_example(&RankingExample {
                query: "math".into(),
                candidate_id: "a".into(),
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();
        let held_out_id = store
            .conn
            .query_row("SELECT MAX(id) FROM recall_ranking_examples", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        let pre_holdout_model = store.fitted_ranking_model_before(held_out_id).unwrap();
        let full_model = store.fitted_ranking_model().unwrap();
        let evaluation = store.evaluate_ranking("math", 2, 1).unwrap();

        assert_eq!(pre_holdout_model.training_examples, 4);
        assert_eq!(
            evaluation.training_examples,
            pre_holdout_model.training_examples
        );
        assert_eq!(full_model.training_examples, 5);
        assert_eq!(evaluation.held_out_successes, 1);
        // `a` is rank one under activation but rank two under the policy fitted
        // before its held-out success. A leaky evaluator would incorrectly
        // train on that final row.
        assert_eq!(evaluation.baseline_mean_rank, Some(1.0));
        assert_eq!(evaluation.learned_mean_rank, Some(2.0));
    }

    #[test]
    fn ranking_evaluation_uses_a_bounded_time_split_and_records_evidence() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .index_document(&document("a", RecallKind::Concept, "math arithmetic", 1))
            .unwrap();
        store
            .index_document(&document("b", RecallKind::Concept, "math arithmetic", 1))
            .unwrap();
        store
            .record_ranking_example(&RankingExample {
                query: "math".into(),
                candidate_id: "a".into(),
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();
        store
            .record_ranking_example(&RankingExample {
                query: "math".into(),
                candidate_id: "b".into(),
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();
        let evaluation = store.evaluate_ranking("math", 2, 1).unwrap();
        assert_eq!(evaluation.held_out_examples, 1);
        assert_eq!(evaluation.training_examples, 1);
        assert_eq!(store.metrics().unwrap().ranking_evaluations, 1);
        assert!(store.latest_ranking_evaluation("math").unwrap().is_some());
    }

    #[test]
    fn time_split_evaluation_proves_a_query_conditioned_search_win() {
        let store = IntuitionStore::in_memory().unwrap();
        // The activation-only ordering is deterministically `a`, then `b`.
        // Repeated historical success for `b` should move `b` first without
        // consulting the final held-out success.
        store
            .index_document(&document("a", RecallKind::Concept, "rank demo", 1))
            .unwrap();
        store
            .index_document(&document("b", RecallKind::Concept, "rank demo", 1))
            .unwrap();
        for _ in 0..3 {
            store
                .record_ranking_example(&RankingExample {
                    query: "Rank demo!".into(),
                    candidate_id: "b".into(),
                    used: true,
                    succeeded: true,
                    rung: 1,
                })
                .unwrap();
        }
        store
            .record_ranking_example(&RankingExample {
                query: "rank demo".into(),
                candidate_id: "b".into(),
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();

        let evaluation = store.evaluate_ranking("demo rank", 2, 1).unwrap();
        assert_eq!(evaluation.training_examples, 3);
        assert_eq!(evaluation.held_out_examples, 1);
        assert_eq!(evaluation.held_out_successes, 1);
        assert_eq!(evaluation.scored_successes, 1);
        assert_eq!(evaluation.baseline_mean_rank, Some(2.0));
        assert_eq!(evaluation.learned_mean_rank, Some(1.0));
        assert!(evaluation.learned_improves_search);

        let stored = store
            .latest_ranking_evaluation("rank demo")
            .unwrap()
            .unwrap();
        assert_eq!(stored.id, evaluation.id);
        let metrics = store.metrics().unwrap();
        assert_eq!(metrics.ranking_evaluations, 1);
        assert_eq!(metrics.ranking_search_wins, 1);
    }

    #[test]
    fn time_split_evaluation_never_calls_untrained_or_unscorable_data_a_win() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .index_document(&document("a", RecallKind::Concept, "rank demo", 1))
            .unwrap();
        store
            .record_ranking_example(&RankingExample {
                query: "rank demo".into(),
                candidate_id: "missing-document".into(),
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();

        let evaluation = store.evaluate_ranking("rank demo", 1, 1).unwrap();
        assert_eq!(evaluation.training_examples, 0);
        assert_eq!(evaluation.held_out_successes, 1);
        assert_eq!(evaluation.scored_successes, 0);
        assert_eq!(evaluation.baseline_mean_rank, None);
        assert_eq!(evaluation.learned_mean_rank, None);
        assert!(!evaluation.learned_improves_search);
    }

    #[test]
    fn caller_declared_grounding_is_not_counted_until_a_verifier_completes_it() {
        let store = IntuitionStore::in_memory().unwrap();
        let task = store
            .generate_self_supervision(
                Some("episode-1"),
                serde_json::json!({"situation":"double 7"}),
                serde_json::json!({"answer":14}),
                "predict_validated_interpretation",
                true,
            )
            .unwrap();
        assert!(task.grounded);
        assert!(!task.completed);
        assert_eq!(store.grounding_ratio().unwrap(), 0.0);
        let metrics = store.metrics().unwrap();
        assert_eq!(metrics.supervision_tasks, 1);
        assert_eq!(metrics.generated_tasks, 1);
        assert_eq!(metrics.completed_tasks, 0);
        assert_eq!(metrics.grounded_tasks, 0);
        assert_eq!(metrics.grounding_ratio, 0.0);
    }

    #[test]
    fn metrics_expose_a_zero_grounding_ratio_for_an_ungrounded_task() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .generate_self_supervision(
                None,
                serde_json::json!({"situation":"unverified"}),
                serde_json::json!({"answer":"unknown"}),
                "predict_missing_phrase",
                false,
            )
            .unwrap();

        let metrics = store.metrics().unwrap();
        assert_eq!(metrics.supervision_tasks, 1);
        assert_eq!(metrics.grounded_tasks, 0);
        assert_eq!(metrics.grounding_ratio, 0.0);
    }

    #[test]
    fn verified_replay_tasks_are_rate_limited_and_only_grounded_on_completion() {
        let store = IntuitionStore::in_memory().unwrap();
        let first = store
            .begin_verified_trace_replay(
                "trusted-episode-0",
                serde_json::json!({"challenge":"replay"}),
                serde_json::json!({"answer":14}),
            )
            .unwrap();
        assert!(!first.completed);
        let completed = store
            .complete_verified_trace_replay(
                first.id,
                true,
                "local-test-verifier",
                serde_json::json!({"status":"matched"}),
            )
            .unwrap();
        assert!(completed.completed);
        assert!(completed.grounded);
        assert_eq!(
            completed.source_episode.as_deref(),
            Some("trusted-episode-0")
        );

        for index in 1..MAX_AUTO_GROUNDED_SUPERVISION_TASKS {
            store
                .begin_verified_trace_replay(
                    &format!("trusted-episode-{index}"),
                    serde_json::json!({"challenge":"replay"}),
                    serde_json::json!({"answer":index}),
                )
                .unwrap();
        }
        assert!(
            store
                .begin_verified_trace_replay(
                    "over-budget-source",
                    serde_json::json!({"challenge":"replay"}),
                    serde_json::json!({"answer":"nope"}),
                )
                .is_err()
        );
        let metrics = store.metrics().unwrap();
        assert_eq!(metrics.generated_tasks, MAX_AUTO_GROUNDED_SUPERVISION_TASKS);
        assert_eq!(metrics.completed_tasks, 1);
        assert_eq!(metrics.grounded_tasks, 1);
    }

    #[test]
    fn representation_training_is_bounded_versioned_and_separate_from_beliefs() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .index_document(&document(
                "double-7",
                RecallKind::Concept,
                "double 7 guide",
                1,
            ))
            .unwrap();
        store
            .record_ranking_example(&RankingExample {
                query: "double 7".into(),
                candidate_id: "double-7".into(),
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();
        for situation in ["double 7", "double 8", "weather now"] {
            store
                .generate_self_supervision(
                    Some("trusted-episode"),
                    serde_json::json!({"situation": situation}),
                    serde_json::json!({"target": situation}),
                    "predict_validated_interpretation",
                    true,
                )
                .unwrap();
        }
        let model = store.train_representation_model(1).unwrap();
        assert_eq!(model.training_tasks, 2);
        assert_eq!(model.held_out_tasks, 1);
        assert!(!model.model_version.is_empty());
        assert!(!model.activated);
        let regression = store.evaluate_representation_model(model.id, 1).unwrap();
        assert!(regression.preserves_search);
        let activated = store.activate_representation_model(model.id).unwrap();
        assert!(activated.activated);
    }

    #[test]
    fn representation_activation_requires_offline_regression_evidence() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .generate_self_supervision(
                Some("trusted-episode"),
                serde_json::json!({"situation":"double 7"}),
                serde_json::json!({"target":"double 7"}),
                "predict_validated_interpretation",
                true,
            )
            .unwrap();
        let model = store.train_representation_model(1).unwrap();
        let error = store.activate_representation_model(model.id).unwrap_err();
        assert!(error.to_string().contains("offline held-out regression"));
    }

    #[test]
    fn activated_representation_artifact_reweights_retrieval_from_prior_episode_tasks() {
        let store = IntuitionStore::in_memory().unwrap();
        store
            .index_document(&document("a-beta", RecallKind::Concept, "beta guide", 1))
            .unwrap();
        store
            .index_document(&document("z-alpha", RecallKind::Concept, "alpha guide", 1))
            .unwrap();
        // Equal lexical evidence starts in deterministic id order.
        assert_eq!(store.retrieve("alpha beta", 2).unwrap()[0].id, "a-beta");
        for (episode, situation) in [
            ("trusted-episode-1", "alpha observation one"),
            ("trusted-episode-2", "alpha observation two"),
            ("trusted-episode-3", "held out unrelated task"),
        ] {
            store
                .generate_self_supervision(
                    Some(episode),
                    serde_json::json!({"situation": situation}),
                    serde_json::json!({"target": situation}),
                    "predict_validated_interpretation",
                    true,
                )
                .unwrap();
        }
        let artifact = store.train_representation_model(1).unwrap();
        assert!(artifact.term_weights["alpha"] > 0.0);
        assert!(!artifact.activated);
        store
            .record_ranking_example(&RankingExample {
                query: "alpha beta".into(),
                candidate_id: "z-alpha".into(),
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();
        assert!(
            store
                .evaluate_representation_model(artifact.id, 1)
                .unwrap()
                .preserves_search
        );
        store.activate_representation_model(artifact.id).unwrap();

        // Activation affects only bounded existing query features. The alpha
        // preference comes from prior episode-backed training tasks, not a
        // belief update, remote model, or new candidate introduction.
        assert_eq!(store.retrieve("alpha beta", 2).unwrap()[0].id, "z-alpha");
    }
}
