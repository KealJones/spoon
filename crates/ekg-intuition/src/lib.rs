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
const MAX_RANKING_EVALUATION_HOLDOUT: usize = 256;
const MAX_REPRESENTATION_TRAINING_HOLDOUT: usize = 256;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisionTask {
    pub id: i64,
    pub kind: String,
    pub input: serde_json::Value,
    pub target: serde_json::Value,
    pub grounded: bool,
    pub source_episode: Option<String>,
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
             CREATE TABLE IF NOT EXISTS supervision_tasks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 kind TEXT NOT NULL,
                 input_json TEXT NOT NULL,
                 target_json TEXT NOT NULL,
                 grounded INTEGER NOT NULL,
                 source_episode TEXT,
                 created_at INTEGER NOT NULL
             );
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
             CREATE TABLE IF NOT EXISTS intuition_stats (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 retrieval_queries INTEGER NOT NULL DEFAULT 0,
                 candidates_examined INTEGER NOT NULL DEFAULT 0
             );",
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
        if candidate_limit == 0 || candidate_limit > 1_024 {
            return Err(IntuitionError::Invalid(
                "candidate limit must be 1..=1024".into(),
            ));
        }
        let terms = tokenize(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let terms_json = terms
            .iter()
            .map(|term| format!("'{term}'"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT d.id, d.kind, d.text, d.vector_json, d.created_at,
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
            let vector_json: String = row.get(3)?;
            Ok((
                row.get::<_, String>(0)?,
                parse_kind(&kind),
                row.get::<_, String>(2)?,
                vector_json,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        let now = unix_time();
        let query_vector = embed(query);
        let mut candidates = Vec::new();
        for row in rows {
            let (id, kind, text, vector_json, created_at, retrieval_count, matched) = row?;
            let vector: Vec<f64> = serde_json::from_str(&vector_json)?;
            let similarity = cosine(&query_vector, &vector);
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
        let weights = self.learned_weights()?;
        for candidate in &mut candidates {
            let outcome = self.candidate_success_rate(query, &candidate.id)?;
            candidate.learned_score = score_candidate(candidate, weights, outcome);
        }
        candidates.sort_by(compare_learned_score);
        Ok(candidates)
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
        let weights = self.learned_weights_before(cutoff)?;
        for candidate in &mut learned {
            let outcome = self.candidate_success_rate_before(&query, &candidate.id, cutoff)?;
            candidate.learned_score = score_candidate(candidate, weights, outcome);
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
                (kind, input_json, target_json, grounded, source_episode, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
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
        })
    }

    /// Epistemic challenges are never represented as a self-justifying graph
    /// update. They must be marked as grounded because the caller promises a
    /// later execution, test, or observation will terminate the challenge.
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
            "SELECT COALESCE(SUM(grounded), 0), COUNT(*) FROM supervision_tasks",
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
        let supervision_tasks =
            self.conn
                .query_row("SELECT COUNT(*) FROM supervision_tasks", [], |row| {
                    row.get::<_, i64>(0)
                })?;
        let grounded_tasks = self.conn.query_row(
            "SELECT COALESCE(SUM(grounded), 0) FROM supervision_tasks",
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
            supervision_tasks: supervision_tasks as u64,
            grounded_tasks: grounded_tasks as u64,
            grounding_ratio,
        })
    }

    fn learned_weights(&self) -> Result<(f64, f64, f64, f64), IntuitionError> {
        self.learned_weights_before(i64::MAX)
    }

    fn learned_weights_before(
        &self,
        cutoff_exclusive: i64,
    ) -> Result<(f64, f64, f64, f64), IntuitionError> {
        let mut statement = self.conn.prepare(
            "SELECT used, succeeded, rung FROM recall_ranking_examples
             WHERE id < ?1
             ORDER BY id DESC LIMIT 4096",
        )?;
        let mut total = 0.0;
        let mut success = 0.0;
        let mut cheap = 0.0;
        for row in statement.query_map(params![cutoff_exclusive], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })? {
            let (used, succeeded, rung) = row?;
            total += 1.0;
            success += (used * succeeded) as f64;
            cheap += if used > 0 {
                1.0 / (rung.max(1) as f64)
            } else {
                0.0
            };
        }
        if total == 0.0 {
            return Ok((0.7, 0.2, 0.1, 0.0));
        }
        let use_rate = success / total;
        let cheap_rate = cheap / total;
        Ok((0.55 + 0.25 * use_rate, 0.15 + 0.1 * cheap_rate, 0.1, 0.2))
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

fn score_candidate(
    candidate: &RecallCandidate,
    weights: (f64, f64, f64, f64),
    outcome: f64,
) -> f64 {
    weights.0 * candidate.similarity
        + weights.1 * candidate.recency
        + weights.2 * candidate.frequency
        + weights.3 * candidate.activation
        + 0.35 * outcome
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

fn cosine(left: &[f64], right: &[f64]) -> f64 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
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
    fn supervision_tracks_grounding_and_never_promotes_a_claim() {
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
        assert_eq!(store.grounding_ratio().unwrap(), 1.0);
        let metrics = store.metrics().unwrap();
        assert_eq!(metrics.supervision_tasks, 1);
        assert_eq!(metrics.grounding_ratio, 1.0);
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
    fn representation_training_is_bounded_versioned_and_separate_from_beliefs() {
        let store = IntuitionStore::in_memory().unwrap();
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
        let activated = store.activate_representation_model(model.id).unwrap();
        assert!(activated.activated);
    }
}
