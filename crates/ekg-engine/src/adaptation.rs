use std::collections::{BTreeMap, HashMap, HashSet};

use ekg_adapt::{
    AdaptationPolicy, ApplyOutcome, AttributionStrength, Claim, Contradiction, ContradictionId,
    CorrectionAction, CorrectionApplier, CorrectionDecision, CorrectionRequest, CorrectionTarget,
    DemonstratedFeature, EvidenceGate, GraphAlternativeSupport, KnowledgeRef, MutationAuthorizer,
    PromotionGate, PromotionReplay, ReconciliationApplier, ReconciliationOutcome,
    ReconciliationPlan, ReconciliationPlanner, Refinement, StagedReconciliation, Uncertainty,
};
use ekg_core::{
    ConceptId, Condition, Episode, EpisodeId, Lifecycle, MutabilityClass, Procedure, ProcedureId,
    RelationshipId, Value, VerifiabilityTier,
};
use ekg_credit::{Attribution, AttributionMechanism, Suspect};
use ekg_exec::{Evaluator, ExecTrace};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    Engine, EngineError, FailureAnalysisRequest, RegressionSuiteCaseResult,
    RegressionSuiteCaseStatus, RegressionSuiteVerdict,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AdaptationPlanId(pub Uuid);

impl AdaptationPlanId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdaptationEvidenceRef {
    pub episode_id: EpisodeId,
    #[serde(default)]
    pub selected_feedback_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttributionSelector {
    pub suspect: Suspect,
    pub mechanism: AttributionMechanism,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AdaptationTarget {
    UnusualInput {
        reason: String,
    },
    Assumption {
        key: String,
        replacement: Value,
    },
    ProcedureScope {
        procedure_id: ProcedureId,
        expected_version: u32,
        condition: Condition,
        learned_from: EpisodeId,
    },
    ProcedureReplacement {
        incumbent_id: ProcedureId,
        incumbent_version: u32,
        challenger: Box<Procedure>,
    },
    ConceptRevision {
        concept_id: ConceptId,
        expected_version: u32,
        revised_description: String,
    },
}

impl AdaptationTarget {
    fn correction_target(&self) -> CorrectionTarget {
        match self {
            Self::UnusualInput { reason } => CorrectionTarget::UnusualInput {
                reason: reason.clone(),
            },
            Self::Assumption { key, replacement } => CorrectionTarget::Assumption {
                key: key.clone(),
                replacement: replacement.clone(),
            },
            Self::ProcedureScope {
                procedure_id,
                expected_version,
                condition,
                learned_from,
            } => CorrectionTarget::ProcedureScope {
                procedure_id: *procedure_id,
                expected_version: *expected_version,
                condition: condition.clone(),
                learned_from: *learned_from,
            },
            Self::ProcedureReplacement {
                incumbent_id,
                incumbent_version,
                challenger,
            } => CorrectionTarget::ProcedureReplacement {
                incumbent_id: *incumbent_id,
                incumbent_version: *incumbent_version,
                challenger: challenger.clone(),
            },
            Self::ConceptRevision {
                concept_id,
                expected_version,
                revised_description,
            } => CorrectionTarget::ConceptRevision {
                concept_id: *concept_id,
                expected_version: *expected_version,
                revised_description: revised_description.clone(),
            },
        }
    }

    fn is_broad(&self) -> bool {
        matches!(
            self,
            Self::ProcedureReplacement { .. } | Self::ConceptRevision { .. }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdaptationPlanRequest {
    pub idempotency_key: String,
    pub analysis: FailureAnalysisRequest,
    pub attribution: AttributionSelector,
    pub evidence: Vec<AdaptationEvidenceRef>,
    pub target: AdaptationTarget,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptationEvidenceGate {
    pub verified_episodes: u32,
    pub distinct_sources: u32,
    pub strongest_tier: Option<VerifiabilityTier>,
    pub challenger_beats_incumbent: bool,
    pub corroborated: bool,
    pub offline: bool,
}

impl From<&AdaptationEvidenceGate> for EvidenceGate {
    fn from(value: &AdaptationEvidenceGate) -> Self {
        Self {
            verified_episodes: value.verified_episodes,
            distinct_sources: value.distinct_sources,
            strongest_tier: value.strongest_tier,
            challenger_beats_incumbent: value.challenger_beats_incumbent,
            corroborated: value.corroborated,
            offline: value.offline,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AdaptationAction {
    RecordOnly {
        reason: String,
    },
    FixAssumption {
        key: String,
        replacement: Value,
    },
    NarrowScope {
        procedure_id: ProcedureId,
        expected_version: u32,
        condition: Condition,
        learned_from: EpisodeId,
    },
    ReplaceProcedure {
        incumbent_id: ProcedureId,
        incumbent_version: u32,
        challenger: Box<Procedure>,
    },
    ReviseConceptOffline {
        concept_id: ConceptId,
        expected_version: u32,
        revised_description: String,
        supporting_episodes: u32,
    },
    ScheduleTest {
        reason: String,
    },
}

impl AdaptationAction {
    fn from_correction(value: CorrectionAction) -> Self {
        match value {
            CorrectionAction::RecordOnly { reason } => Self::RecordOnly { reason },
            CorrectionAction::FixAssumption { key, replacement } => {
                Self::FixAssumption { key, replacement }
            }
            CorrectionAction::NarrowScope {
                procedure_id,
                expected_version,
                condition,
                learned_from,
            } => Self::NarrowScope {
                procedure_id,
                expected_version,
                condition,
                learned_from,
            },
            CorrectionAction::ReplaceProcedure {
                incumbent_id,
                incumbent_version,
                challenger,
            } => Self::ReplaceProcedure {
                incumbent_id,
                incumbent_version,
                challenger,
            },
            CorrectionAction::ReviseConceptOffline {
                concept_id,
                expected_version,
                revised_description,
                supporting_episodes,
            } => Self::ReviseConceptOffline {
                concept_id,
                expected_version,
                revised_description,
                supporting_episodes,
            },
            CorrectionAction::ScheduleTest { reason } => Self::ScheduleTest { reason },
        }
    }

    fn correction_action(&self) -> CorrectionAction {
        match self {
            Self::RecordOnly { reason } => CorrectionAction::RecordOnly {
                reason: reason.clone(),
            },
            Self::FixAssumption { key, replacement } => CorrectionAction::FixAssumption {
                key: key.clone(),
                replacement: replacement.clone(),
            },
            Self::NarrowScope {
                procedure_id,
                expected_version,
                condition,
                learned_from,
            } => CorrectionAction::NarrowScope {
                procedure_id: *procedure_id,
                expected_version: *expected_version,
                condition: condition.clone(),
                learned_from: *learned_from,
            },
            Self::ReplaceProcedure {
                incumbent_id,
                incumbent_version,
                challenger,
            } => CorrectionAction::ReplaceProcedure {
                incumbent_id: *incumbent_id,
                incumbent_version: *incumbent_version,
                challenger: challenger.clone(),
            },
            Self::ReviseConceptOffline {
                concept_id,
                expected_version,
                revised_description,
                supporting_episodes,
            } => CorrectionAction::ReviseConceptOffline {
                concept_id: *concept_id,
                expected_version: *expected_version,
                revised_description: revised_description.clone(),
                supporting_episodes: *supporting_episodes,
            },
            Self::ScheduleTest { reason } => CorrectionAction::ScheduleTest {
                reason: reason.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationScope {
    NoGraphChange,
    OnlineNarrow,
    OfflineBroad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AdaptationKnowledgeRef {
    Concept { id: ConceptId },
    Procedure { id: ProcedureId },
    Relationship { id: RelationshipId },
}

impl From<KnowledgeRef> for AdaptationKnowledgeRef {
    fn from(value: KnowledgeRef) -> Self {
        match value {
            KnowledgeRef::Concept(id) => Self::Concept { id },
            KnowledgeRef::Procedure(id) => Self::Procedure { id },
            KnowledgeRef::Relationship(id) => Self::Relationship { id },
        }
    }
}

impl From<AdaptationKnowledgeRef> for KnowledgeRef {
    fn from(value: AdaptationKnowledgeRef) -> Self {
        match value {
            AdaptationKnowledgeRef::Concept { id } => Self::Concept(id),
            AdaptationKnowledgeRef::Procedure { id } => Self::Procedure(id),
            AdaptationKnowledgeRef::Relationship { id } => Self::Relationship(id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdaptationReconciliationOutcome {
    PreservedByAlternativeSupport,
    MarkStale,
    MarkUnderReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptationReconciliationEntry {
    pub knowledge: AdaptationKnowledgeRef,
    pub depth: usize,
    pub expected_version: u32,
    pub previous_lifecycle: Lifecycle,
    pub next_lifecycle: Lifecycle,
    pub outcome: AdaptationReconciliationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptationReconciliationPlan {
    pub changed: AdaptationKnowledgeRef,
    pub entries: Vec<AdaptationReconciliationEntry>,
}

impl From<ReconciliationPlan> for AdaptationReconciliationPlan {
    fn from(value: ReconciliationPlan) -> Self {
        Self {
            changed: value.changed.into(),
            entries: value
                .entries
                .into_iter()
                .map(|entry| AdaptationReconciliationEntry {
                    knowledge: entry.knowledge.into(),
                    depth: entry.depth,
                    expected_version: entry.expected_version,
                    previous_lifecycle: entry.previous_lifecycle,
                    next_lifecycle: entry.next_lifecycle,
                    outcome: match entry.outcome {
                        ReconciliationOutcome::PreservedByAlternativeSupport => {
                            AdaptationReconciliationOutcome::PreservedByAlternativeSupport
                        }
                        ReconciliationOutcome::MarkStale => {
                            AdaptationReconciliationOutcome::MarkStale
                        }
                        ReconciliationOutcome::MarkUnderReview => {
                            AdaptationReconciliationOutcome::MarkUnderReview
                        }
                    },
                })
                .collect(),
        }
    }
}

impl From<AdaptationReconciliationPlan> for ReconciliationPlan {
    fn from(value: AdaptationReconciliationPlan) -> Self {
        Self {
            changed: value.changed.into(),
            entries: value
                .entries
                .into_iter()
                .map(|entry| ekg_adapt::ReconciliationEntry {
                    knowledge: entry.knowledge.into(),
                    depth: entry.depth,
                    expected_version: entry.expected_version,
                    previous_lifecycle: entry.previous_lifecycle,
                    next_lifecycle: entry.next_lifecycle,
                    outcome: match entry.outcome {
                        AdaptationReconciliationOutcome::PreservedByAlternativeSupport => {
                            ReconciliationOutcome::PreservedByAlternativeSupport
                        }
                        AdaptationReconciliationOutcome::MarkStale => {
                            ReconciliationOutcome::MarkStale
                        }
                        AdaptationReconciliationOutcome::MarkUnderReview => {
                            ReconciliationOutcome::MarkUnderReview
                        }
                    },
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptationPlan {
    pub id: AdaptationPlanId,
    pub idempotency_key: String,
    pub analysis_episode_id: EpisodeId,
    pub attribution: Attribution,
    pub evidence: Vec<AdaptationEvidenceRef>,
    pub evidence_gate: AdaptationEvidenceGate,
    pub target: AdaptationTarget,
    pub action: AdaptationAction,
    pub rationale: String,
    pub mutation_scope: MutationScope,
    pub reconciliation: Option<AdaptationReconciliationPlan>,
    pub created_at: i64,
}

#[derive(Debug)]
pub struct OfflineCapability {
    lease: crate::runtime::MaintenanceLease,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyAdaptationRequest {
    pub plan_id: AdaptationPlanId,
    pub idempotency_key: String,
    pub applied_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AdaptationOutcome {
    NoGraphChange,
    ProcedureUpdated {
        procedure_id: ProcedureId,
        previous_version: u32,
        current_version: u32,
    },
    ConceptUpdated {
        concept_id: ConceptId,
    },
}

impl From<ApplyOutcome> for AdaptationOutcome {
    fn from(value: ApplyOutcome) -> Self {
        match value {
            ApplyOutcome::NoGraphChange => Self::NoGraphChange,
            ApplyOutcome::ProcedureUpdated {
                procedure_id,
                previous_version,
                current_version,
            } => Self::ProcedureUpdated {
                procedure_id,
                previous_version,
                current_version,
            },
            ApplyOutcome::ConceptUpdated { concept_id } => Self::ConceptUpdated { concept_id },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptationReconciliationReceipt {
    pub updated: Vec<AdaptationKnowledgeRef>,
    pub preserved: Vec<AdaptationKnowledgeRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptationReceipt {
    pub plan_id: AdaptationPlanId,
    pub idempotency_key: String,
    pub outcome: AdaptationOutcome,
    pub reconciliation: Option<AdaptationReconciliationReceipt>,
    pub evidence: Vec<AdaptationEvidenceRef>,
    pub applied_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptationRecord {
    pub plan: AdaptationPlan,
    pub receipt: Option<AdaptationReceipt>,
    /// The immutable full-suite gate for a broad plan, including rejected
    /// attempts. Narrow/local corrections intentionally do not require it.
    pub regression_suite: Option<RegressionSuiteVerdict>,
}

/// Durable authorization and progress journal for a multi-store adaptation.
///
/// The stage is inserted before the primary mutation. If the process stops
/// after any later commit, retry can prove that the broad capability was
/// already consumed and recover the exact remaining work without minting new
/// authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdaptationApplyStage {
    request: ApplyAdaptationRequest,
    outcome: Option<ApplyOutcome>,
    reconciliation_complete: bool,
    reconciliation: Option<AdaptationReconciliationReceipt>,
}

pub(crate) struct AdaptationStore {
    conn: Connection,
}

impl AdaptationStore {
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
            "CREATE TABLE IF NOT EXISTS engine_adaptation_plans (
                id TEXT PRIMARY KEY,
                idempotency_key TEXT NOT NULL UNIQUE,
                request_json TEXT NOT NULL,
                plan_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS engine_adaptation_receipts (
                plan_id TEXT PRIMARY KEY,
                idempotency_key TEXT NOT NULL UNIQUE,
                receipt_json TEXT NOT NULL,
                applied_at INTEGER NOT NULL,
                FOREIGN KEY (plan_id) REFERENCES engine_adaptation_plans(id)
             );
             CREATE TABLE IF NOT EXISTS engine_adaptation_apply_stages (
                plan_id TEXT PRIMARY KEY,
                idempotency_key TEXT NOT NULL UNIQUE,
                request_json TEXT NOT NULL,
                stage_json TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                FOREIGN KEY (plan_id) REFERENCES engine_adaptation_plans(id)
             );
             CREATE TABLE IF NOT EXISTS engine_assumption_overrides (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                source_plan_id TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (source_plan_id) REFERENCES engine_adaptation_plans(id)
             );
             CREATE TABLE IF NOT EXISTS engine_adaptation_regression_verdicts (
                plan_id TEXT PRIMARY KEY,
                verdict_json TEXT NOT NULL,
                recorded_at INTEGER NOT NULL,
                FOREIGN KEY (plan_id) REFERENCES engine_adaptation_plans(id)
             );",
        )?;
        Ok(())
    }

    fn by_key(&self, key: &str) -> Result<Option<(String, AdaptationPlan)>, EngineError> {
        self.conn
            .query_row(
                "SELECT request_json, plan_json FROM engine_adaptation_plans WHERE idempotency_key = ?1",
                params![key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .map(|(request, plan)| Ok((request, serde_json::from_str(&plan)?)))
            .transpose()
    }

    fn insert_plan(&self, request_json: &str, plan: &AdaptationPlan) -> Result<(), EngineError> {
        self.conn.execute(
            "INSERT INTO engine_adaptation_plans
                (id, idempotency_key, request_json, plan_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                plan.id.0.to_string(),
                plan.idempotency_key,
                request_json,
                serde_json::to_string(plan)?,
                plan.created_at,
            ],
        )?;
        Ok(())
    }

    fn get_plan(&self, id: AdaptationPlanId) -> Result<Option<AdaptationPlan>, EngineError> {
        self.conn
            .query_row(
                "SELECT plan_json FROM engine_adaptation_plans WHERE id = ?1",
                params![id.0.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }

    fn get_receipt(&self, id: AdaptationPlanId) -> Result<Option<AdaptationReceipt>, EngineError> {
        self.conn
            .query_row(
                "SELECT receipt_json FROM engine_adaptation_receipts WHERE plan_id = ?1",
                params![id.0.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }

    fn regression_verdict(
        &self,
        id: AdaptationPlanId,
    ) -> Result<Option<RegressionSuiteVerdict>, EngineError> {
        self.conn
            .query_row(
                "SELECT verdict_json FROM engine_adaptation_regression_verdicts WHERE plan_id = ?1",
                params![id.0.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }

    /// Verdicts are immutable evidence. If a restart retries the same pinned
    /// plan, it must use the first report rather than quietly replacing it.
    fn record_regression_verdict(
        &self,
        plan_id: AdaptationPlanId,
        verdict: &RegressionSuiteVerdict,
        recorded_at: i64,
    ) -> Result<RegressionSuiteVerdict, EngineError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO engine_adaptation_regression_verdicts
                (plan_id, verdict_json, recorded_at) VALUES (?1, ?2, ?3)",
            params![
                plan_id.0.to_string(),
                serde_json::to_string(verdict)?,
                recorded_at,
            ],
        )?;
        self.regression_verdict(plan_id)?.ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "regression verdict for adaptation plan {} disappeared",
                plan_id.0
            ))
        })
    }

    fn receipt_by_key(&self, key: &str) -> Result<Option<AdaptationReceipt>, EngineError> {
        self.conn
            .query_row(
                "SELECT receipt_json FROM engine_adaptation_receipts WHERE idempotency_key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }

    fn insert_receipt(&self, receipt: &AdaptationReceipt) -> Result<(), EngineError> {
        self.conn.execute(
            "INSERT INTO engine_adaptation_receipts
                (plan_id, idempotency_key, receipt_json, applied_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                receipt.plan_id.0.to_string(),
                receipt.idempotency_key,
                serde_json::to_string(receipt)?,
                receipt.applied_at,
            ],
        )?;
        Ok(())
    }

    fn persist_assumption_override(
        &self,
        key: &str,
        value: &Value,
        source_plan_id: AdaptationPlanId,
        updated_at: i64,
    ) -> Result<(), EngineError> {
        self.conn.execute(
            "INSERT INTO engine_assumption_overrides
                (key, value_json, source_plan_id, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                source_plan_id = excluded.source_plan_id,
                updated_at = excluded.updated_at",
            params![
                key,
                serde_json::to_string(value)?,
                source_plan_id.0.to_string(),
                updated_at,
            ],
        )?;
        Ok(())
    }

    fn assumption_override(&self, key: &str) -> Result<Option<Value>, EngineError> {
        self.conn
            .query_row(
                "SELECT value_json FROM engine_assumption_overrides WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }

    fn assumption_overrides(&self) -> Result<HashMap<String, Value>, EngineError> {
        let mut statement = self
            .conn
            .prepare("SELECT key, value_json FROM engine_assumption_overrides ORDER BY key")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (key, json) = row?;
            Ok((key, serde_json::from_str(&json)?))
        })
        .collect()
    }

    fn get_stage(
        &self,
        plan_id: AdaptationPlanId,
    ) -> Result<Option<AdaptationApplyStage>, EngineError> {
        self.conn
            .query_row(
                "SELECT stage_json FROM engine_adaptation_apply_stages WHERE plan_id = ?1",
                params![plan_id.0.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }

    fn begin_stage(
        &self,
        request: &ApplyAdaptationRequest,
    ) -> Result<AdaptationApplyStage, EngineError> {
        let request_json = serde_json::to_string(request)?;
        if let Some((stored_request, stage_json)) = self
            .conn
            .query_row(
                "SELECT request_json, stage_json FROM engine_adaptation_apply_stages
                 WHERE plan_id = ?1 OR idempotency_key = ?2",
                params![request.plan_id.0.to_string(), request.idempotency_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if stored_request != request_json {
                return Err(EngineError::InvalidInput(format!(
                    "adaptation apply idempotency conflict for key {}",
                    request.idempotency_key
                )));
            }
            return Ok(serde_json::from_str(&stage_json)?);
        }
        let stage = AdaptationApplyStage {
            request: request.clone(),
            outcome: None,
            reconciliation_complete: false,
            reconciliation: None,
        };
        self.conn.execute(
            "INSERT INTO engine_adaptation_apply_stages
                (plan_id, idempotency_key, request_json, stage_json, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                request.plan_id.0.to_string(),
                request.idempotency_key,
                request_json,
                serde_json::to_string(&stage)?,
                request.applied_at,
            ],
        )?;
        Ok(stage)
    }

    fn update_stage(&self, stage: &AdaptationApplyStage) -> Result<(), EngineError> {
        let changed = self.conn.execute(
            "UPDATE engine_adaptation_apply_stages SET stage_json = ?2 WHERE plan_id = ?1",
            params![
                stage.request.plan_id.0.to_string(),
                serde_json::to_string(stage)?,
            ],
        )?;
        if changed != 1 {
            return Err(EngineError::InvalidInput(format!(
                "adaptation apply stage {} disappeared during recovery",
                stage.request.plan_id.0
            )));
        }
        Ok(())
    }

    fn pending_stage_requests(&self) -> Result<Vec<ApplyAdaptationRequest>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT stages.stage_json
             FROM engine_adaptation_apply_stages AS stages
             LEFT JOIN engine_adaptation_receipts AS receipts
               ON receipts.plan_id = stages.plan_id
             WHERE receipts.plan_id IS NULL
             ORDER BY stages.started_at, stages.plan_id",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            let stage: AdaptationApplyStage = serde_json::from_str(&row?)?;
            Ok(stage.request)
        })
        .collect()
    }
}

impl Engine {
    pub fn plan_adaptation(
        &self,
        request: AdaptationPlanRequest,
    ) -> Result<AdaptationPlan, EngineError> {
        if request.idempotency_key.trim().is_empty() {
            return Err(EngineError::InvalidInput(
                "adaptation plan idempotency key must be non-empty".into(),
            ));
        }
        let request_json = serde_json::to_string(&request)?;
        if let Some((stored_request, plan)) = self.adaptations.by_key(&request.idempotency_key)? {
            if stored_request != request_json {
                return Err(EngineError::InvalidInput(format!(
                    "adaptation idempotency conflict for key {}",
                    request.idempotency_key
                )));
            }
            return Ok(plan);
        }

        let analysis = self.analyze_failure(request.analysis.clone())?;
        let attribution = analysis
            .ranked
            .iter()
            .filter(|candidate| {
                candidate.suspect == request.attribution.suspect
                    && candidate.mechanism == request.attribution.mechanism
            })
            .max_by(|left, right| {
                left.confidence
                    .cmp(&right.confidence)
                    .then_with(|| left.score.total_cmp(&right.score))
            })
            .cloned()
            .ok_or_else(|| {
                EngineError::InvalidInput(
                    "selected attribution is absent from the trusted failure analysis".into(),
                )
            })?;

        let gate = self.derive_evidence_gate(&request, &attribution)?;
        let decision = AdaptationPolicy::decide(CorrectionRequest {
            attribution: AttributionStrength::from(&attribution),
            target: request.target.correction_target(),
            evidence: (&gate).into(),
        });
        let action = AdaptationAction::from_correction(decision.action);
        let mutation_scope = mutation_scope(&action);
        let trusted_episode_ids = self.trusted_strong_episode_ids()?;
        let reconciliation = changed_knowledge(&action)
            .map(|changed| {
                ReconciliationPlanner::plan(
                    &self.graph,
                    changed,
                    &GraphAlternativeSupport::new_trusted(&self.episodes, &trusted_episode_ids),
                )
                .map(Into::into)
            })
            .transpose()?;
        let plan = AdaptationPlan {
            id: AdaptationPlanId::new(),
            idempotency_key: request.idempotency_key.clone(),
            analysis_episode_id: request.analysis.episode_id,
            attribution,
            evidence: request.evidence,
            evidence_gate: gate,
            target: request.target,
            action,
            rationale: decision.rationale,
            mutation_scope,
            reconciliation,
            created_at: request.created_at,
        };
        self.adaptations.insert_plan(&request_json, &plan)?;
        Ok(plan)
    }

    pub fn get_adaptation(
        &self,
        id: AdaptationPlanId,
    ) -> Result<Option<AdaptationRecord>, EngineError> {
        let Some(plan) = self.adaptations.get_plan(id)? else {
            return Ok(None);
        };
        Ok(Some(AdaptationRecord {
            plan,
            receipt: self.adaptations.get_receipt(id)?,
            regression_suite: self.adaptations.regression_verdict(id)?,
        }))
    }

    /// Returns the immutable, durable full regression-suite report for a
    /// broad adaptation. A missing report means the plan has never crossed
    /// the broad-mutation gate.
    pub fn adaptation_regression_suite(
        &self,
        id: AdaptationPlanId,
    ) -> Result<Option<RegressionSuiteVerdict>, EngineError> {
        self.adaptations.regression_verdict(id)
    }

    /// Returns the latest durable correction for a named environmental
    /// assumption, if adaptation has established one.
    pub fn assumption_override(&self, key: &str) -> Result<Option<Value>, EngineError> {
        self.adaptations.assumption_override(key)
    }

    /// Returns all durable assumption corrections for context assembly.
    pub fn assumption_overrides(&self) -> Result<HashMap<String, Value>, EngineError> {
        self.adaptations.assumption_overrides()
    }

    pub fn issue_offline_capability(
        &mut self,
        request: &ApplyAdaptationRequest,
    ) -> Result<OfflineCapability, EngineError> {
        self.require_admin()?;
        let record = self.get_adaptation(request.plan_id)?.ok_or_else(|| {
            EngineError::Adapt(ekg_adapt::AdaptError::NotFound(format!(
                "adaptation plan {}",
                request.plan_id.0
            )))
        })?;
        if record.plan.mutation_scope != MutationScope::OfflineBroad {
            return Err(EngineError::InvalidInput(
                "offline authority may only be issued for an existing broad adaptation plan".into(),
            ));
        }
        let request_digest = adaptation_request_digest(request)?;
        let lease = self
            .runtime
            .acquire_maintenance(self.instance_id, &request_digest)?;
        Ok(OfflineCapability { lease })
    }

    pub fn apply_adaptation(
        &mut self,
        request: ApplyAdaptationRequest,
    ) -> Result<AdaptationReceipt, EngineError> {
        self.apply_adaptation_with_capability(request, None)
    }

    pub fn apply_adaptation_offline(
        &mut self,
        request: ApplyAdaptationRequest,
        capability: &OfflineCapability,
    ) -> Result<AdaptationReceipt, EngineError> {
        self.apply_adaptation_with_capability(request, Some(capability))
    }

    fn apply_adaptation_with_capability(
        &mut self,
        request: ApplyAdaptationRequest,
        capability: Option<&OfflineCapability>,
    ) -> Result<AdaptationReceipt, EngineError> {
        if request.idempotency_key.trim().is_empty() {
            return Err(EngineError::InvalidInput(
                "adaptation apply idempotency key must be non-empty".into(),
            ));
        }
        let request_digest = adaptation_request_digest(&request)?;
        if let Some(receipt) = self.adaptations.receipt_by_key(&request.idempotency_key)? {
            if receipt.plan_id != request.plan_id {
                return Err(EngineError::InvalidInput(format!(
                    "adaptation apply idempotency conflict for key {}",
                    request.idempotency_key
                )));
            }
            self.runtime
                .release_owned_completed_maintenance(self.instance_id, &request_digest)?;
            return Ok(receipt);
        }
        let record = self.get_adaptation(request.plan_id)?.ok_or_else(|| {
            EngineError::Adapt(ekg_adapt::AdaptError::NotFound(format!(
                "adaptation plan {}",
                request.plan_id.0
            )))
        })?;
        if let Some(receipt) = record.receipt {
            if receipt.idempotency_key != request.idempotency_key {
                return Err(EngineError::InvalidInput(format!(
                    "adaptation apply idempotency conflict for plan {}",
                    request.plan_id.0
                )));
            }
            self.runtime
                .release_owned_completed_maintenance(self.instance_id, &request_digest)?;
            return Ok(receipt);
        }

        let existing_stage = self.adaptations.get_stage(request.plan_id)?;
        let stage_exists = existing_stage.is_some();
        if record.plan.mutation_scope == MutationScope::OfflineBroad {
            if stage_exists {
                let existing = self.runtime.maintenance_for_request(&request_digest)?;
                match existing {
                    Some(lease) if self.runtime.validate_maintenance(&lease).is_ok() => {
                        if lease.owner != self.instance_id {
                            return Err(EngineError::InvalidInput(
                                "the staged broad adaptation is owned by another live engine instance"
                                    .into(),
                            ));
                        }
                    }
                    _ => {
                        // The durable stage is proof that the original one-shot
                        // authority was consumed before mutation began. An expired
                        // or cleared lease may therefore be reacquired, but normal
                        // cycle exclusion is rechecked atomically.
                        self.runtime
                            .acquire_maintenance(self.instance_id, &request_digest)?;
                    }
                }
            } else {
                self.consume_offline_capability(capability, &request_digest)?;
            }

            // Run the deterministic full suite before writing an apply stage.
            // A rejected candidate has made no mutation, so it must not leave
            // a recoverable stage that would make future Engine startup retry
            // a known-bad broad change forever.
            if existing_stage
                .as_ref()
                .is_none_or(|stage| stage.outcome.is_none())
            {
                let verdict = self.broad_regression_suite(&record.plan, request.applied_at)?;
                if !verdict.accepted {
                    self.runtime
                        .release_owned_completed_maintenance(self.instance_id, &request_digest)?;
                    return Err(regression_suite_rejection(&verdict));
                }
            }
        }
        let mut stage = self.adaptations.begin_stage(&request)?;
        let correction = CorrectionDecision {
            action: record.plan.action.correction_action(),
            rationale: record.plan.rationale.clone(),
        };
        let outcome = match stage.outcome.clone() {
            Some(outcome) => outcome,
            None => {
                let outcome = match record.plan.mutation_scope {
                    MutationScope::NoGraphChange => {
                        if let AdaptationAction::FixAssumption { key, replacement } =
                            &record.plan.action
                        {
                            self.adaptations.persist_assumption_override(
                                key,
                                replacement,
                                record.plan.id,
                                request.applied_at,
                            )?;
                        }
                        ApplyOutcome::NoGraphChange
                    }
                    MutationScope::OnlineNarrow => {
                        let procedure = match &record.plan.action {
                            AdaptationAction::NarrowScope {
                                procedure_id,
                                expected_version,
                                ..
                            } => self
                                .graph
                                .get_procedure_version(*procedure_id, *expected_version)?
                                .ok_or_else(|| {
                                    EngineError::InvalidInput(format!(
                                        "procedure {procedure_id} v{expected_version} not found"
                                    ))
                                })?,
                            _ => {
                                return Err(EngineError::InvalidInput(
                                    "online-narrow plan does not contain a scope correction".into(),
                                ));
                            }
                        };
                        let trusted_regressions = self.trusted_strong_episode_ids()?;
                        let authorization =
                            MutationAuthorizer::authorize_for_procedure_with_trusted_regressions(
                                &self.episodes,
                                &procedure,
                                &correction,
                                &record.plan.attribution,
                                &trusted_regressions,
                            )?;
                        match CorrectionApplier::apply(
                            &self.graph,
                            &authorization,
                            request.applied_at,
                        ) {
                            Ok(outcome) => outcome,
                            Err(error) => self.recover_narrow_apply(&record.plan.action, error)?,
                        }
                    }
                    MutationScope::OfflineBroad => {
                        self.apply_broad_action(&record.plan.action, request.applied_at)?
                    }
                };
                stage.outcome = Some(outcome.clone());
                self.adaptations.update_stage(&stage)?;
                outcome
            }
        };

        let reconciliation = if stage.reconciliation_complete {
            stage.reconciliation.clone()
        } else {
            let reconciliation = if !matches!(outcome, ApplyOutcome::NoGraphChange) {
                record
                    .plan
                    .reconciliation
                    .clone()
                    .map(|plan| {
                        self.apply_reconciliation_with_refresh(
                            &record.plan,
                            plan,
                            request.applied_at,
                        )
                    })
                    .transpose()?
            } else {
                None
            };
            stage.reconciliation_complete = true;
            stage.reconciliation = reconciliation.clone();
            self.adaptations.update_stage(&stage)?;
            reconciliation
        };
        let receipt = AdaptationReceipt {
            plan_id: record.plan.id,
            idempotency_key: request.idempotency_key,
            outcome: outcome.into(),
            reconciliation,
            evidence: record.plan.evidence,
            applied_at: request.applied_at,
        };
        self.adaptations.insert_receipt(&receipt)?;
        if record.plan.mutation_scope == MutationScope::OfflineBroad {
            self.runtime
                .release_owned_completed_maintenance(self.instance_id, &request_digest)?;
        }
        Ok(receipt)
    }

    /// Resumes every authorized adaptation whose final receipt was not written.
    /// Callers should invoke this during engine startup before accepting work.
    pub fn recover_pending_adaptations(&mut self) -> Result<Vec<AdaptationReceipt>, EngineError> {
        let requests = self.adaptations.pending_stage_requests()?;
        requests
            .into_iter()
            .map(|request| self.apply_adaptation_with_capability(request, None))
            .collect()
    }

    fn apply_reconciliation_with_refresh(
        &self,
        adaptation: &AdaptationPlan,
        planned: AdaptationReconciliationPlan,
        applied_at: i64,
    ) -> Result<AdaptationReconciliationReceipt, EngineError> {
        let apply = |plan: AdaptationReconciliationPlan| {
            let staged = StagedReconciliation::new(
                format!("adaptation:{}:reconciliation", adaptation.id.0),
                plan.into(),
                applied_at,
            )?;
            let result = ReconciliationApplier::apply(&self.graph, &staged)?;
            Ok::<_, EngineError>(AdaptationReconciliationReceipt {
                updated: result.updated.into_iter().map(Into::into).collect(),
                preserved: result.preserved.into_iter().map(Into::into).collect(),
            })
        };
        match apply(planned) {
            Ok(receipt) => Ok(receipt),
            Err(EngineError::Adapt(ekg_adapt::AdaptError::Graph(
                ekg_graph::GraphError::RevisionConflict { .. },
            ))) => {
                let changed = changed_knowledge(&adaptation.action).ok_or_else(|| {
                    EngineError::InvalidInput(
                        "mutating adaptation has no reconciliation root".into(),
                    )
                })?;
                let refreshed = ReconciliationPlanner::plan(
                    &self.graph,
                    changed,
                    &GraphAlternativeSupport::new_trusted(
                        &self.episodes,
                        &self.trusted_strong_episode_ids()?,
                    ),
                )?;
                apply(refreshed.into())
            }
            Err(error) => Err(error),
        }
    }

    pub fn admin_record_contradiction(
        &self,
        left: Claim,
        right: Claim,
        created_at: i64,
    ) -> Result<Contradiction, EngineError> {
        self.require_admin()?;
        self.validate_trusted_claim(&left)?;
        self.validate_trusted_claim(&right)?;
        Ok(self
            .contradictions
            .record(left, right, &self.episodes, created_at)?)
    }

    pub fn get_contradiction(
        &self,
        id: ContradictionId,
    ) -> Result<Option<Contradiction>, EngineError> {
        Ok(self.contradictions.get(id)?)
    }

    pub fn list_held_contradictions(&self) -> Result<Vec<Contradiction>, EngineError> {
        Ok(self.contradictions.list_held()?)
    }

    pub fn admin_add_claim_dependency(
        &self,
        dependent_claim_id: &str,
        support_claim_id: &str,
    ) -> Result<(), EngineError> {
        self.require_admin()?;
        if dependent_claim_id.trim().is_empty() || support_claim_id.trim().is_empty() {
            return Err(EngineError::InvalidInput(
                "claim dependency identifiers must be non-empty".into(),
            ));
        }
        if dependent_claim_id == support_claim_id {
            return Err(EngineError::InvalidInput(
                "a claim cannot depend on itself".into(),
            ));
        }
        if !self.claim_identifier_exists(dependent_claim_id)?
            || !self.claim_identifier_exists(support_claim_id)?
        {
            return Err(EngineError::InvalidInput(
                "both sides of a claim dependency must identify recorded claims or predicates"
                    .into(),
            ));
        }
        Ok(self
            .contradictions
            .add_claim_dependency(dependent_claim_id, support_claim_id)?)
    }

    fn claim_identifier_exists(&self, claim_id: &str) -> Result<bool, EngineError> {
        if self.contradictions.contains_claim_identifier(claim_id)? {
            return Ok(true);
        }
        if let Some(raw) = claim_id.strip_prefix("concept:")
            && let Ok(id) = uuid::Uuid::parse_str(raw)
        {
            return Ok(self.graph.get_concept(ekg_core::ConceptId(id))?.is_some());
        }
        if let Some(raw) = claim_id
            .strip_prefix("procedure:")
            .and_then(|value| value.strip_suffix(":result"))
            && let Ok(id) = uuid::Uuid::parse_str(raw)
        {
            return Ok(self
                .graph
                .get_procedure(ekg_core::ProcedureId(id))?
                .is_some());
        }
        Ok(false)
    }

    pub fn uncertainty_for_claim(&self, claim_id: &str) -> Result<Uncertainty, EngineError> {
        Ok(self.contradictions.uncertainty_for_claim(claim_id)?)
    }

    pub fn held_contradictions_for_predicate(
        &self,
        predicate: &str,
    ) -> Result<Vec<ContradictionId>, EngineError> {
        Ok(self.contradictions.held_for_predicate(predicate)?)
    }

    pub fn refinement_context_for_predicate(
        &self,
        predicate: &str,
        environment: &std::collections::BTreeMap<String, Value>,
    ) -> Result<ekg_adapt::PredicateRefinementContext, EngineError> {
        Ok(self
            .contradictions
            .refinement_context_for_predicate(predicate, environment)?)
    }

    pub fn refinements_for_claim(&self, claim_id: &str) -> Result<Vec<Refinement>, EngineError> {
        Ok(self.contradictions.refinements_for_claim(claim_id)?)
    }

    pub fn admin_refine_contradiction(
        &self,
        id: ContradictionId,
        discriminator: DemonstratedFeature,
        updated_at: i64,
    ) -> Result<Refinement, EngineError> {
        self.require_admin()?;
        let contradiction = self
            .contradictions
            .get(id)?
            .ok_or_else(|| ekg_adapt::AdaptError::NotFound(format!("contradiction {}", id.0)))?;
        self.validate_trusted_claim(&contradiction.left)?;
        self.validate_trusted_claim(&contradiction.right)?;
        Ok(self
            .contradictions
            .refine(id, discriminator, &self.episodes, updated_at)?)
    }

    fn validate_trusted_claim(&self, claim: &Claim) -> Result<(), EngineError> {
        for episode_id in &claim.supporting_episodes {
            let episode = self.episodes.get(*episode_id)?;
            if self.trust.receipt_for_episode(&episode)?.is_none() {
                return Err(EngineError::Adapt(ekg_adapt::AdaptError::Unauthorized(
                    format!(
                        "claim evidence episode {episode_id} lacks an exact Engine trust receipt"
                    ),
                )));
            }
        }
        Ok(())
    }

    fn trusted_strong_episode_ids(&self) -> Result<HashSet<EpisodeId>, EngineError> {
        let mut trusted = HashSet::new();
        for episode in self.episodes.list_recent(u32::MAX)? {
            let Some(evaluation) = episode.evaluation.as_ref() else {
                continue;
            };
            if matches!(
                evaluation.tier,
                VerifiabilityTier::Hard | VerifiabilityTier::Consensus
            ) && self
                .trust
                .verified_engine_episode(&episode, evaluation.tier)?
                .is_some()
            {
                trusted.insert(episode.id);
            }
        }
        Ok(trusted)
    }

    fn consume_offline_capability(
        &self,
        capability: Option<&OfflineCapability>,
        request_digest: &str,
    ) -> Result<(), EngineError> {
        let Some(capability) = capability else {
            return Err(EngineError::Adapt(
                ekg_adapt::AdaptError::OfflineCapabilityRequired(
                    "broad adaptation requires a valid engine-issued offline capability".into(),
                ),
            ));
        };
        if capability.lease.owner != self.instance_id
            || capability.lease.request_digest != request_digest
        {
            return Err(EngineError::Adapt(
                ekg_adapt::AdaptError::OfflineCapabilityRequired(
                    "offline capability is not bound to this engine and exact apply request".into(),
                ),
            ));
        }
        self.runtime.validate_maintenance(&capability.lease)?;
        Ok(())
    }

    fn derive_evidence_gate(
        &self,
        request: &AdaptationPlanRequest,
        attribution: &Attribution,
    ) -> Result<AdaptationEvidenceGate, EngineError> {
        if request.evidence.is_empty() {
            return Err(EngineError::InvalidInput(
                "adaptation planning requires canonical episode evidence".into(),
            ));
        }
        let analyzed = AdaptationEvidenceRef {
            episode_id: request.analysis.episode_id,
            selected_feedback_id: request.analysis.selected_feedback_id,
        };
        if !request.evidence.contains(&analyzed) {
            return Err(EngineError::InvalidInput(
                "adaptation evidence must include the analyzed failure evidence".into(),
            ));
        }
        let mut unique = HashSet::new();
        let mut sources = HashSet::new();
        let mut verified_episode_ids = HashSet::new();
        let mut strongest_tier = None;
        let mut episodes = HashMap::<EpisodeId, Episode>::new();
        for evidence in &request.evidence {
            if !unique.insert((evidence.episode_id, evidence.selected_feedback_id)) {
                return Err(EngineError::InvalidInput(
                    "adaptation evidence contains a duplicate reference".into(),
                ));
            }
            let (episode, selected) = crate::credit::selected_failure_evidence(
                &self.episodes,
                evidence.episode_id,
                evidence.selected_feedback_id,
            )?;
            let trace: ExecTrace = episode
                .execution_trace
                .clone()
                .ok_or_else(|| EngineError::MissingTrace(episode.id))
                .and_then(|trace| serde_json::from_value(trace).map_err(EngineError::from))?;
            if !trace.steps.iter().any(|step| {
                step.procedure_called == Some(attribution.suspect.procedure)
                    && step.procedure_version == Some(attribution.suspect.version)
            }) {
                return Err(EngineError::InvalidInput(format!(
                    "evidence episode {} does not contain attributed procedure {} v{}",
                    episode.id, attribution.suspect.procedure, attribution.suspect.version
                )));
            }
            let trusted = if let Some(feedback_id) = evidence.selected_feedback_id {
                let feedback = self
                    .episodes
                    .list_feedback(evidence.episode_id)?
                    .into_iter()
                    .find(|feedback| feedback.id == feedback_id)
                    .ok_or_else(|| {
                        EngineError::InvalidInput(format!(
                            "feedback {feedback_id} does not belong to episode {}",
                            evidence.episode_id
                        ))
                    })?;
                self.trust
                    .verified_feedback(&feedback, selected.evaluation.tier)?
            } else {
                self.trust
                    .verified_engine_episode(&episode, selected.evaluation.tier)?
            };
            if matches!(
                selected.evaluation.tier,
                VerifiabilityTier::Hard | VerifiabilityTier::Consensus
            ) && trusted.is_none()
            {
                return Err(EngineError::InvalidInput(format!(
                    "evidence episode {} lacks an Engine trust receipt for its strong evaluation",
                    evidence.episode_id
                )));
            }
            if !selected.evaluation.success && trusted.is_some() {
                verified_episode_ids.insert(evidence.episode_id);
            }
            if let Some(receipt) = trusted {
                strongest_tier = stronger_tier(strongest_tier, receipt.tier);
                sources.insert(receipt.issuer);
            } else {
                strongest_tier = stronger_tier(strongest_tier, VerifiabilityTier::Deferred);
                sources.insert("untrusted:deferred".into());
            }
            episodes
                .entry(episode.id)
                .and_modify(|stored| {
                    if !stored.failed() && episode.failed() {
                        *stored = episode.clone();
                    }
                })
                .or_insert(episode);
        }
        let verified_episodes = u32::try_from(verified_episode_ids.len()).unwrap_or(u32::MAX);
        let distinct_sources = u32::try_from(sources.len()).unwrap_or(u32::MAX);
        let challenger_beats_incumbent = match &request.target {
            AdaptationTarget::ProcedureReplacement {
                incumbent_id,
                incumbent_version,
                challenger,
            } => self.challenger_beats_evidence(
                *incumbent_id,
                *incumbent_version,
                challenger,
                &episodes.into_values().collect::<Vec<_>>(),
            )?,
            _ => false,
        };
        Ok(AdaptationEvidenceGate {
            verified_episodes,
            distinct_sources,
            strongest_tier,
            challenger_beats_incumbent,
            corroborated: verified_episodes >= 2 && distinct_sources >= 2,
            offline: request.target.is_broad(),
        })
    }

    fn challenger_beats_evidence(
        &self,
        incumbent_id: ProcedureId,
        incumbent_version: u32,
        challenger: &Procedure,
        episodes: &[Episode],
    ) -> Result<bool, EngineError> {
        let Some(incumbent) = self
            .graph
            .get_procedure_version(incumbent_id, incumbent_version)?
        else {
            return Ok(false);
        };
        if !is_narrow_body_replacement(&incumbent, challenger)? || episodes.is_empty() {
            return Ok(false);
        }
        let mut replays = Vec::new();
        for episode in episodes {
            let Some(evaluation) = episode.evaluation.as_ref() else {
                return Ok(false);
            };
            if !matches!(
                evaluation.tier,
                VerifiabilityTier::Hard | VerifiabilityTier::Consensus
            ) {
                continue;
            }
            let Some(expected) = episode.observed_result.as_ref() else {
                return Ok(false);
            };
            let Some(trace_json) = episode.execution_trace.clone() else {
                return Ok(false);
            };
            let trace: ExecTrace = serde_json::from_value(trace_json)?;
            let Some(top) = trace.steps.last() else {
                return Ok(false);
            };
            if top.procedure_called != Some(incumbent_id)
                || top.procedure_version != Some(incumbent_version)
            {
                return Ok(false);
            }
            let args = match top.input.as_ref() {
                None => Vec::new(),
                Some(Value::List(values)) => values.clone(),
                Some(_) => return Ok(false),
            };
            let mut evaluator = Evaluator::new().with_budget(self.max_steps);
            let mut registered = HashSet::new();
            for step in &trace.steps {
                let (Some(id), Some(version)) = (step.procedure_called, step.procedure_version)
                else {
                    return Ok(false);
                };
                if registered.insert(id) {
                    if id == incumbent_id {
                        evaluator.register_procedure(challenger.clone());
                    } else {
                        let Some(exact) = self.graph.get_procedure_version(id, version)? else {
                            return Ok(false);
                        };
                        evaluator.register_procedure(exact);
                    }
                }
            }
            let candidate = evaluator.exec_procedure(&incumbent_id, args).ok();
            let challenger_correct = candidate
                .as_ref()
                .is_some_and(|result| &result.value == expected);
            replays.push(PromotionReplay {
                episode_id: episode.id,
                incumbent_correct: evaluation.success,
                challenger_correct,
                incumbent_trace_steps: u32::try_from(trace.len()).ok(),
                challenger_trace_steps: candidate
                    .as_ref()
                    .and_then(|result| u32::try_from(result.trace.len()).ok()),
                incumbent_candidates_explored: None,
                challenger_candidates_explored: None,
                transfer: false,
            });
        }
        Ok(PromotionGate::evaluate(replays).shadow_eligible())
    }

    fn recover_narrow_apply(
        &self,
        action: &AdaptationAction,
        error: ekg_adapt::AdaptError,
    ) -> Result<ApplyOutcome, EngineError> {
        let AdaptationAction::NarrowScope {
            procedure_id,
            expected_version,
            condition,
            learned_from,
        } = action
        else {
            return Err(error.into());
        };
        let Some(current) = self.graph.get_procedure(*procedure_id)? else {
            return Err(error.into());
        };
        if current.version == expected_version.saturating_add(1)
            && current.contract.requires.iter().any(|stored| {
                match (stable_json(stored), stable_json(condition)) {
                    (Ok(stored), Ok(expected)) => stored == expected,
                    _ => false,
                }
            })
            && current.contract.confidence.scope.iter().any(|scope| {
                scope.description == condition.description
                    && scope.learned_from == Some(*learned_from)
            })
        {
            return Ok(ApplyOutcome::ProcedureUpdated {
                procedure_id: *procedure_id,
                previous_version: *expected_version,
                current_version: current.version,
            });
        }
        Err(error.into())
    }

    fn apply_broad_action(
        &self,
        action: &AdaptationAction,
        updated_at: i64,
    ) -> Result<ApplyOutcome, EngineError> {
        match action {
            AdaptationAction::ReplaceProcedure {
                incumbent_id,
                incumbent_version,
                challenger,
            } => {
                let current = self.graph.get_procedure(*incumbent_id)?.ok_or_else(|| {
                    EngineError::InvalidInput(format!("procedure {incumbent_id} does not exist"))
                })?;
                let mut expected_replacement = (**challenger).clone();
                expected_replacement.updated_at = updated_at;
                if current.version == incumbent_version.saturating_add(1)
                    && stable_json(&current)? == stable_json(&expected_replacement)?
                {
                    return Ok(ApplyOutcome::ProcedureUpdated {
                        procedure_id: *incumbent_id,
                        previous_version: *incumbent_version,
                        current_version: current.version,
                    });
                }
                if current.version != *incumbent_version
                    || !is_narrow_body_replacement(&current, challenger)?
                {
                    return Err(EngineError::InvalidInput(
                        "procedure replacement no longer matches the pinned incumbent".into(),
                    ));
                }
                reject_protected_concept(&self.graph, current.concept)?;
                let replacement = expected_replacement;
                self.graph
                    .revise_procedure(&replacement, *incumbent_version)?;
                Ok(ApplyOutcome::ProcedureUpdated {
                    procedure_id: *incumbent_id,
                    previous_version: *incumbent_version,
                    current_version: replacement.version,
                })
            }
            AdaptationAction::ReviseConceptOffline {
                concept_id,
                expected_version,
                revised_description,
                ..
            } => {
                let mut concept = self.graph.get_concept(*concept_id)?.ok_or_else(|| {
                    EngineError::InvalidInput(format!("concept {concept_id} does not exist"))
                })?;
                let current_version = self.graph.current_concept_version(*concept_id)?;
                if current_version == expected_version.saturating_add(1)
                    && concept.description.as_deref() == Some(revised_description)
                {
                    return Ok(ApplyOutcome::ConceptUpdated {
                        concept_id: *concept_id,
                    });
                }
                if current_version != *expected_version {
                    return Err(EngineError::InvalidInput(
                        "concept revision no longer matches the pinned version".into(),
                    ));
                }
                if !matches!(
                    concept.mutability,
                    MutabilityClass::DefeasibleGeneral | MutabilityClass::Procedural
                ) {
                    return Err(EngineError::InvalidInput(format!(
                        "offline adaptation cannot revise {:?} concepts",
                        concept.mutability
                    )));
                }
                concept.description = Some(revised_description.clone());
                concept.updated_at = updated_at;
                self.graph.revise_concept(&concept, *expected_version)?;
                Ok(ApplyOutcome::ConceptUpdated {
                    concept_id: *concept_id,
                })
            }
            _ => Err(EngineError::InvalidInput(
                "offline adaptation plan does not contain a broad mutation".into(),
            )),
        }
    }

    /// Executes every trusted, version-pinned case relevant to a broad plan
    /// before touching the graph. The report is persisted even for a rejected
    /// change, which preserves the evidence trail and makes retries honest.
    fn broad_regression_suite(
        &self,
        plan: &AdaptationPlan,
        recorded_at: i64,
    ) -> Result<RegressionSuiteVerdict, EngineError> {
        if let Some(existing) = self.adaptations.regression_verdict(plan.id)? {
            return Ok(existing);
        }

        let mut verdict = match &plan.action {
            AdaptationAction::ReplaceProcedure {
                incumbent_id,
                incumbent_version,
                challenger,
            } => {
                self.run_candidate_regression_cases(*incumbent_id, *incumbent_version, challenger)?
            }
            AdaptationAction::ReviseConceptOffline { concept_id, .. } => {
                self.run_concept_regression_cases(*concept_id)?
            }
            _ => {
                return Err(EngineError::InvalidInput(
                    "only broad adaptations have a full regression suite".into(),
                ));
            }
        };
        verdict.finalize();
        self.adaptations
            .record_regression_verdict(plan.id, &verdict, recorded_at)
    }

    /// Replays all durable verified cases pinned to the replaced procedure
    /// version through the candidate body. Test data comes exclusively from
    /// the episode store; the caller cannot supply expected outcomes here.
    fn run_candidate_regression_cases(
        &self,
        procedure_id: ProcedureId,
        procedure_version: u32,
        candidate: &Procedure,
    ) -> Result<RegressionSuiteVerdict, EngineError> {
        let cases = self
            .episodes
            .list_verified_regression_cases(procedure_id, procedure_version)?;
        let mut verdict = RegressionSuiteVerdict::empty();
        for case in cases {
            let supplied = case
                .test_case
                .inputs
                .iter()
                .cloned()
                .collect::<BTreeMap<_, _>>();
            let args = match crate::engine::bind_inputs(candidate, &supplied, None) {
                Ok(args) => args,
                Err(error) => {
                    verdict.inapplicable = verdict.inapplicable.saturating_add(1);
                    verdict.cases.push(RegressionSuiteCaseResult {
                        episode_id: case.episode_id,
                        procedure_id: case.procedure_id,
                        procedure_version: case.procedure_version,
                        expected_output: case.test_case.expected_output,
                        actual_output: None,
                        status: RegressionSuiteCaseStatus::Inapplicable,
                        details: format!("candidate cannot accept durable test input: {error}"),
                    });
                    continue;
                }
            };
            let mut evaluator = self.current_evaluator()?;
            evaluator.register_procedure(candidate.clone());
            let attempt = evaluator.exec_procedure_captured(&candidate.id, args);
            match attempt.result {
                Ok(actual) if actual == case.test_case.expected_output => {
                    verdict.passed = verdict.passed.saturating_add(1);
                    verdict.cases.push(RegressionSuiteCaseResult {
                        episode_id: case.episode_id,
                        procedure_id: case.procedure_id,
                        procedure_version: case.procedure_version,
                        expected_output: case.test_case.expected_output,
                        actual_output: Some(actual),
                        status: RegressionSuiteCaseStatus::Passed,
                        details: "candidate matched the immutable verified result".into(),
                    });
                }
                Ok(actual) => {
                    verdict.failed = verdict.failed.saturating_add(1);
                    verdict.cases.push(RegressionSuiteCaseResult {
                        episode_id: case.episode_id,
                        procedure_id: case.procedure_id,
                        procedure_version: case.procedure_version,
                        expected_output: case.test_case.expected_output,
                        actual_output: Some(actual),
                        status: RegressionSuiteCaseStatus::Failed,
                        details: "candidate output differed from immutable verified result".into(),
                    });
                }
                Err(error) => {
                    verdict.failed = verdict.failed.saturating_add(1);
                    verdict.cases.push(RegressionSuiteCaseResult {
                        episode_id: case.episode_id,
                        procedure_id: case.procedure_id,
                        procedure_version: case.procedure_version,
                        expected_output: case.test_case.expected_output,
                        actual_output: None,
                        status: RegressionSuiteCaseStatus::Failed,
                        details: format!("candidate execution failed: {error}"),
                    });
                }
            }
        }
        Ok(verdict)
    }

    /// A concept revision has no executable candidate body. Its applicable
    /// suite is the set of current executable procedures that implement the
    /// concept, replayed at their pinned current versions. This still prevents
    /// a behavior-wide structural change from being authorized with no locally
    /// verified behavior at all.
    fn run_concept_regression_cases(
        &self,
        concept_id: ConceptId,
    ) -> Result<RegressionSuiteVerdict, EngineError> {
        let mut verdict = RegressionSuiteVerdict::empty();
        for procedure in self.graph.list_procedures()? {
            if procedure.concept != Some(concept_id)
                || !crate::engine::is_current_executable(procedure.lifecycle)
            {
                continue;
            }
            let cases = self
                .episodes
                .list_verified_regression_cases(procedure.id, procedure.version)?;
            let candidate_verdict =
                self.run_candidate_regression_cases(procedure.id, procedure.version, &procedure)?;
            // A procedure with no durable cases contributes no evidence; its
            // behavior is not silently treated as a pass.
            if cases.is_empty() {
                continue;
            }
            verdict.passed = verdict.passed.saturating_add(candidate_verdict.passed);
            verdict.failed = verdict.failed.saturating_add(candidate_verdict.failed);
            verdict.inapplicable = verdict
                .inapplicable
                .saturating_add(candidate_verdict.inapplicable);
            verdict.cases.extend(candidate_verdict.cases);
        }
        Ok(verdict)
    }
}

fn adaptation_request_digest(request: &ApplyAdaptationRequest) -> Result<String, EngineError> {
    let bytes = serde_json::to_vec(request)?;
    let mut digest = Sha256::new();
    digest.update(b"ekg:offline-adaptation-request:v1\0");
    digest.update(bytes);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn regression_suite_rejection(verdict: &RegressionSuiteVerdict) -> EngineError {
    EngineError::InvalidInput(format!(
        "broad adaptation regression suite rejected mutation: {} applicable cases (minimum {}), {} passed, {} failed, {} inapplicable",
        verdict.applicable,
        verdict.required_minimum,
        verdict.passed,
        verdict.failed,
        verdict.inapplicable,
    ))
}

fn mutation_scope(action: &AdaptationAction) -> MutationScope {
    match action {
        AdaptationAction::NarrowScope { .. } => MutationScope::OnlineNarrow,
        AdaptationAction::ReplaceProcedure { .. }
        | AdaptationAction::ReviseConceptOffline { .. } => MutationScope::OfflineBroad,
        AdaptationAction::RecordOnly { .. }
        | AdaptationAction::FixAssumption { .. }
        | AdaptationAction::ScheduleTest { .. } => MutationScope::NoGraphChange,
    }
}

fn changed_knowledge(action: &AdaptationAction) -> Option<KnowledgeRef> {
    match action {
        AdaptationAction::NarrowScope { procedure_id, .. }
        | AdaptationAction::ReplaceProcedure {
            incumbent_id: procedure_id,
            ..
        } => Some(KnowledgeRef::Procedure(*procedure_id)),
        AdaptationAction::ReviseConceptOffline { concept_id, .. } => {
            Some(KnowledgeRef::Concept(*concept_id))
        }
        _ => None,
    }
}

fn stronger_tier(
    current: Option<VerifiabilityTier>,
    candidate: VerifiabilityTier,
) -> Option<VerifiabilityTier> {
    let rank = |tier| match tier {
        VerifiabilityTier::Hard => 3,
        VerifiabilityTier::Consensus => 2,
        VerifiabilityTier::Deferred => 1,
    };
    Some(match current {
        Some(current) if rank(current) >= rank(candidate) => current,
        _ => candidate,
    })
}

fn reject_protected_concept(
    graph: &ekg_graph::KnowledgeStore,
    concept_id: Option<ConceptId>,
) -> Result<(), EngineError> {
    if let Some(concept_id) = concept_id {
        let concept = graph
            .get_concept(concept_id)?
            .ok_or_else(|| EngineError::InvalidInput(format!("concept {concept_id} not found")))?;
        if matches!(
            concept.mutability,
            MutabilityClass::Definitional
                | MutabilityClass::Normative
                | MutabilityClass::CoreMachinery
                | MutabilityClass::Particular
        ) {
            return Err(EngineError::InvalidInput(format!(
                "adaptation cannot revise {:?} knowledge",
                concept.mutability
            )));
        }
    }
    Ok(())
}

fn stable_json(value: &impl Serialize) -> Result<String, EngineError> {
    Ok(serde_json::to_string(value)?)
}

fn is_narrow_body_replacement(
    incumbent: &Procedure,
    challenger: &Procedure,
) -> Result<bool, EngineError> {
    Ok(challenger.id == incumbent.id
        && challenger.version == incumbent.version.saturating_add(1)
        && challenger.created_at == incumbent.created_at
        && challenger.body != incumbent.body
        && crate::engine::is_current_executable(challenger.lifecycle)
        && challenger.name == incumbent.name
        && stable_json(&challenger.params)? == stable_json(&incumbent.params)?
        && stable_json(&challenger.contract)? == stable_json(&incumbent.contract)?
        && stable_json(&challenger.test_cases)? == stable_json(&incumbent.test_cases)?
        && challenger.concept == incumbent.concept
        && challenger.lifecycle == incumbent.lifecycle)
}
