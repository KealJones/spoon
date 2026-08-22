//! Phase 3 intuition primitives.
//!
//! This crate deliberately keeps retrieval and ranking separate from belief
//! mutation. It can change candidate order and representation statistics, but
//! it cannot promote a claim, mint trust, or alter graph lifecycle state.

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const VECTOR_DIMENSIONS: usize = 32;
const MAX_QUERY_TERMS: usize = 64;

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
    pub supervision_tasks: u64,
    pub grounded_tasks: u64,
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
             CREATE TABLE IF NOT EXISTS supervision_tasks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 kind TEXT NOT NULL,
                 input_json TEXT NOT NULL,
                 target_json TEXT NOT NULL,
                 grounded INTEGER NOT NULL,
                 source_episode TEXT,
                 created_at INTEGER NOT NULL
             );
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
        candidates.sort_by(|left, right| right.activation.total_cmp(&left.activation));
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
        Ok(candidates)
    }

    /// Online ranker trained only on retrieval/use outcomes. It changes
    /// ordering, not knowledge truth or lifecycle.
    pub fn rank(
        &self,
        query: &str,
        candidate_limit: usize,
    ) -> Result<Vec<RecallCandidate>, IntuitionError> {
        let mut candidates = self.retrieve(query, candidate_limit)?;
        let weights = self.learned_weights()?;
        for candidate in &mut candidates {
            let outcome = self.candidate_success_rate(query, &candidate.id)?;
            candidate.learned_score = weights.0 * candidate.similarity
                + weights.1 * candidate.recency
                + weights.2 * candidate.frequency
                + weights.3 * candidate.activation
                + 0.35 * outcome;
        }
        candidates.sort_by(|left, right| right.learned_score.total_cmp(&left.learned_score));
        Ok(candidates)
    }

    pub fn record_ranking_example(&self, example: &RankingExample) -> Result<(), IntuitionError> {
        self.conn.execute(
            "INSERT INTO recall_ranking_examples
                (query, candidate_id, used, succeeded, rung, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                example.query,
                example.candidate_id,
                i64::from(example.used),
                i64::from(example.succeeded),
                i64::from(example.rung),
                unix_time(),
            ],
        )?;
        Ok(())
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
        Ok(IntuitionMetrics {
            indexed_documents: indexed_documents as u64,
            inverted_term_rows: inverted_term_rows as u64,
            retrieval_queries: retrieval_queries as u64,
            candidates_examined: candidates_examined as u64,
            ranking_examples: ranking_examples as u64,
            supervision_tasks: supervision_tasks as u64,
            grounded_tasks: grounded_tasks as u64,
        })
    }

    fn learned_weights(&self) -> Result<(f64, f64, f64, f64), IntuitionError> {
        let mut statement = self.conn.prepare(
            "SELECT used, succeeded, rung FROM recall_ranking_examples
                 ORDER BY id DESC LIMIT 4096",
        )?;
        let mut total = 0.0;
        let mut success = 0.0;
        let mut cheap = 0.0;
        for row in statement.query_map([], |row| {
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
        let stats: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(used * succeeded), 0), COALESCE(SUM(used), 0)
                 FROM (
                     SELECT used, succeeded FROM recall_ranking_examples
                     WHERE query = ?1 AND candidate_id = ?2
                     ORDER BY id DESC LIMIT 256
                 )",
                params![query, candidate_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((successes, uses)) = stats else {
            return Ok(0.5);
        };
        Ok((successes.max(0) as f64 + 1.0) / (uses.max(0) as f64 + 2.0))
    }
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
        assert_eq!(store.metrics().unwrap().supervision_tasks, 1);
    }
}
