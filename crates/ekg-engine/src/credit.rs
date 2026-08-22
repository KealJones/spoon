use std::collections::{HashMap, HashSet};

use ekg_core::{Contract, Episode, EpisodeId, Evaluation, Expr, ProcedureId, Value};
use ekg_credit::{
    Attribution, AttributionConfidence, AttributionEvidence, ContractAttributionReport,
    CounterfactualCandidate, CounterfactualMode, CounterfactualReplayer, CounterfactualReport,
    ReplayBudget, ReplayObservation, ReplayOutcome as CreditReplayOutcome, ReplayProvenance,
    ReplayRequest, ReplayVerificationProvenance, attribute_contract_violations,
    rank_statistical_suspects_from_aggregates, run_counterfactual_replays,
};
use ekg_episode::{CreditAggregateSnapshot, CreditElementRef, EpisodeStore};
use ekg_exec::{ContractChecks, Evaluator, ExecTrace};
use ekg_graph::KnowledgeStore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Display;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::{Engine, EngineError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcedureVersionRef {
    pub id: ProcedureId,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ReplayVerification {
    DeterministicExpected {
        expected: Value,
    },
    SimulatedExpected {
        expected: Value,
        model_id: String,
        model_version: String,
        assumptions: Vec<String>,
    },
    /// An engine-minted, immutable simulator observation. Supplying a receipt
    /// id is insufficient: the replayer resolves and validates its contents.
    SimulatedReceipt {
        expected: Value,
        receipt_id: String,
    },
}

impl ReplayVerification {
    fn suggested_expected(&self) -> &Value {
        match self {
            Self::DeterministicExpected { expected, .. }
            | Self::SimulatedExpected { expected, .. }
            | Self::SimulatedReceipt { expected, .. } => expected,
        }
    }

    fn matches_mode(&self, mode: CounterfactualMode) -> bool {
        matches!(
            (self, mode),
            (
                Self::DeterministicExpected { .. },
                CounterfactualMode::Deterministic
            ) | (
                Self::SimulatedExpected { .. } | Self::SimulatedReceipt { .. },
                CounterfactualMode::Simulated
            )
        )
    }
}

/// A typed, pure mutation of exactly one versioned procedure element.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CounterfactualMutation {
    ReplaceBody {
        target: ProcedureVersionRef,
        body: Expr,
        verification: ReplayVerification,
    },
    ReplaceContract {
        target: ProcedureVersionRef,
        contract: Contract,
        verification: ReplayVerification,
    },
}

impl CounterfactualMutation {
    fn target(&self) -> ProcedureVersionRef {
        match self {
            Self::ReplaceBody { target, .. } | Self::ReplaceContract { target, .. } => *target,
        }
    }

    fn verification(&self) -> &ReplayVerification {
        match self {
            Self::ReplaceBody { verification, .. } | Self::ReplaceContract { verification, .. } => {
                verification
            }
        }
    }

    fn set_verification(&mut self, next: ReplayVerification) {
        match self {
            Self::ReplaceBody { verification, .. } | Self::ReplaceContract { verification, .. } => {
                *verification = next
            }
        }
    }
}

/// A bounded request delivered to an engine-selected simulator. The trace and
/// typed single mutation are immutable identities rather than prose hints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimulatedReplayRequest {
    pub source_episode: EpisodeId,
    pub source_trace_hash: String,
    pub suspect: ekg_credit::Suspect,
    pub mutation: CounterfactualMutation,
    pub model_id: String,
    pub model_version: String,
    pub assumptions: Vec<String>,
    pub step_budget: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimulatedReplayObservation {
    pub result: Value,
    pub steps_used: u32,
    pub details: String,
}

/// Engine-owned simulator boundary. Identity comes from the implementation,
/// so a candidate cannot relabel an arbitrary model as an approved one.
pub trait SimulatedReplayModel {
    type Error: Display;

    fn model_id(&self) -> &str;
    fn model_version(&self) -> &str;
    fn simulate(
        &mut self,
        request: SimulatedReplayRequest,
    ) -> Result<SimulatedReplayObservation, Self::Error>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SimulatedReplayReceipt {
    source_episode: EpisodeId,
    source_trace_hash: String,
    suspect: ekg_credit::Suspect,
    mutation_hash: String,
    model_id: String,
    model_version: String,
    assumptions: Vec<String>,
    step_budget: u32,
    canonical_expected: Value,
    result: Value,
    steps_used: u32,
    details: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FailureAnalysisBudget {
    pub top_k: usize,
    pub max_replays: u32,
    pub max_replay_steps: u32,
}

impl Default for FailureAnalysisBudget {
    fn default() -> Self {
        Self {
            top_k: 3,
            max_replays: 3,
            max_replay_steps: 10_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FailureAnalysisRequest {
    pub episode_id: EpisodeId,
    #[serde(default)]
    pub selected_feedback_id: Option<Uuid>,
    pub candidates: Vec<CounterfactualCandidate>,
    pub budget: FailureAnalysisBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FailureEvidenceSource {
    EpisodeEvaluation,
    LateFeedback { feedback_id: Uuid },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureEvidence {
    pub source: FailureEvidenceSource,
    pub observed_result: Option<Value>,
    pub evaluation: Evaluation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureAnalysisCost {
    pub original_execution_cost: f64,
    pub analysis_cache_lookups: u64,
    pub evidence_digest_source_episode_reads: u64,
    pub evidence_digest_history_episodes_scanned: u64,
    pub evidence_digest_feedback_rows_scanned: u64,
    pub evidence_digest_trace_steps_scanned: u64,
    pub evidence_digest_procedure_snapshots_read: u64,
    #[serde(default)]
    pub evidence_digest_element_aggregate_rows_read: u64,
    #[serde(default)]
    pub evidence_digest_pair_aggregate_rows_read: u64,
    pub evidence_digest_work_units: u64,
    pub contract_steps: u64,
    pub statistical_episodes_scanned: u64,
    pub statistical_work_units: f64,
    pub statistical_feedback_rows_scanned: u64,
    /// Evidence rows represented by the materialized aggregate rows. These
    /// are not reread during online analysis.
    #[serde(default)]
    pub statistical_feedback_rows_indexed: u64,
    pub statistical_conflicts_excluded: u64,
    pub statistical_history_episodes_used: u64,
    pub statistical_trace_steps_scanned: u64,
    pub statistical_element_exposures: u64,
    pub statistical_cooccurrence_pairs: u64,
    #[serde(default)]
    pub statistical_element_aggregate_rows_read: u64,
    #[serde(default)]
    pub statistical_pair_aggregate_rows_read: u64,
    /// Index maintenance paid while the source episode was persisted. This is
    /// reported separately from online analysis so the latency metric cannot
    /// disguise deferred database work.
    #[serde(default)]
    pub source_index_maintenance_work_units: u64,
    /// Execution, online attribution, and source-index maintenance together.
    #[serde(default)]
    pub lifecycle_credit_cost: f64,
    pub replay_steps: u64,
    pub attribution_cost: f64,
    pub total_cost: f64,
    pub attribution_cost_ratio: f64,
}

/// Phase 2's bounded acceptance envelope requires attribution to remain less
/// than half of combined execution and attribution work. Equality fails.
pub const PHASE2_MAX_ATTRIBUTION_COST_RATIO: f64 = 0.5;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceDigestCost {
    source_episode_reads: u64,
    history_episodes_scanned: u64,
    feedback_rows_scanned: u64,
    trace_steps_scanned: u64,
    procedure_snapshots_read: u64,
    element_aggregate_rows_read: u64,
    pair_aggregate_rows_read: u64,
}

impl EvidenceDigestCost {
    fn work_units(self) -> u64 {
        self.source_episode_reads
            .saturating_add(self.history_episodes_scanned)
            .saturating_add(self.feedback_rows_scanned)
            .saturating_add(self.trace_steps_scanned)
            .saturating_add(self.procedure_snapshots_read)
            .saturating_add(self.element_aggregate_rows_read)
            .saturating_add(self.pair_aggregate_rows_read)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureAnalysis {
    /// Content-addressed identity for safe retry/deduplication by persistence callers.
    pub analysis_id: String,
    pub episode_id: EpisodeId,
    pub failure_evidence: FailureEvidence,
    pub contract: ContractAttributionReport,
    pub statistical: Vec<Attribution>,
    pub counterfactual: CounterfactualReport,
    pub ranked: Vec<Attribution>,
    pub cost: FailureAnalysisCost,
}

pub(crate) struct CreditAnalysisStore {
    conn: Connection,
}

impl CreditAnalysisStore {
    pub(crate) fn open(path: &str) -> Result<Self, EngineError> {
        let store = Self {
            conn: Connection::open(path)?,
        };
        store.create_schema()?;
        Ok(store)
    }

    pub(crate) fn in_memory() -> Result<Self, EngineError> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.create_schema()?;
        Ok(store)
    }

    fn create_schema(&self) -> Result<(), EngineError> {
        self.conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS engine_credit_analyses (
                analysis_id TEXT PRIMARY KEY,
                request_digest TEXT NOT NULL,
                request_json TEXT NOT NULL,
                analysis_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS engine_credit_analysis_keys (
                idempotency_key TEXT PRIMARY KEY,
                request_digest TEXT NOT NULL,
                analysis_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (analysis_id) REFERENCES engine_credit_analyses(analysis_id)
             );
             CREATE TABLE IF NOT EXISTS engine_simulated_replay_receipts (
                receipt_id TEXT PRIMARY KEY,
                receipt_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
             );
             CREATE TRIGGER IF NOT EXISTS engine_credit_analyses_no_update
             BEFORE UPDATE ON engine_credit_analyses BEGIN
                SELECT RAISE(ABORT, 'credit analyses are immutable');
             END;
             CREATE TRIGGER IF NOT EXISTS engine_credit_analyses_no_delete
             BEFORE DELETE ON engine_credit_analyses BEGIN
                SELECT RAISE(ABORT, 'credit analyses are immutable');
             END;
             CREATE TRIGGER IF NOT EXISTS engine_credit_analysis_keys_no_update
             BEFORE UPDATE ON engine_credit_analysis_keys BEGIN
                SELECT RAISE(ABORT, 'credit analysis keys are immutable');
             END;
             CREATE TRIGGER IF NOT EXISTS engine_credit_analysis_keys_no_delete
             BEFORE DELETE ON engine_credit_analysis_keys BEGIN
                SELECT RAISE(ABORT, 'credit analysis keys are immutable');
             END;
             CREATE TRIGGER IF NOT EXISTS engine_simulated_replay_receipts_no_update
             BEFORE UPDATE ON engine_simulated_replay_receipts BEGIN
                SELECT RAISE(ABORT, 'simulated replay receipts are immutable');
             END;
             CREATE TRIGGER IF NOT EXISTS engine_simulated_replay_receipts_no_delete
             BEFORE DELETE ON engine_simulated_replay_receipts BEGIN
                SELECT RAISE(ABORT, 'simulated replay receipts are immutable');
             END;",
        )?;
        Ok(())
    }

    fn get(&self, analysis_id: &str) -> Result<Option<FailureAnalysis>, EngineError> {
        self.conn
            .query_row(
                "SELECT analysis_json FROM engine_credit_analyses WHERE analysis_id = ?1",
                params![analysis_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }

    fn get_by_key(
        &self,
        idempotency_key: &str,
        request_digest: &str,
        request_json: &str,
    ) -> Result<Option<FailureAnalysis>, EngineError> {
        let stored = self
            .conn
            .query_row(
                "SELECT keys.request_digest, analyses.request_json, analyses.analysis_json
                 FROM engine_credit_analysis_keys AS keys
                 JOIN engine_credit_analyses AS analyses
                   ON analyses.analysis_id = keys.analysis_id
                 WHERE keys.idempotency_key = ?1",
                params![idempotency_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((stored_digest, stored_request_json, analysis_json)) = stored else {
            return Ok(None);
        };
        if stored_digest != request_digest || stored_request_json != request_json {
            return Err(EngineError::InvalidInput(format!(
                "credit analysis idempotency key {idempotency_key:?} is already bound to a different request"
            )));
        }
        Ok(Some(serde_json::from_str(&analysis_json)?))
    }

    fn get_by_key_unchecked(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<FailureAnalysis>, EngineError> {
        self.conn
            .query_row(
                "SELECT analyses.analysis_json
                 FROM engine_credit_analysis_keys AS keys
                 JOIN engine_credit_analyses AS analyses
                   ON analyses.analysis_id = keys.analysis_id
                 WHERE keys.idempotency_key = ?1",
                params![idempotency_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }

    fn insert_or_get(
        &self,
        idempotency_key: &str,
        request_digest: &str,
        request_json: &str,
        analysis: &FailureAnalysis,
    ) -> Result<FailureAnalysis, EngineError> {
        validate_analysis_key(idempotency_key)?;
        let analysis_json = serde_json::to_string(analysis)?;
        let created_at = now_unix();
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO engine_credit_analyses
                (analysis_id, request_digest, request_json, analysis_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                analysis.analysis_id,
                request_digest,
                request_json,
                analysis_json,
                created_at,
            ],
        )?;
        let (stored_request_digest, stored_analysis_json) = transaction.query_row(
            "SELECT request_digest, analysis_json
             FROM engine_credit_analyses WHERE analysis_id = ?1",
            params![analysis.analysis_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        if stored_request_digest != request_digest || stored_analysis_json != analysis_json {
            return Err(EngineError::InvalidInput(format!(
                "credit analysis identity collision for {}",
                analysis.analysis_id
            )));
        }
        let key_insert = transaction.execute(
            "INSERT INTO engine_credit_analysis_keys
                (idempotency_key, request_digest, analysis_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                idempotency_key,
                request_digest,
                analysis.analysis_id,
                created_at,
            ],
        );
        if let Err(error) = key_insert {
            drop(transaction);
            if let Some(stored) = self.get_by_key(idempotency_key, request_digest, request_json)? {
                return Ok(stored);
            }
            return Err(error.into());
        }
        transaction.commit()?;
        Ok(analysis.clone())
    }

    fn get_simulated_receipt(
        &self,
        receipt_id: &str,
    ) -> Result<Option<SimulatedReplayReceipt>, EngineError> {
        self.conn
            .query_row(
                "SELECT receipt_json FROM engine_simulated_replay_receipts WHERE receipt_id = ?1",
                params![receipt_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }

    fn insert_simulated_receipt(
        &self,
        receipt_id: &str,
        receipt: &SimulatedReplayReceipt,
    ) -> Result<(), EngineError> {
        let receipt_json = serde_json::to_string(receipt)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO engine_simulated_replay_receipts
                (receipt_id, receipt_json, created_at) VALUES (?1, ?2, ?3)",
            params![receipt_id, receipt_json, now_unix()],
        )?;
        let stored: String = self.conn.query_row(
            "SELECT receipt_json FROM engine_simulated_replay_receipts WHERE receipt_id = ?1",
            params![receipt_id],
            |row| row.get(0),
        )?;
        if stored != receipt_json {
            return Err(EngineError::InvalidInput(format!(
                "simulated replay receipt identity collision for {receipt_id}"
            )));
        }
        Ok(())
    }
}

#[derive(Default)]
struct HistoricalEvidenceCost {
    episodes_scanned: u64,
    feedback_rows_scanned: u64,
    feedback_rows_indexed: u64,
    conflicts_excluded: u64,
    feedback_ids_scanned: Vec<Uuid>,
    feedback_ids_used: Vec<Uuid>,
    conflicting_episode_ids: Vec<EpisodeId>,
}

/// Trust boundary between generic credit candidates and version-pinned pure
/// execution. Ephemeral execution traces are deliberately discarded.
pub struct VersionPinnedReplayer<'a> {
    graph: &'a KnowledgeStore,
    episodes: &'a EpisodeStore,
    simulated_receipts: Option<&'a CreditAnalysisStore>,
    selected_feedback_id: Option<Uuid>,
}

impl<'a> VersionPinnedReplayer<'a> {
    pub fn new(graph: &'a KnowledgeStore, episodes: &'a EpisodeStore) -> Self {
        Self {
            graph,
            episodes,
            simulated_receipts: None,
            selected_feedback_id: None,
        }
    }

    pub fn with_selected_feedback(mut self, feedback_id: Option<Uuid>) -> Self {
        self.selected_feedback_id = feedback_id;
        self
    }

    fn with_simulated_receipts(mut self, receipts: &'a CreditAnalysisStore) -> Self {
        self.simulated_receipts = Some(receipts);
        self
    }

    fn not_replayable(reason: impl Into<String>) -> ReplayObservation {
        ReplayObservation {
            outcome: CreditReplayOutcome::NotReplayable {
                reason: reason.into(),
            },
            steps_used: 0,
            details: "counterfactual was rejected before execution".into(),
            provenance: ReplayProvenance::default(),
        }
    }
}

impl CounterfactualReplayer for VersionPinnedReplayer<'_> {
    type Error = EngineError;

    fn replay(&mut self, request: ReplayRequest) -> Result<ReplayObservation, Self::Error> {
        let (source, _) = match selected_failure_evidence(
            self.episodes,
            request.source_episode,
            self.selected_feedback_id,
        ) {
            Ok(selected) => selected,
            Err(EngineError::InvalidInput(reason)) => {
                return Ok(Self::not_replayable(reason));
            }
            Err(error) => return Err(error),
        };
        if !source.failed() {
            return Ok(Self::not_replayable(
                "selected source evidence does not identify a failed episode",
            ));
        }
        let canonical = self.episodes.get(request.source_episode)?;
        let Some(canonical_evaluation) = canonical.evaluation.as_ref() else {
            return Ok(Self::not_replayable(
                "source episode has no immutable evaluation for a deterministic oracle",
            ));
        };
        if !matches!(
            canonical_evaluation.tier,
            ekg_core::VerifiabilityTier::Hard | ekg_core::VerifiabilityTier::Consensus
        ) {
            return Ok(Self::not_replayable(
                "deterministic replay requires a precommitted Hard or Consensus oracle",
            ));
        }
        let Some(canonical_expected) = canonical.prediction.clone() else {
            return Ok(Self::not_replayable(
                "source episode has no precommitted prediction for a deterministic oracle",
            ));
        };
        let oracle_digest =
            stable_digest(&(canonical.id, canonical_evaluation.tier, &canonical_expected))?;
        let source_observed = source.observed_result.clone();
        let trace_json = match source.execution_trace {
            Some(trace) => trace,
            None => {
                return Ok(Self::not_replayable(
                    "source episode has no execution trace",
                ));
            }
        };
        let trace: ExecTrace = match serde_json::from_value(trace_json) {
            Ok(trace) => trace,
            Err(error) => {
                return Ok(Self::not_replayable(format!(
                    "source execution trace is invalid: {error}"
                )));
            }
        };
        if trace.steps.is_empty() {
            return Ok(Self::not_replayable("source execution trace is empty"));
        }

        let mut versions = HashMap::<ProcedureId, u32>::new();
        for (index, step) in trace.steps.iter().enumerate() {
            let Some(id) = step.procedure_called else {
                return Ok(Self::not_replayable(format!(
                    "source trace step {index} has no procedure id"
                )));
            };
            let Some(version) = step.procedure_version else {
                return Ok(Self::not_replayable(format!(
                    "source trace step {index} has no procedure version"
                )));
            };
            if versions
                .insert(id, version)
                .is_some_and(|seen| seen != version)
            {
                return Ok(Self::not_replayable(format!(
                    "source trace mixes versions of procedure {id}"
                )));
            }
        }

        let Some(suspect_step) = trace.steps.get(request.suspect.trace_step) else {
            return Ok(Self::not_replayable("suspect trace step is absent"));
        };
        if suspect_step.procedure_called != Some(request.suspect.procedure)
            || suspect_step.procedure_version != Some(request.suspect.version)
        {
            return Ok(Self::not_replayable(
                "suspect identity does not match the pinned source trace step",
            ));
        }
        let target_occurrences = trace
            .steps
            .iter()
            .filter(|step| {
                step.procedure_called == Some(request.suspect.procedure)
                    && step.procedure_version == Some(request.suspect.version)
            })
            .count();
        if target_occurrences != 1 {
            return Ok(Self::not_replayable(format!(
                "procedure {} v{} occurs {target_occurrences} times; a version-wide mutation cannot support step-local decisive attribution",
                request.suspect.procedure, request.suspect.version
            )));
        }
        let target_occurrence = trace.steps[..=request.suspect.trace_step]
            .iter()
            .filter(|step| step.procedure_called == Some(request.suspect.procedure))
            .count()
            - 1;
        let source_contract_checks = suspect_step.contract_checks.clone();

        let mutation: CounterfactualMutation =
            match serde_json::from_value(request.change.replacement.clone()) {
                Ok(mutation) => mutation,
                Err(error) => {
                    return Ok(Self::not_replayable(format!(
                        "mutation is not a supported pure procedure patch: {error}"
                    )));
                }
            };
        let target = mutation.target();
        if target.id != request.suspect.procedure || target.version != request.suspect.version {
            return Ok(Self::not_replayable(
                "mutation target does not match the source suspect id and version",
            ));
        }
        if !mutation.verification().matches_mode(request.mode) {
            return Ok(Self::not_replayable(
                "mutation verification provenance does not match replay mode",
            ));
        }

        let top = trace.steps.last().expect("nonempty trace checked above");
        let top_id = top.procedure_called.expect("all step ids checked above");
        let args = match top.input.as_ref() {
            None => Vec::new(),
            Some(Value::List(values)) => values.clone(),
            Some(_) => {
                return Ok(Self::not_replayable(
                    "top-level trace input is not a positional argument list",
                ));
            }
        };

        let mut pinned = Vec::with_capacity(versions.len());
        for (id, version) in versions {
            let Some(procedure) = self.graph.get_procedure_version(id, version)? else {
                return Ok(Self::not_replayable(format!(
                    "historical procedure {id} v{version} is unavailable"
                )));
            };
            pinned.push(procedure);
        }

        let source_trace_hash = if request.mode == CounterfactualMode::Simulated {
            sha256_digest(&trace)?
        } else {
            stable_digest(&trace)?
        };
        let mutation_hash = mutation_digest(&mutation)?;
        if request.mode == CounterfactualMode::Simulated {
            let receipt_id = match mutation.verification() {
                ReplayVerification::SimulatedReceipt {
                    expected,
                    receipt_id,
                } if expected == &canonical_expected => receipt_id,
                ReplayVerification::SimulatedReceipt { .. } => {
                    return Ok(Self::not_replayable(
                        "simulated receipt expected value does not match the immutable canonical prediction",
                    ));
                }
                _ => {
                    return Ok(Self::not_replayable(
                        "simulated replay has no engine-issued trusted simulator receipt",
                    ));
                }
            };
            let Some(store) = self.simulated_receipts else {
                return Ok(Self::not_replayable(
                    "simulated replay receipts are only resolvable inside the engine trust boundary",
                ));
            };
            let Some(receipt) = store.get_simulated_receipt(receipt_id)? else {
                return Ok(Self::not_replayable(
                    "simulated replay receipt is absent from engine-owned storage",
                ));
            };
            if sha256_digest(&receipt)? != *receipt_id {
                return Ok(Self::not_replayable(
                    "simulated replay receipt failed its content-address check",
                ));
            }
            if receipt.source_episode != canonical.id
                || receipt.source_trace_hash != source_trace_hash
                || receipt.suspect != request.suspect
                || receipt.mutation_hash != mutation_hash
                || receipt.canonical_expected != canonical_expected
            {
                return Ok(Self::not_replayable(
                    "simulated replay receipt does not bind this source trace, mutation, suspect, and oracle",
                ));
            }
            if receipt.steps_used > receipt.step_budget || receipt.steps_used > request.step_budget
            {
                return Ok(Self::not_replayable(
                    "simulated replay receipt exceeds its issued or current replay budget",
                ));
            }
            let provenance = ReplayProvenance {
                source_trace_hash: Some(source_trace_hash),
                mutation_hash: Some(mutation_hash),
                verification: Some(ReplayVerificationProvenance::Simulated {
                    receipt_id: Some(receipt_id.clone()),
                    model_id: receipt.model_id.clone(),
                    model_version: receipt.model_version.clone(),
                    assumptions: receipt.assumptions.clone(),
                }),
            };
            let (outcome, detail) = if source_observed.as_ref() == Some(&canonical_expected) {
                (
                    CreditReplayOutcome::NotReplayable {
                        reason: "the source already matched its oracle, so simulated recovery cannot establish a fault".into(),
                    },
                    "source result already matched the immutable oracle".to_owned(),
                )
            } else if receipt.result == canonical_expected {
                (
                    CreditReplayOutcome::Succeeded,
                    "bounded simulator result matched the immutable canonical prediction"
                        .to_owned(),
                )
            } else {
                (
                    CreditReplayOutcome::Failed,
                    format!(
                        "bounded simulator result {} did not match immutable canonical prediction {}",
                        receipt.result, canonical_expected
                    ),
                )
            };
            return Ok(ReplayObservation {
                outcome,
                steps_used: receipt.steps_used,
                details: format!(
                    "{detail}; simulator {} v{}; receipt {receipt_id}; {}",
                    receipt.model_id, receipt.model_version, receipt.details
                ),
                provenance,
            });
        }
        let verifier = format!(
            "engine:canonical_prediction:episode={}:oracle={oracle_digest}:baseline={source_trace_hash}",
            canonical.id
        );
        let replay_provenance = || ReplayProvenance {
            source_trace_hash: Some(source_trace_hash.clone()),
            mutation_hash: Some(mutation_hash.clone()),
            verification: Some(ReplayVerificationProvenance::Deterministic {
                verifier: verifier.clone(),
            }),
        };

        let mut baseline = Evaluator::new().with_budget(request.step_budget);
        for procedure in &pinned {
            baseline.register_procedure(procedure.clone());
        }
        let baseline_attempt = baseline.exec_procedure_captured(&top_id, args.clone());
        let baseline_steps = baseline.budget().steps_used;
        if stable_digest(&baseline_attempt.trace)? != source_trace_hash {
            return Ok(ReplayObservation {
                outcome: CreditReplayOutcome::NotReplayable {
                    reason:
                        "exact unmodified baseline did not reproduce the immutable source trace"
                            .into(),
                },
                steps_used: baseline_steps,
                details: "counterfactual was not executed because baseline identity failed".into(),
                provenance: replay_provenance(),
            });
        }
        if baseline_steps >= request.step_budget {
            return Ok(ReplayObservation {
                outcome: CreditReplayOutcome::NotReplayable {
                    reason: "replay budget was exhausted reproducing the exact baseline".into(),
                },
                steps_used: baseline_steps,
                details: "no mutation was executed after baseline reproduction".into(),
                provenance: replay_provenance(),
            });
        }

        let mut evaluator = Evaluator::new().with_budget(request.step_budget - baseline_steps);
        for mut procedure in pinned {
            if procedure.id == target.id {
                match &mutation {
                    CounterfactualMutation::ReplaceBody { body, .. } => {
                        if procedure.body == *body {
                            return Ok(ReplayObservation {
                                outcome: CreditReplayOutcome::NotReplayable {
                                    reason:
                                        "replacement body is identical to the pinned source body"
                                            .into(),
                                },
                                steps_used: baseline_steps,
                                details: format!(
                                    "caller expected {} was ignored; canonical oracle {oracle_digest}; exact baseline reproduced before rejecting a no-op patch",
                                    mutation.verification().suggested_expected()
                                ),
                                provenance: replay_provenance(),
                            });
                        }
                        procedure.body = body.clone();
                    }
                    CounterfactualMutation::ReplaceContract { contract, .. } => {
                        if stable_digest(&procedure.contract)? == stable_digest(contract)? {
                            return Ok(ReplayObservation {
                                outcome: CreditReplayOutcome::NotReplayable {
                                    reason: "replacement contract is identical to the pinned source contract"
                                        .into(),
                                },
                                steps_used: baseline_steps,
                                details: format!(
                                    "caller expected {} was ignored; canonical oracle {oracle_digest}; exact baseline reproduced before rejecting a no-op patch",
                                    mutation.verification().suggested_expected()
                                ),
                                provenance: replay_provenance(),
                            });
                        }
                        procedure.contract = contract.clone();
                    }
                }
            }
            evaluator.register_procedure(procedure);
        }

        let attempt = evaluator.exec_procedure_captured(&top_id, args);
        let mutation_steps = evaluator.budget().steps_used;
        let steps_used = baseline_steps.saturating_add(mutation_steps);
        let contract_effect = match &mutation {
            CounterfactualMutation::ReplaceBody { .. } => true,
            CounterfactualMutation::ReplaceContract { .. } => attempt
                .trace
                .steps
                .iter()
                .filter(|step| step.procedure_called == Some(target.id))
                .nth(target_occurrence)
                .is_some_and(|step| {
                    contract_statuses_changed(&source_contract_checks, &step.contract_checks)
                }),
        };
        // `attempt.trace` is deliberately discarded: it refers to an
        // ephemeral mutation and must never masquerade as persisted history.
        let suggestion = mutation.verification().suggested_expected();
        let oracle_detail = format!(
            "caller expected {suggestion} was ignored; canonical precommitted oracle {canonical_expected} ({oracle_digest}); baseline {baseline_steps} steps, mutation {mutation_steps} steps"
        );
        let (outcome, details) = match attempt.result {
            Ok(value)
                if value == canonical_expected
                    && source_observed.as_ref() == Some(&canonical_expected) =>
            {
                (
                    CreditReplayOutcome::NotReplayable {
                        reason: "the output verifier already matched the source result and cannot establish a causal mutation effect".into(),
                    },
                    format!("counterfactual produced the unchanged source result; {oracle_detail}"),
                )
            }
            Ok(value) if value == canonical_expected && !contract_effect => (
                CreditReplayOutcome::NotReplayable {
                    reason: "the contract patch produced no observable contract-check effect".into(),
                },
                format!(
                    "contract check statuses were unchanged by the ephemeral patch; {oracle_detail}"
                ),
            ),
            Ok(value) if value == canonical_expected => (
                CreditReplayOutcome::Succeeded,
                format!(
                    "counterfactual output matched canonical oracle for {} v{}; {oracle_detail}",
                    target.id, target.version,
                ),
            ),
            Ok(value) => (
                CreditReplayOutcome::Failed,
                format!(
                    "counterfactual output {value} did not match canonical oracle {canonical_expected}; {oracle_detail}"
                ),
            ),
            Err(error) => (
                CreditReplayOutcome::Failed,
                format!("counterfactual execution failed: {error}; {oracle_detail}"),
            ),
        };
        Ok(ReplayObservation {
            outcome,
            steps_used,
            details,
            provenance: replay_provenance(),
        })
    }
}

impl Engine {
    pub fn version_pinned_replayer(&self) -> VersionPinnedReplayer<'_> {
        VersionPinnedReplayer::new(&self.graph, &self.episodes)
    }

    /// Executes a typed single mutation in an explicitly admin-selected
    /// bounded model and persists the exact observation as immutable model
    /// evidence. This never creates mutation authority.
    pub fn issue_simulated_replay_receipt<M: SimulatedReplayModel>(
        &self,
        source_episode: EpisodeId,
        mut candidate: CounterfactualCandidate,
        step_budget: u32,
        simulator: &mut M,
    ) -> Result<CounterfactualCandidate, EngineError> {
        self.require_admin()?;
        if candidate.mode != CounterfactualMode::Simulated {
            return Err(EngineError::InvalidInput(
                "simulated receipt issuance requires a simulated candidate".into(),
            ));
        }
        if step_budget == 0 {
            return Err(EngineError::InvalidInput(
                "simulated receipt step budget must be positive".into(),
            ));
        }
        validate_simulator_identity(simulator.model_id(), "model id")?;
        validate_simulator_identity(simulator.model_version(), "model version")?;

        let source = self.episodes.get(source_episode)?;
        if !source.failed() {
            return Err(EngineError::InvalidInput(
                "simulated receipt source must be a failed finalized episode".into(),
            ));
        }
        let evaluation = source.evaluation.as_ref().ok_or_else(|| {
            EngineError::InvalidInput("simulated receipt source has no immutable evaluation".into())
        })?;
        if !matches!(
            evaluation.tier,
            ekg_core::VerifiabilityTier::Hard | ekg_core::VerifiabilityTier::Consensus
        ) {
            return Err(EngineError::InvalidInput(
                "simulated receipt source requires a Hard or Consensus oracle".into(),
            ));
        }
        let canonical_expected = source.prediction.clone().ok_or_else(|| {
            EngineError::InvalidInput(
                "simulated receipt source has no precommitted prediction".into(),
            )
        })?;
        let trace_json = source.execution_trace.clone().ok_or_else(|| {
            EngineError::InvalidInput("simulated receipt source has no execution trace".into())
        })?;
        let trace: ExecTrace = serde_json::from_value(trace_json)?;
        let suspect_step = trace
            .steps
            .get(candidate.suspect.trace_step)
            .ok_or_else(|| {
                EngineError::InvalidInput("simulated receipt suspect trace step is absent".into())
            })?;
        if suspect_step.procedure_called != Some(candidate.suspect.procedure)
            || suspect_step.procedure_version != Some(candidate.suspect.version)
        {
            return Err(EngineError::InvalidInput(
                "simulated receipt suspect does not match the immutable trace".into(),
            ));
        }
        let occurrences = trace
            .steps
            .iter()
            .filter(|step| {
                step.procedure_called == Some(candidate.suspect.procedure)
                    && step.procedure_version == Some(candidate.suspect.version)
            })
            .count();
        if occurrences != 1 {
            return Err(EngineError::InvalidInput(format!(
                "simulated receipt requires an unambiguous suspect occurrence, found {occurrences}"
            )));
        }

        let mut mutation: CounterfactualMutation =
            serde_json::from_value(candidate.change.replacement.clone()).map_err(|error| {
                EngineError::InvalidInput(format!(
                    "simulated receipt mutation is not a typed single patch: {error}"
                ))
            })?;
        if mutation.target().id != candidate.suspect.procedure
            || mutation.target().version != candidate.suspect.version
        {
            return Err(EngineError::InvalidInput(
                "simulated receipt mutation target does not match its suspect".into(),
            ));
        }
        let (claimed_model_id, claimed_model_version, assumptions) = match mutation.verification() {
            ReplayVerification::SimulatedExpected {
                model_id,
                model_version,
                assumptions,
                ..
            } => (model_id.clone(), model_version.clone(), assumptions.clone()),
            _ => {
                return Err(EngineError::InvalidInput(
                    "simulated receipt issuance requires an unissued simulated expectation".into(),
                ));
            }
        };
        if claimed_model_id != simulator.model_id()
            || claimed_model_version != simulator.model_version()
        {
            return Err(EngineError::InvalidInput(
                "candidate simulator identity does not match the engine-selected simulator".into(),
            ));
        }
        validate_assumptions(&assumptions)?;

        let source_trace_hash = sha256_digest(&trace)?;
        let mutation_hash = mutation_digest(&mutation)?;
        let simulation_request = SimulatedReplayRequest {
            source_episode,
            source_trace_hash: source_trace_hash.clone(),
            suspect: candidate.suspect,
            mutation: mutation.clone(),
            model_id: simulator.model_id().to_owned(),
            model_version: simulator.model_version().to_owned(),
            assumptions: assumptions.clone(),
            step_budget,
        };
        let observation = simulator
            .simulate(simulation_request)
            .map_err(|error| EngineError::InvalidInput(format!("simulator failed: {error}")))?;
        if observation.steps_used > step_budget {
            return Err(EngineError::InvalidInput(format!(
                "simulator exceeded its step budget: used {}, allowed {step_budget}",
                observation.steps_used
            )));
        }
        let receipt = SimulatedReplayReceipt {
            source_episode,
            source_trace_hash,
            suspect: candidate.suspect,
            mutation_hash,
            model_id: simulator.model_id().to_owned(),
            model_version: simulator.model_version().to_owned(),
            assumptions,
            step_budget,
            canonical_expected: canonical_expected.clone(),
            result: observation.result,
            steps_used: observation.steps_used,
            details: observation.details,
        };
        let receipt_id = sha256_digest(&receipt)?;
        self.credit_analyses
            .insert_simulated_receipt(&receipt_id, &receipt)?;
        mutation.set_verification(ReplayVerification::SimulatedReceipt {
            expected: canonical_expected,
            receipt_id,
        });
        candidate.change.replacement = serde_json::to_value(mutation)?;
        Ok(candidate)
    }

    pub fn analyze_failure(
        &self,
        request: FailureAnalysisRequest,
    ) -> Result<FailureAnalysis, EngineError> {
        let request_digest = stable_digest(&request)?;
        let (evidence_digest, evidence_digest_cost, snapshot) =
            self.credit_evidence_digest(&request)?;
        let automatic_key = format!("automatic:{request_digest}:{evidence_digest}");
        self.analyze_failure_with_cost(&automatic_key, request, evidence_digest_cost, snapshot)
    }

    pub fn analyze_failure_idempotent(
        &self,
        idempotency_key: &str,
        request: FailureAnalysisRequest,
    ) -> Result<FailureAnalysis, EngineError> {
        validate_analysis_key(idempotency_key)?;
        let request_json = serde_json::to_string(&request)?;
        let request_digest = stable_digest(&request)?;
        if let Some(stored) =
            self.credit_analyses
                .get_by_key(idempotency_key, &request_digest, &request_json)?
        {
            return Ok(stored);
        }
        let (_, evidence_digest_cost, snapshot) = self.credit_evidence_digest(&request)?;
        self.compute_and_store_analysis(
            idempotency_key,
            request,
            request_json,
            request_digest,
            evidence_digest_cost,
            snapshot,
        )
    }

    fn analyze_failure_with_cost(
        &self,
        idempotency_key: &str,
        request: FailureAnalysisRequest,
        evidence_digest_cost: EvidenceDigestCost,
        snapshot: CreditAggregateSnapshot,
    ) -> Result<FailureAnalysis, EngineError> {
        validate_analysis_key(idempotency_key)?;
        let request_json = serde_json::to_string(&request)?;
        let request_digest = stable_digest(&request)?;
        if let Some(stored) =
            self.credit_analyses
                .get_by_key(idempotency_key, &request_digest, &request_json)?
        {
            return Ok(stored);
        }
        self.compute_and_store_analysis(
            idempotency_key,
            request,
            request_json,
            request_digest,
            evidence_digest_cost,
            snapshot,
        )
    }

    fn compute_and_store_analysis(
        &self,
        idempotency_key: &str,
        request: FailureAnalysisRequest,
        request_json: String,
        request_digest: String,
        evidence_digest_cost: EvidenceDigestCost,
        snapshot: CreditAggregateSnapshot,
    ) -> Result<FailureAnalysis, EngineError> {
        let mut analysis =
            self.compute_failure_analysis(request, evidence_digest_cost, snapshot, 1)?;
        analysis.analysis_id = stable_digest(&(&request_digest, &analysis.analysis_id))?;
        self.credit_analyses.insert_or_get(
            idempotency_key,
            &request_digest,
            &request_json,
            &analysis,
        )
    }

    pub fn get_failure_analysis(
        &self,
        analysis_id: &str,
    ) -> Result<Option<FailureAnalysis>, EngineError> {
        if analysis_id.trim().is_empty() {
            return Err(EngineError::InvalidInput(
                "credit analysis id cannot be empty".into(),
            ));
        }
        self.credit_analyses.get(analysis_id)
    }

    pub fn get_failure_analysis_by_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<FailureAnalysis>, EngineError> {
        validate_analysis_key(idempotency_key)?;
        self.credit_analyses.get_by_key_unchecked(idempotency_key)
    }

    fn credit_evidence_digest(
        &self,
        request: &FailureAnalysisRequest,
    ) -> Result<(String, EvidenceDigestCost, CreditAggregateSnapshot), EngineError> {
        let source = self.episodes.get(request.episode_id)?;
        let mut cost = EvidenceDigestCost {
            source_episode_reads: 1,
            ..EvidenceDigestCost::default()
        };
        let mut referenced_versions = request
            .candidates
            .iter()
            .map(|candidate| (candidate.suspect.procedure, candidate.suspect.version))
            .collect::<HashSet<_>>();
        let mut trace_versions = HashSet::new();
        if let Some(trace_json) = source.execution_trace.as_ref()
            && let Ok(trace) = serde_json::from_value::<ExecTrace>(trace_json.clone())
        {
            cost.trace_steps_scanned = trace.steps.len() as u64;
            trace_versions.extend(
                trace
                    .steps
                    .iter()
                    .filter_map(|step| Some((step.procedure_called?, step.procedure_version?))),
            );
        }
        referenced_versions.extend(trace_versions.iter().copied());
        let mut referenced_versions = referenced_versions.into_iter().collect::<Vec<_>>();
        referenced_versions.sort_by_key(|(procedure, version)| (procedure.0, *version));
        let elements = trace_versions
            .into_iter()
            .map(|(procedure, version)| CreditElementRef { procedure, version })
            .collect::<Vec<_>>();
        let snapshot = self
            .episodes
            .credit_aggregate_summary(&elements, request.episode_id)?;
        cost.element_aggregate_rows_read = snapshot.elements.len() as u64;
        cost.pair_aggregate_rows_read = snapshot.pairs.len() as u64;
        let snapshots = referenced_versions
            .into_iter()
            .map(|(procedure, version)| {
                Ok((
                    procedure,
                    version,
                    self.graph.get_procedure_version(procedure, version)?,
                ))
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        cost.procedure_snapshots_read = snapshots.len() as u64;
        Ok((
            stable_digest(&(&source, &snapshot, snapshots))?,
            cost,
            snapshot,
        ))
    }

    fn compute_failure_analysis(
        &self,
        request: FailureAnalysisRequest,
        evidence_digest_cost: EvidenceDigestCost,
        mut snapshot: CreditAggregateSnapshot,
        analysis_cache_lookups: u64,
    ) -> Result<FailureAnalysis, EngineError> {
        let source_index_maintenance_work_units = snapshot.source_index_maintenance_work_units;
        let (episode, failure_evidence) = selected_failure_evidence(
            &self.episodes,
            request.episode_id,
            request.selected_feedback_id,
        )?;
        if !episode.failed() {
            return Err(EngineError::InvalidInput(format!(
                "episode {} is not a failed evaluated episode",
                episode.id
            )));
        }
        let mut contract = attribute_contract_violations(&episode)?;
        overlay_source_contribution(&mut snapshot, &episode);
        let historical_cost = indexed_historical_evidence_cost(&snapshot);
        let statistical_report = rank_statistical_suspects_from_aggregates(&episode, &snapshot)?;
        let statistical_cost = statistical_report.cost;
        let mut statistical = statistical_report.attributions;
        if !historical_cost.feedback_ids_used.is_empty() || historical_cost.conflicts_excluded > 0 {
            let detail = format!(
                "joined append-only historical feedback [{}]; excluded conflicting episode(s) [{}]",
                historical_cost
                    .feedback_ids_used
                    .iter()
                    .map(Uuid::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                historical_cost
                    .conflicting_episode_ids
                    .iter()
                    .map(EpisodeId::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            for attribution in &mut statistical {
                attribution.provenance.details.push(detail.clone());
            }
        }

        let authoritative_scores = contract.attributions.iter().chain(statistical.iter()).fold(
            HashMap::new(),
            |mut scores, attribution| {
                scores
                    .entry(attribution.suspect)
                    .and_modify(|score: &mut f64| *score = score.max(attribution.score))
                    .or_insert(attribution.score);
                scores
            },
        );
        // External patches are suggestions, never an authority over which
        // element is suspicious or how it ranks.
        let mut candidates = request
            .candidates
            .into_iter()
            .filter(|candidate| authoritative_scores.contains_key(&candidate.suspect))
            .collect::<Vec<_>>();
        for candidate in &mut candidates {
            candidate.prior_score = authoritative_scores[&candidate.suspect];
        }
        candidates.sort_by(|left, right| {
            right
                .prior_score
                .total_cmp(&left.prior_score)
                .then_with(|| left.suspect.trace_step.cmp(&right.suspect.trace_step))
                .then_with(|| left.suspect.procedure.0.cmp(&right.suspect.procedure.0))
                .then_with(|| left.suspect.version.cmp(&right.suspect.version))
                .then_with(|| {
                    stable_digest(&left.change)
                        .unwrap_or_default()
                        .cmp(&stable_digest(&right.change).unwrap_or_default())
                })
        });

        let original_execution_cost =
            if episode.cost.budget_spent.is_finite() && episode.cost.budget_spent > 0.0 {
                episode.cost.budget_spent
            } else {
                f64::from(episode.cost.steps_taken).max(1.0)
            };
        let mut replayer = VersionPinnedReplayer::new(&self.graph, &self.episodes)
            .with_simulated_receipts(&self.credit_analyses)
            .with_selected_feedback(request.selected_feedback_id);
        let mut counterfactual = run_counterfactual_replays(
            &episode,
            &candidates,
            ReplayBudget {
                top_k: request.budget.top_k,
                max_replays: request.budget.max_replays,
                max_steps: request.budget.max_replay_steps,
                total_episode_cost: original_execution_cost,
            },
            &mut replayer,
        )?;
        promote_engine_verified_replays(&self.credit_analyses, &mut counterfactual)?;

        if let FailureEvidenceSource::LateFeedback { feedback_id } = &failure_evidence.source {
            let detail = format!("failure selected from late feedback {feedback_id}");
            for attribution in contract
                .attributions
                .iter_mut()
                .chain(statistical.iter_mut())
                .chain(counterfactual.attributions.iter_mut())
            {
                attribution.provenance.details.push(detail.clone());
            }
        }

        let statistical_work_units = statistical_cost.work_units as f64;
        let evidence_digest_work_units = evidence_digest_cost.work_units();
        let attribution_cost = contract.attribution_cost
            + statistical_work_units
            + counterfactual.attribution_cost
            + evidence_digest_work_units as f64
            + analysis_cache_lookups as f64;
        let total_cost = original_execution_cost + attribution_cost;
        let lifecycle_credit_cost = total_cost + source_index_maintenance_work_units as f64;
        let attribution_cost_ratio = if total_cost > 0.0 {
            attribution_cost / total_cost
        } else {
            0.0
        };
        let mut ranked = contract.attributions.clone();
        ranked.extend(counterfactual.attributions.clone());
        ranked.extend(statistical.clone());
        ranked.sort_by(|left, right| {
            right
                .confidence
                .cmp(&left.confidence)
                .then_with(|| right.score.total_cmp(&left.score))
        });
        let contract_steps = contract.steps_inspected as u64;
        let replay_steps = u64::from(counterfactual.steps_spent);
        let analysis_id = stable_digest(&(
            episode.id,
            &failure_evidence,
            &contract,
            &statistical,
            &counterfactual,
            request.budget,
            historical_cost.feedback_rows_scanned,
            historical_cost.feedback_rows_indexed,
            historical_cost.conflicts_excluded,
            &historical_cost.feedback_ids_scanned,
            &historical_cost.conflicting_episode_ids,
            evidence_digest_cost,
            analysis_cache_lookups,
        ))?;
        Ok(FailureAnalysis {
            analysis_id,
            episode_id: episode.id,
            failure_evidence,
            contract,
            statistical,
            counterfactual,
            ranked,
            cost: FailureAnalysisCost {
                original_execution_cost,
                analysis_cache_lookups,
                evidence_digest_source_episode_reads: evidence_digest_cost.source_episode_reads,
                evidence_digest_history_episodes_scanned: evidence_digest_cost
                    .history_episodes_scanned,
                evidence_digest_feedback_rows_scanned: evidence_digest_cost.feedback_rows_scanned,
                evidence_digest_trace_steps_scanned: evidence_digest_cost.trace_steps_scanned,
                evidence_digest_procedure_snapshots_read: evidence_digest_cost
                    .procedure_snapshots_read,
                evidence_digest_element_aggregate_rows_read: evidence_digest_cost
                    .element_aggregate_rows_read,
                evidence_digest_pair_aggregate_rows_read: evidence_digest_cost
                    .pair_aggregate_rows_read,
                evidence_digest_work_units,
                contract_steps,
                statistical_episodes_scanned: historical_cost.episodes_scanned,
                statistical_work_units,
                statistical_feedback_rows_scanned: historical_cost.feedback_rows_scanned,
                statistical_feedback_rows_indexed: historical_cost.feedback_rows_indexed,
                statistical_conflicts_excluded: historical_cost.conflicts_excluded,
                statistical_history_episodes_used: statistical_cost.history_episodes_used,
                statistical_trace_steps_scanned: statistical_cost.history_trace_steps_scanned,
                statistical_element_exposures: statistical_cost.element_exposures_counted,
                statistical_cooccurrence_pairs: statistical_cost.cooccurrence_pairs_counted,
                statistical_element_aggregate_rows_read: statistical_cost.aggregate_rows_read,
                statistical_pair_aggregate_rows_read: statistical_cost.pair_aggregate_rows_read,
                source_index_maintenance_work_units,
                lifecycle_credit_cost,
                replay_steps,
                attribution_cost,
                total_cost,
                attribution_cost_ratio,
            },
        })
    }
}

pub(crate) fn selected_failure_evidence(
    episodes: &EpisodeStore,
    episode_id: EpisodeId,
    selected_feedback_id: Option<Uuid>,
) -> Result<(Episode, FailureEvidence), EngineError> {
    let mut episode = episodes.get(episode_id)?;
    let source = if let Some(feedback_id) = selected_feedback_id {
        let feedback = episodes
            .list_feedback(episode_id)?
            .into_iter()
            .find(|feedback| feedback.id == feedback_id)
            .ok_or_else(|| {
                EngineError::InvalidInput(format!(
                    "feedback {feedback_id} does not belong to episode {episode_id}"
                ))
            })?;
        episode.observed_result = Some(feedback.observed_result);
        episode.evaluation = Some(feedback.evaluation);
        FailureEvidenceSource::LateFeedback { feedback_id }
    } else {
        FailureEvidenceSource::EpisodeEvaluation
    };
    let evaluation = episode.evaluation.clone().ok_or_else(|| {
        EngineError::InvalidInput(format!(
            "episode {episode_id} has no selected evaluation evidence"
        ))
    })?;
    let observed_result = episode.observed_result.clone();
    Ok((
        episode,
        FailureEvidence {
            source,
            observed_result,
            evaluation,
        },
    ))
}

fn indexed_historical_evidence_cost(snapshot: &CreditAggregateSnapshot) -> HistoricalEvidenceCost {
    HistoricalEvidenceCost {
        episodes_scanned: 0,
        feedback_rows_scanned: 0,
        feedback_rows_indexed: snapshot
            .elements
            .iter()
            .map(|item| item.feedback_rows_scanned)
            .sum(),
        conflicts_excluded: snapshot
            .elements
            .iter()
            .map(|item| item.conflicts_excluded)
            .sum(),
        feedback_ids_scanned: Vec::new(),
        feedback_ids_used: Vec::new(),
        conflicting_episode_ids: Vec::new(),
    }
}

fn overlay_source_contribution(snapshot: &mut CreditAggregateSnapshot, episode: &Episode) {
    let old = snapshot.source_contribution;
    let Some(evaluation) = episode.evaluation.as_ref() else {
        return;
    };
    let desired_weight = match evaluation.tier {
        ekg_core::VerifiabilityTier::Hard => 1.0,
        ekg_core::VerifiabilityTier::Consensus => 0.6,
        ekg_core::VerifiabilityTier::Deferred => 0.2,
    };
    let source_elements = episode
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
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    for aggregate in &mut snapshot.elements {
        if !source_elements.contains(&aggregate.element) {
            continue;
        }
        if old.included {
            aggregate.exposures = aggregate.exposures.saturating_sub(1);
            aggregate.failures = aggregate.failures.saturating_sub(u32::from(old.failed));
            aggregate.weighted_exposure = (aggregate.weighted_exposure - old.weight).max(0.0);
            aggregate.weighted_failures =
                (aggregate.weighted_failures - if old.failed { old.weight } else { 0.0 }).max(0.0);
            aggregate.episode_ids.retain(|id| *id != episode.id);
        }
        aggregate.exposures = aggregate.exposures.saturating_add(1);
        aggregate.failures = aggregate
            .failures
            .saturating_add(u32::from(!evaluation.success));
        aggregate.weighted_exposure += desired_weight;
        if !evaluation.success {
            aggregate.weighted_failures += desired_weight;
        }
        if !aggregate.episode_ids.contains(&episode.id) {
            aggregate.episode_ids.push(episode.id);
            aggregate.episode_ids.sort_by_key(|id| id.0);
        }
    }
    for pair in &mut snapshot.pairs {
        if source_elements.contains(&pair.left) && source_elements.contains(&pair.right) {
            pair.together = pair.together.saturating_sub(u32::from(old.included));
            pair.together = pair.together.saturating_add(1);
        }
    }
    snapshot.source_contribution = ekg_episode::CreditEpisodeContribution {
        included: true,
        failed: !evaluation.success,
        weight: desired_weight,
    };
}

fn promote_engine_verified_replays(
    receipts: &CreditAnalysisStore,
    report: &mut CounterfactualReport,
) -> Result<(), EngineError> {
    for attribution in &mut report.attributions {
        let deterministic = attribution.evidence.iter().any(|evidence| {
            matches!(
                evidence,
                AttributionEvidence::Replay {
                    mode: CounterfactualMode::Deterministic,
                    counterfactual_succeeded: Some(true),
                    provenance: ReplayProvenance {
                        source_trace_hash: Some(source),
                        mutation_hash: Some(mutation),
                        verification: Some(ReplayVerificationProvenance::Deterministic { verifier }),
                    },
                    ..
                } if !source.is_empty()
                    && !mutation.is_empty()
                    && verifier.starts_with("engine:canonical_prediction:")
            )
        });
        if deterministic {
            attribution.confidence = AttributionConfidence::Certain;
            attribution.score = 1.0;
            attribution.decisive = true;
            attribution.provenance.details.push(
                "engine verified immutable oracle identity and exact unmodified baseline".into(),
            );
            continue;
        }

        let simulated_receipt_id =
            attribution
                .evidence
                .iter()
                .find_map(|evidence| match evidence {
                    AttributionEvidence::Replay {
                        mode: CounterfactualMode::Simulated,
                        counterfactual_succeeded: Some(true),
                        provenance:
                            ReplayProvenance {
                                source_trace_hash: Some(source),
                                mutation_hash: Some(mutation),
                                verification:
                                    Some(ReplayVerificationProvenance::Simulated {
                                        receipt_id: Some(receipt_id),
                                        model_id,
                                        model_version,
                                        ..
                                    }),
                            },
                        ..
                    } if !source.is_empty()
                        && !mutation.is_empty()
                        && !model_id.is_empty()
                        && !model_version.is_empty() =>
                    {
                        Some(receipt_id.clone())
                    }
                    _ => None,
                });
        if let Some(receipt_id) = simulated_receipt_id
            && receipts.get_simulated_receipt(&receipt_id)?.is_some()
        {
            attribution.confidence = AttributionConfidence::Medium;
            attribution.score = attribution.score.max(0.75);
            attribution.decisive = false;
            attribution.limitations.retain(|limitation| {
                !matches!(
                    limitation,
                    ekg_credit::AttributionLimitation::UnverifiedReplayProvenance { .. }
                )
            });
            attribution.provenance.details.push(format!(
                "engine resolved immutable bounded simulator receipt {receipt_id}; model evidence remains non-decisive"
            ));
        }
    }
    Ok(())
}

fn contract_statuses_changed(source: &ContractChecks, counterfactual: &ContractChecks) -> bool {
    source
        .requires
        .iter()
        .map(|check| check.status)
        .ne(counterfactual.requires.iter().map(|check| check.status))
        || source
            .promises
            .iter()
            .map(|check| check.status)
            .ne(counterfactual.promises.iter().map(|check| check.status))
        || source
            .fails_when
            .iter()
            .map(|check| check.status)
            .ne(counterfactual.fails_when.iter().map(|check| check.status))
}

fn mutation_digest(mutation: &CounterfactualMutation) -> Result<String, EngineError> {
    match mutation {
        CounterfactualMutation::ReplaceBody { target, body, .. } => {
            stable_digest(&("replace_body", target, body))
        }
        CounterfactualMutation::ReplaceContract {
            target, contract, ..
        } => stable_digest(&("replace_contract", target, contract)),
    }
}

fn validate_simulator_identity(value: &str, label: &str) -> Result<(), EngineError> {
    if value.trim().is_empty() || value.len() > 256 {
        return Err(EngineError::InvalidInput(format!(
            "simulator {label} must contain 1 to 256 bytes"
        )));
    }
    Ok(())
}

fn validate_assumptions(assumptions: &[String]) -> Result<(), EngineError> {
    if assumptions.len() > 64
        || assumptions
            .iter()
            .any(|assumption| assumption.trim().is_empty() || assumption.len() > 1_024)
    {
        return Err(EngineError::InvalidInput(
            "simulator assumptions must contain at most 64 nonempty entries of at most 1024 bytes"
                .into(),
        ));
    }
    Ok(())
}

fn sha256_digest(value: &impl Serialize) -> Result<String, EngineError> {
    let bytes = serde_json::to_vec(value)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn stable_digest(value: &impl Serialize) -> Result<String, EngineError> {
    let bytes = serde_json::to_vec(value)?;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(format!("fnv1a64:{hash:016x}"))
}

fn validate_analysis_key(key: &str) -> Result<(), EngineError> {
    if key.trim().is_empty() || key.len() > 256 {
        return Err(EngineError::InvalidInput(
            "credit analysis idempotency key must contain 1 to 256 bytes".into(),
        ));
    }
    Ok(())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
