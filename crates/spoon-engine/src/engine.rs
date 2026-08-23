use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spoon_capability::{
    CapabilityBundle, CapabilityInvocation, CapabilityInvocationAdapter, CapabilityProcedure,
    CapabilityStore, ImportedCapability, LocalValidation, Permission, PermissionPolicy,
    PrimitivePolicy,
};
use spoon_core::{
    Concept, ConceptId, ContractCheckResult, Episode, EpisodeCost, EpisodeId, EscalationRung,
    Evaluation, Expr, Lifecycle, ObservedFact, Procedure, ProcedureId, ReasoningTrace,
    Relationship, RelationshipId, Session, SessionVisibility, SpoonError, TestCase, TraceStep,
    TraceStepStatus, Value, VerifiabilityTier,
};
use spoon_episode::{
    EpisodeFeedback, EpisodeStore, TeacherInteractionMetrics, VerifiedRegressionCase,
};
use spoon_exec::{ConditionCheckStatus, Evaluator, ExecStepStatus, ExecTrace};
use spoon_graph::{ActivationSpreadQuery, ActivationSpreadResult, GraphError, KnowledgeStore};
use spoon_intuition::{
    EpistemicChallengeKind, IntuitionMetrics, IntuitionStore, RankingExample, RecallCandidate,
    RecallDocument, RecallKind, SupervisionTask,
};
use thiserror::Error;

use crate::evaluate_deterministic;

/// A self-supervision replay is deliberately much smaller than the normal
/// execution allowance. It verifies one known trace; it is not a back door to
/// an open-ended autonomous run.
const MAX_GROUNDED_SUPERVISION_REPLAY_STEPS: u32 = 256;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error(transparent)]
    Core(#[from] SpoonError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Credit(#[from] spoon_credit::CreditError),
    #[error(transparent)]
    Adapt(#[from] spoon_adapt::AdaptError),
    #[error(transparent)]
    Intuition(#[from] spoon_intuition::IntuitionError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    InvalidInput(String),
    #[error("episode {0} has no replayable execution trace")]
    MissingTrace(EpisodeId),
    #[error("trace does not identify a top-level procedure")]
    MissingTopLevelProcedure,
    #[error("execution failed in episode {episode_id}: {source}")]
    ExecutionFailed {
        episode_id: EpisodeId,
        #[source]
        source: SpoonError,
    },
    /// Capability invocations are persisted even when authorization, schema,
    /// policy, or an injected adapter rejects them. The episode id is the
    /// immutable evidence handle for the failed attempt.
    #[error("capability invocation failed in episode {episode_id}: {reason}")]
    CapabilityInvocationFailed {
        episode_id: EpisodeId,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutcome {
    pub value: Value,
    pub trace: ExecTrace,
    pub episode: Episode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayOutcome {
    pub value: Value,
    pub trace: ExecTrace,
    pub source_episode: EpisodeId,
}

/// The immediate result of one locally authorized capability call.
///
/// `invocation.output` is intentionally available to the immediate caller,
/// but the durable `episode` stores only its digest and structural summary.
/// This keeps credentials or sensitive transport payloads out of Spoon's
/// long-lived cognitive record while retaining auditable provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityExecutionOutcome {
    pub invocation: CapabilityInvocation,
    pub episode: Episode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub episode_count: u64,
    /// Number of strong, successful answers retained as local regression baselines.
    /// This is a baseline-coverage signal, not proof that every baseline was
    /// re-run successfully.
    pub verified_answer_count: u64,
    pub rung_distribution: Vec<(String, u32)>,
    pub intuition: IntuitionMetrics,
    /// Bounded durable evidence for Phase 6 exit criteria. Counts describe
    /// only persisted observations, not blanket claims of transfer, weaning,
    /// or regression freedom.
    pub phase6: Phase6EvidenceMetrics,
    /// Immutable benchmark/probe evidence for the full Section 38 scorecard.
    /// Empty/insufficient slots are explicit rather than scored optimistically.
    pub section38: crate::Section38TelemetrySnapshot,
}

/// Durable, conservative evidence for the Phase 6 exit criteria.
///
/// Managed-skill evidence is intentionally bounded to the 512 records exposed
/// by the existing read-only skill view. The examined-record count makes that
/// boundary visible to snapshot consumers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Phase6EvidenceMetrics {
    pub teacher_interaction_episodes: u64,
    pub teacher_assisted_successes: u64,
    pub teacher_free_successes: u64,
    pub managed_skill_records_examined: u64,
    pub replay_preserved_skill_verdicts: u64,
    pub replay_regressions: u64,
    pub transfer_eligible_skill_verdicts: u64,
    pub currently_promoted_skills: u64,
    /// Only promoted skills can produce these counters. A zero means no
    /// post-promotion use was recorded; it does not establish non-survival.
    pub post_promotion_skill_uses: u64,
    pub post_promotion_skill_successes: u64,
}

/// Phase 0 orchestration boundary. It owns the graph and episode stores and
/// creates a fresh bounded evaluator for each run so execution state cannot
/// leak across episodes.
pub struct Engine {
    pub(crate) graph: KnowledgeStore,
    pub(crate) episodes: EpisodeStore,
    pub(crate) max_steps: u32,
    pub(crate) pending_cycles: HashMap<crate::CycleId, crate::cycle::PendingCycle>,
    pub(crate) adaptations: crate::adaptation::AdaptationStore,
    pub(crate) credit_analyses: crate::credit::CreditAnalysisStore,
    pub(crate) contradictions: spoon_adapt::ContradictionStore,
    pub(crate) lesson_stages: crate::lesson::LessonStageStore,
    pub(crate) runtime: crate::runtime::RuntimeStore,
    pub(crate) compression: crate::compression::CompressionStore,
    pub(crate) regression: crate::regression::RegressionStore,
    pub(crate) trust: crate::trust::TrustLedger,
    pub(crate) intuition: IntuitionStore,
    pub(crate) capabilities: CapabilityStore,
    pub(crate) goals: crate::goals::GoalStore,
    pub(crate) skills: crate::skills::SkillStore,
    pub(crate) telemetry: crate::telemetry::FalsificationTelemetryStore,
    pub(crate) instance_id: uuid::Uuid,
    pub(crate) admin_enabled: bool,
}

impl Engine {
    pub fn open(path: &str) -> Result<Self, EngineError> {
        let mut engine = Self {
            graph: KnowledgeStore::new(path)?,
            episodes: EpisodeStore::new(path)?,
            max_steps: 1_000_000,
            pending_cycles: HashMap::new(),
            adaptations: crate::adaptation::AdaptationStore::open(path)?,
            credit_analyses: crate::credit::CreditAnalysisStore::open(path)?,
            contradictions: spoon_adapt::ContradictionStore::open(path)?,
            lesson_stages: crate::lesson::LessonStageStore::open(path)?,
            runtime: crate::runtime::RuntimeStore::open(path)?,
            compression: crate::compression::CompressionStore::open(path)?,
            regression: crate::regression::RegressionStore::open(path)?,
            trust: crate::trust::TrustLedger::open(path)?,
            intuition: IntuitionStore::open(path)?,
            capabilities: CapabilityStore::open(path)
                .map_err(|error| EngineError::InvalidInput(format!("capability store: {error}")))?,
            goals: crate::goals::GoalStore::open(path)?,
            skills: crate::skills::SkillStore::open(path)?,
            telemetry: crate::telemetry::FalsificationTelemetryStore::open(path)?,
            instance_id: uuid::Uuid::new_v4(),
            admin_enabled: false,
        };
        // An adaptation stage is durable proof that authorization (including
        // any one-shot offline capability) was consumed before mutation began.
        // Finish those exact staged requests before accepting new work.
        engine.recover_pending_episode_sagas()?;
        engine.recover_pending_feedback_sagas()?;
        engine.recover_pending_cycles()?;
        engine.recover_pending_lessons()?;
        engine.recover_pending_adaptations()?;
        engine.reconcile_observed_fact_contradictions()?;
        engine.rebuild_intuition_index()?;
        Ok(engine)
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        Ok(Self {
            graph: KnowledgeStore::in_memory()?,
            episodes: EpisodeStore::in_memory()?,
            max_steps: 1_000_000,
            pending_cycles: HashMap::new(),
            adaptations: crate::adaptation::AdaptationStore::in_memory()?,
            credit_analyses: crate::credit::CreditAnalysisStore::in_memory()?,
            contradictions: spoon_adapt::ContradictionStore::in_memory()?,
            lesson_stages: crate::lesson::LessonStageStore::in_memory()?,
            runtime: crate::runtime::RuntimeStore::in_memory()?,
            compression: crate::compression::CompressionStore::in_memory()?,
            regression: crate::regression::RegressionStore::in_memory()?,
            trust: crate::trust::TrustLedger::in_memory()?,
            intuition: IntuitionStore::in_memory()?,
            capabilities: CapabilityStore::in_memory()
                .map_err(|error| EngineError::InvalidInput(format!("capability store: {error}")))?,
            goals: crate::goals::GoalStore::in_memory()?,
            skills: crate::skills::SkillStore::in_memory()?,
            telemetry: crate::telemetry::FalsificationTelemetryStore::in_memory()?,
            instance_id: uuid::Uuid::new_v4(),
            admin_enabled: false,
        })
    }

    pub fn in_memory_with_admin(secret: &str) -> Result<Self, EngineError> {
        let mut engine = Self::in_memory()?;
        engine.enable_admin(secret)?;
        Ok(engine)
    }

    pub fn open_with_admin(path: &str, secret: &str) -> Result<Self, EngineError> {
        let mut engine = Self::open(path)?;
        engine.enable_admin(secret)?;
        Ok(engine)
    }

    pub fn with_max_steps(mut self, max_steps: u32) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// Retrieves a bounded, inverted-index candidate set. This is a
    /// representation/search operation only; candidates remain untrusted
    /// until ordinary Engine reasoning and evidence gates use them.
    pub fn recall_candidates(
        &self,
        query: &str,
        candidate_limit: usize,
    ) -> Result<Vec<RecallCandidate>, EngineError> {
        Ok(self.intuition.retrieve(query, candidate_limit)?)
    }

    pub fn rank_recall_candidates(
        &self,
        query: &str,
        candidate_limit: usize,
    ) -> Result<Vec<RecallCandidate>, EngineError> {
        Ok(self.intuition.rank(query, candidate_limit)?)
    }

    /// Runs a bounded time-split ranking evaluation. The held-out outcomes are
    /// never used to score the learned ordering, so the result is evidence
    /// about search policy rather than a new trust assertion.
    pub fn evaluate_recall_ranking(
        &self,
        query: &str,
        candidate_limit: usize,
        holdout_examples: usize,
    ) -> Result<spoon_intuition::RankingEvaluation, EngineError> {
        Ok(self
            .intuition
            .evaluate_ranking(query, candidate_limit, holdout_examples)?)
    }

    /// Compares lexical and local-semantic candidate coverage on complete,
    /// held-out query groups. It is retrieval evidence only and cannot alter
    /// belief, trust, or graph lifecycle state.
    pub fn evaluate_semantic_recall(
        &self,
        candidate_limit: usize,
        holdout_queries: usize,
    ) -> Result<spoon_intuition::SemanticRecallEvaluation, EngineError> {
        Ok(self
            .intuition
            .evaluate_semantic_recall(candidate_limit, holdout_queries)?)
    }

    /// Retrieves graph-neighborhood candidates through a hard-bounded typed
    /// activation spread. The returned activation is relevance, not trust.
    pub fn activation_candidates(
        &self,
        query: &ActivationSpreadQuery,
    ) -> Result<ActivationSpreadResult, EngineError> {
        Ok(self.graph.activation_spread(query)?)
    }

    pub fn discover_capability(
        &self,
        description: &spoon_capability::InterfaceDescription,
    ) -> Result<CapabilityBundle, EngineError> {
        spoon_capability::discover_interface(description)
            .map_err(|error| EngineError::InvalidInput(format!("capability discovery: {error}")))
    }

    pub fn import_capability_bundle(
        &self,
        bytes: &[u8],
    ) -> Result<ImportedCapability, EngineError> {
        self.capabilities
            .import(bytes)
            .map_err(|error| EngineError::InvalidInput(format!("capability import: {error}")))
    }

    pub fn import_and_revalidate_capability_bundle(
        &self,
        bytes: &[u8],
        validation: &LocalValidation,
    ) -> Result<ImportedCapability, EngineError> {
        if validation.passed {
            self.validate_local_capability_episodes(validation)?;
        }
        self.capabilities
            .import_and_revalidate(bytes, validation)
            .map_err(|error| {
                EngineError::InvalidInput(format!("capability import and revalidation: {error}"))
            })
    }

    pub fn reconstruct_capability(
        &self,
        content_id: &str,
    ) -> Result<spoon_capability::ReconstructedCapability, EngineError> {
        self.capabilities.reconstruct(content_id).map_err(|error| {
            EngineError::InvalidInput(format!("capability reconstruction: {error}"))
        })
    }

    pub fn export_capability_bundle(&self, content_id: &str) -> Result<Vec<u8>, EngineError> {
        self.capabilities
            .export(content_id)
            .map_err(|error| EngineError::InvalidInput(format!("capability export: {error}")))
    }

    pub fn revalidate_capability(
        &self,
        content_id: &str,
        validation: &LocalValidation,
    ) -> Result<ImportedCapability, EngineError> {
        if validation.passed {
            self.validate_local_capability_episodes(validation)?;
        }
        self.capabilities
            .revalidate(content_id, validation)
            .map_err(|error| EngineError::InvalidInput(format!("capability validation: {error}")))
    }

    pub fn grant_capability_permission(
        &self,
        content_id: &str,
        permission: &Permission,
    ) -> Result<(), EngineError> {
        self.require_admin()?;
        self.capabilities
            .grant(content_id, permission)
            .map_err(|error| EngineError::InvalidInput(format!("capability grant: {error}")))
    }

    pub fn revoke_capability_permission(
        &self,
        content_id: &str,
        permission: &Permission,
    ) -> Result<(), EngineError> {
        self.require_admin()?;
        self.capabilities
            .revoke(content_id, permission)
            .map_err(|error| EngineError::InvalidInput(format!("capability revoke: {error}")))
    }

    pub fn require_capability_permissions(
        &self,
        content_id: &str,
        permissions: &[Permission],
    ) -> Result<(), EngineError> {
        self.capabilities
            .require_permissions(content_id, permissions)
            .map_err(|error| {
                EngineError::InvalidInput(format!("capability authorization: {error}"))
            })
    }

    fn validate_local_capability_episodes(
        &self,
        validation: &LocalValidation,
    ) -> Result<(), EngineError> {
        if validation.validation_episodes.is_empty() {
            return Err(EngineError::InvalidInput(
                "capability validation requires locally trusted episode evidence".into(),
            ));
        }
        for episode_id in &validation.validation_episodes {
            let uuid = uuid::Uuid::parse_str(episode_id).map_err(|_| {
                EngineError::InvalidInput(
                    "capability validation episode ids must be local UUIDs".into(),
                )
            })?;
            let episode = self.episodes.get(EpisodeId(uuid))?;
            if self.trust.receipt_for_episode(&episode)?.is_none() {
                return Err(EngineError::InvalidInput(format!(
                    "capability validation episode {episode_id} has no exact Engine trust receipt"
                )));
            }
            if !episode.evaluation.as_ref().is_some_and(|evaluation| {
                evaluation.success
                    && matches!(
                        evaluation.tier,
                        VerifiabilityTier::Hard | VerifiabilityTier::Consensus
                    )
            }) {
                return Err(EngineError::InvalidInput(format!(
                    "capability validation episode {episode_id} is not a successful strong evaluation"
                )));
            }
        }
        Ok(())
    }

    pub fn require_capability_procedure(
        &self,
        content_id: &str,
        procedure_id: &str,
    ) -> Result<spoon_capability::CapabilityProcedure, EngineError> {
        self.capabilities
            .require_procedure_permissions(content_id, procedure_id)
            .map_err(|error| {
                EngineError::InvalidInput(format!("capability authorization: {error}"))
            })
    }

    /// Invoke one exact, locally revalidated capability procedure through an
    /// injected host adapter.
    ///
    /// The Engine never chooses an ambient network, filesystem, or process
    /// implementation here: the caller supplies both the narrow primitive
    /// policy and the adapter that owns the effect boundary. `CapabilityStore`
    /// resolves the stored procedure and re-checks its status and every grant
    /// on every invocation, including immediately after a revocation.
    ///
    /// The returned invocation contains the immediate typed output. The
    /// durable episode deliberately contains only output/input digests and a
    /// structural summary, plus the capability receipt, declared effects,
    /// permissions, and resource usage. If authorization or execution fails,
    /// an immutable failure episode is persisted before this method returns
    /// [`EngineError::CapabilityInvocationFailed`].
    pub fn invoke_capability<A: CapabilityInvocationAdapter>(
        &self,
        content_id: &str,
        procedure_id: &str,
        input: &serde_json::Value,
        expected_output: Option<&serde_json::Value>,
        policy: &PrimitivePolicy,
        adapter: &mut A,
    ) -> Result<CapabilityExecutionOutcome, EngineError> {
        self.invoke_capability_with_permission_policy(
            content_id,
            procedure_id,
            input,
            expected_output,
            policy,
            None,
            adapter,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn invoke_capability_with_permission_policy<A: CapabilityInvocationAdapter>(
        &self,
        content_id: &str,
        procedure_id: &str,
        input: &serde_json::Value,
        expected_output: Option<&serde_json::Value>,
        policy: &PrimitivePolicy,
        permission_policy: Option<&PermissionPolicy>,
        adapter: &mut A,
    ) -> Result<CapabilityExecutionOutcome, EngineError> {
        // Reconstruction is authority-free. It lets a rejected invocation
        // retain the exact declared procedure shape when the durable store can
        // still read it, without ever treating that shape as executable.
        let procedure = self
            .reconstruct_capability(content_id)
            .ok()
            .and_then(|capability| {
                capability
                    .procedures
                    .into_iter()
                    .find(|procedure| procedure.id == procedure_id)
            });
        let input_digest = json_digest(input);
        let policy_digest = json_digest(policy);

        let invocation = match permission_policy {
            Some(permission_policy) => self.capabilities.invoke_with_permission_policy(
                content_id,
                procedure_id,
                input,
                policy,
                permission_policy,
                adapter,
            ),
            None => self
                .capabilities
                .invoke(content_id, procedure_id, input, policy, adapter),
        };
        match invocation {
            Ok(invocation) => {
                let output_digest = invocation.output_digest.clone();
                let mut episode = self.capability_invocation_episode(
                    content_id,
                    procedure_id,
                    procedure.as_ref(),
                    &input_digest,
                    &policy_digest,
                );
                let expected_digest = expected_output.map(json_digest);
                episode.prediction = expected_digest.clone().map(Value::Text);
                episode.action = Some(format!(
                    "capability:{}:{}:invoked",
                    content_id, procedure_id
                ));
                episode.observed_result = Some(redacted_capability_output(
                    &output_digest,
                    &invocation.output,
                ));
                episode.evaluation = Some(match expected_output {
                    Some(expected) => Evaluation {
                        tier: VerifiabilityTier::Hard,
                        success: expected == &invocation.output,
                        details: format!(
                            "deterministic capability output comparison (expected {}, observed {})",
                            expected_digest.unwrap_or_else(|| "sha256:unavailable".into()),
                            output_digest
                        ),
                        surprise: Some(if expected == &invocation.output {
                            0.0
                        } else {
                            1.0
                        }),
                    },
                    None => Evaluation {
                        tier: VerifiabilityTier::Deferred,
                        success: true,
                        details: format!(
                            "capability adapter completed; output retained only as {} and needs independent verification",
                            output_digest
                        ),
                        surprise: None,
                    },
                });
                episode.reasoning_trace.steps.push(TraceStep {
                    description: "invoke locally authorized capability procedure".into(),
                    procedure_used: None,
                    contract_check: Some(ContractCheckResult {
                        all_requires_met: true,
                        all_promises_met: episode.succeeded(),
                        no_failure_conditions_met: true,
                        violations: Vec::new(),
                    }),
                    input: Some(Value::Text(input_digest.clone())),
                    output: Some(Value::Text(output_digest.clone())),
                    rung: EscalationRung::Run,
                    status: TraceStepStatus::Succeeded,
                });
                episode.execution_trace = Some(serde_json::json!({
                    "kind": "capability_invocation_v1",
                    "contentId": content_id,
                    "procedureId": procedure_id,
                    "inputDigest": input_digest,
                    "outputDigest": output_digest,
                    "redacted": true,
                    "receipt": &invocation.receipt,
                    "usage": invocation.usage,
                }));
                episode.teacher_interaction = Some(serde_json::json!({
                    "kind": "capability_invocation",
                    "contentId": content_id,
                    "procedure": capability_procedure_summary(procedure.as_ref(), procedure_id),
                    "declaredEffects": procedure.as_ref().map(|item| &item.effects),
                    "declaredPermissions": procedure.as_ref().map(|item| &item.permissions),
                    "policyDigest": policy_digest,
                    "inputDigest": input_digest,
                    "output": redacted_json_output(&output_digest, &invocation.output),
                    "receipt": &invocation.receipt,
                    "usage": invocation.usage,
                }));
                episode.cost = EpisodeCost {
                    rung_reached: EscalationRung::Run,
                    steps_taken: 1,
                    budget_spent: invocation.usage.steps as f64,
                };
                self.persist_engine_episode(&episode)?;
                Ok(CapabilityExecutionOutcome {
                    invocation,
                    episode,
                })
            }
            Err(error) => {
                let mut episode = self.capability_invocation_episode(
                    content_id,
                    procedure_id,
                    procedure.as_ref(),
                    &input_digest,
                    &policy_digest,
                );
                episode.action = Some(format!("capability:{}:{}:failed", content_id, procedure_id));
                let failure_digest = text_digest(&error.to_string());
                episode.evaluation = Some(Evaluation {
                    tier: VerifiabilityTier::Hard,
                    success: false,
                    details: format!("capability invocation rejected ({failure_digest})"),
                    surprise: None,
                });
                episode.reasoning_trace.steps.push(TraceStep {
                    description: "capability authorization or adapter invocation rejected".into(),
                    procedure_used: None,
                    contract_check: Some(ContractCheckResult {
                        all_requires_met: false,
                        all_promises_met: false,
                        no_failure_conditions_met: false,
                        violations: vec![failure_digest.clone()],
                    }),
                    input: Some(Value::Text(input_digest.clone())),
                    output: None,
                    rung: EscalationRung::Run,
                    status: TraceStepStatus::Failed {
                        error: failure_digest.clone(),
                    },
                });
                episode.execution_trace = Some(serde_json::json!({
                    "kind": "capability_invocation_v1",
                    "contentId": content_id,
                    "procedureId": procedure_id,
                    "inputDigest": input_digest,
                    "policyDigest": policy_digest,
                    "failureDigest": failure_digest,
                    "redacted": true,
                }));
                episode.teacher_interaction = Some(serde_json::json!({
                    "kind": "capability_invocation",
                    "contentId": content_id,
                    "procedure": capability_procedure_summary(procedure.as_ref(), procedure_id),
                    "declaredEffects": procedure.as_ref().map(|item| &item.effects),
                    "declaredPermissions": procedure.as_ref().map(|item| &item.permissions),
                    "policyDigest": policy_digest,
                    "inputDigest": input_digest,
                    "failure": { "redacted": true, "digest": failure_digest },
                }));
                episode.cost = EpisodeCost {
                    rung_reached: EscalationRung::Run,
                    steps_taken: 1,
                    budget_spent: 0.0,
                };
                self.persist_engine_episode(&episode)?;
                Err(EngineError::CapabilityInvocationFailed {
                    episode_id: episode.id,
                    reason: error.to_string(),
                })
            }
        }
    }

    fn capability_invocation_episode(
        &self,
        content_id: &str,
        procedure_id: &str,
        procedure: Option<&CapabilityProcedure>,
        input_digest: &str,
        policy_digest: &str,
    ) -> Episode {
        let mut episode =
            Episode::new(format!("capability invocation {content_id}/{procedure_id}"));
        episode.context.environment = BTreeMap::from([
            ("capabilityContentId".into(), Value::Text(content_id.into())),
            (
                "capabilityProcedureId".into(),
                Value::Text(procedure_id.into()),
            ),
            ("inputDigest".into(), Value::Text(input_digest.into())),
            ("policyDigest".into(), Value::Text(policy_digest.into())),
        ]);
        if let Some(procedure) = procedure {
            episode.context.environment.insert(
                "capabilityPrimitive".into(),
                Value::Text(format!("{:?}", procedure.primitive)),
            );
        }
        episode
    }

    pub fn create_goal(
        &self,
        kind: crate::goals::GoalKind,
        statement: &str,
        parent_id: Option<&str>,
    ) -> Result<crate::goals::Goal, EngineError> {
        self.goals.create_goal(kind, statement, parent_id)
    }

    pub fn list_goals(&self) -> Result<Vec<crate::goals::Goal>, EngineError> {
        self.goals.list_goals()
    }

    pub fn create_learning_goal(
        &self,
        statement: &str,
        standing_goal_id: &str,
        source_gap_id: &str,
        derivation_reason: &str,
    ) -> Result<crate::goals::Goal, EngineError> {
        self.goals.create_learning_goal(
            statement,
            standing_goal_id,
            source_gap_id,
            derivation_reason,
        )
    }

    pub fn create_instrumental_goal(
        &self,
        statement: &str,
        parent_goal_id: &str,
        derivation_reason: &str,
    ) -> Result<crate::goals::Goal, EngineError> {
        self.goals
            .create_instrumental_goal(statement, parent_goal_id, derivation_reason)
    }

    pub fn list_goal_derivation_records(
        &self,
    ) -> Result<Vec<crate::goals::GoalDerivationRecord>, EngineError> {
        self.goals.list_goal_derivation_records()
    }

    pub fn list_learning_goal_records(
        &self,
    ) -> Result<Vec<crate::goals::GoalLearningRecord>, EngineError> {
        self.goals.list_learning_goal_records()
    }

    pub fn record_curiosity_gap(
        &self,
        gap: &crate::goals::CuriosityGap,
    ) -> Result<(), EngineError> {
        self.goals.record_gap(gap)
    }

    pub fn rank_curiosity_gaps(
        &self,
        limit: u32,
    ) -> Result<Vec<crate::goals::CuriosityGap>, EngineError> {
        self.goals.rank_gaps(limit)
    }

    /// Produces at most one bounded, read-only learning proposal for the
    /// highest-valued unresolved gap.  Scheduling does not execute the
    /// proposal or grant mutation authority.
    pub fn schedule_next_learning_action(
        &self,
    ) -> Result<Option<crate::goals::ScheduledLearningAction>, EngineError> {
        self.goals.schedule_next_learning_action()
    }

    pub fn list_scheduled_learning_actions(
        &self,
        limit: u32,
    ) -> Result<Vec<crate::goals::ScheduledLearningAction>, EngineError> {
        self.goals.list_scheduled_learning_actions(limit)
    }

    pub fn discover_skill_candidates(
        &self,
        limit: u32,
    ) -> Result<Vec<spoon_adapt::SkillCandidate>, EngineError> {
        let episodes = self.episodes.list_recent(limit.clamp(1, 512))?;
        let mut candidates = spoon_adapt::discover_skills(&episodes);
        for episode in &episodes {
            if let Some(candidate) = spoon_adapt::discover_single_success(episode) {
                candidates.push(candidate);
            }
            if let Some(candidate) = spoon_adapt::discover_failure_critic(episode) {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    pub fn plan_episode_compression(
        &self,
        limit: u32,
    ) -> Result<spoon_adapt::EpisodeCompressionPlan, EngineError> {
        let episodes = self.episodes.list_recent(limit.clamp(1, 512))?;
        Ok(spoon_adapt::plan_episode_compression(&episodes))
    }

    /// Materializes a bounded compression plan without mutating or deleting
    /// the source episodes. Failed episodes are rejected by the store.
    pub fn compress_episode_history(
        &self,
        limit: u32,
    ) -> Result<crate::EpisodeCompressionResult, EngineError> {
        let episodes = self.episodes.list_recent(limit.clamp(1, 512))?;
        let plan = spoon_adapt::plan_episode_compression(&episodes);
        self.compression.apply(&episodes, plan)
    }

    pub fn list_episode_compression_records(
        &self,
        limit: u32,
    ) -> Result<Vec<crate::EpisodeCompressionRecord>, EngineError> {
        self.compression.list(limit)
    }

    pub fn list_verified_answers(
        &self,
        limit: u32,
    ) -> Result<Vec<crate::VerifiedAnswerRecord>, EngineError> {
        self.regression.list(limit)
    }

    /// Persists a discovered skill only when each cited episode is exact,
    /// strong Engine evidence. This prevents a caller-created report from
    /// becoming a future optimization target without provenance.
    pub fn register_skill_candidate(
        &self,
        candidate: &spoon_adapt::SkillCandidate,
    ) -> Result<crate::ManagedSkill, EngineError> {
        for episode_id in &candidate.source_episode_ids {
            let episode = self.episodes.get(*episode_id)?;
            if self.trust.receipt_for_episode(&episode)?.is_none() {
                return Err(EngineError::InvalidInput(format!(
                    "skill candidate source episode {episode_id} lacks an exact Engine trust receipt"
                )));
            }
        }
        self.skills.register(candidate)
    }

    pub fn list_managed_skills(&self, limit: u32) -> Result<Vec<crate::ManagedSkill>, EngineError> {
        self.skills.list(limit)
    }

    pub fn list_active_managed_skills(
        &self,
        limit: u32,
    ) -> Result<Vec<crate::ManagedSkill>, EngineError> {
        self.skills.list_active(limit)
    }

    /// Ranks non-retired skills using query fit and durable post-activation
    /// outcomes. Ranking is advisory and cannot promote or execute a skill.
    pub fn rank_active_managed_skills(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<crate::ManagedSkill>, EngineError> {
        self.skills.rank_active(query, limit)
    }

    pub fn select_executable_managed_skill(
        &self,
        query: &str,
    ) -> Result<Option<crate::ManagedSkill>, EngineError> {
        Ok(self
            .skills
            .rank_active(query, 512)?
            .into_iter()
            .find(|skill| {
                skill.lifecycle == crate::SkillLifecycle::Promoted
                    && !skill.candidate.failure_critic
            }))
    }

    pub fn register_single_success_skill(
        &self,
        episode_id: EpisodeId,
    ) -> Result<crate::ManagedSkill, EngineError> {
        let episode = self.episodes.get(episode_id)?;
        let candidate = spoon_adapt::discover_single_success(&episode).ok_or_else(|| {
            EngineError::InvalidInput(
                "episode is not a successful Hard or Consensus candidate".into(),
            )
        })?;
        self.register_skill_candidate(&candidate)
    }

    pub fn register_failure_critic_skill(
        &self,
        episode_id: EpisodeId,
    ) -> Result<crate::ManagedSkill, EngineError> {
        let episode = self.episodes.get(episode_id)?;
        let candidate = spoon_adapt::discover_failure_critic(&episode).ok_or_else(|| {
            EngineError::InvalidInput("episode is not an eligible failure critic".into())
        })?;
        self.register_skill_candidate(&candidate)
    }

    /// Records a caller-supplied replay report and enters shadow only after the
    /// conservative gate has preserved every replayed verified result and
    /// measured a win. This compatibility API does not execute the challenger;
    /// callers that need an engine-derived verdict should use
    /// [`Self::evaluate_skill_for_shadow_with_challenger`].
    pub fn evaluate_skill_for_shadow(
        &self,
        skill_id: &str,
        replays: impl IntoIterator<Item = spoon_adapt::PromotionReplay>,
    ) -> Result<crate::ManagedSkill, EngineError> {
        let skill = self.skills.get(skill_id)?.ok_or_else(|| {
            EngineError::InvalidInput(format!("unknown managed skill {skill_id}"))
        })?;
        if skill.lifecycle != crate::SkillLifecycle::Candidate {
            return Err(EngineError::InvalidInput(
                "only a candidate skill may enter shadow evaluation".into(),
            ));
        }
        let replays: Vec<_> = replays.into_iter().collect();
        let source_ids = skill
            .candidate
            .source_episode_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        for replay in &replays {
            if !source_ids.contains(&replay.episode_id) {
                return Err(EngineError::InvalidInput(format!(
                    "promotion replay episode {} is outside the candidate's derivation evidence",
                    replay.episode_id
                )));
            }
            if !seen.insert(replay.episode_id) {
                return Err(EngineError::InvalidInput(format!(
                    "promotion replay episode {} is duplicated",
                    replay.episode_id
                )));
            }
            let episode = self.episodes.get(replay.episode_id)?;
            if !episode.succeeded() || self.trust.receipt_for_episode(&episode)?.is_none() {
                return Err(EngineError::InvalidInput(format!(
                    "promotion replay episode {} is not a successful trusted verification",
                    replay.episode_id
                )));
            }
        }
        let verdict = spoon_adapt::PromotionGate::evaluate(replays);
        self.skills.record_replay_verdict(skill_id, &verdict)
    }

    /// Replays a candidate skill's immutable, trusted source episodes against
    /// an exact newer procedure revision owned by this engine. The caller
    /// supplies only the revision identity: expected outputs, input bindings,
    /// nested procedure versions, and challenger metrics all come from the
    /// persisted source traces and the local graph.
    ///
    /// This is the strict promotion path. It cannot accept caller-provided
    /// correctness, trace, transfer, or cost claims, and it never records a
    /// shadow verdict unless every source episode is independently replayed.
    pub fn evaluate_skill_for_shadow_with_challenger(
        &self,
        skill_id: &str,
        challenger_id: ProcedureId,
        challenger_version: u32,
    ) -> Result<crate::ManagedSkill, EngineError> {
        let skill = self.skills.get(skill_id)?.ok_or_else(|| {
            EngineError::InvalidInput(format!("unknown managed skill {skill_id}"))
        })?;
        if skill.lifecycle != crate::SkillLifecycle::Candidate {
            return Err(EngineError::InvalidInput(
                "only a candidate skill may enter shadow evaluation".into(),
            ));
        }
        if skill.candidate.failure_critic {
            return Err(EngineError::InvalidInput(
                "failure-critic skills cannot be evaluated as executable challengers".into(),
            ));
        }
        let source_episodes = skill
            .candidate
            .source_episode_ids
            .iter()
            .map(|episode_id| self.episodes.get(*episode_id))
            .collect::<Result<Vec<_>, _>>()?;
        if source_episodes.is_empty() {
            return Err(EngineError::InvalidInput(
                "candidate skill has no source episodes".into(),
            ));
        }

        let (incumbent_id, incumbent_version) = source_episodes
            .iter()
            .map(|episode| {
                if !episode.succeeded() || self.trust.receipt_for_episode(episode)?.is_none() {
                    return Err(EngineError::InvalidInput(format!(
                        "promotion source episode {} is not a successful trusted verification",
                        episode.id
                    )));
                }
                parse_procedure_action(episode.action.as_deref()).ok_or_else(|| {
                    EngineError::InvalidInput(format!(
                        "promotion source episode {} is not a versioned procedure execution",
                        episode.id
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .try_fold(None, |selected, current| {
                if let Some(selected) = selected {
                    if selected != current {
                        return Err(EngineError::InvalidInput(
                            "promotion sources must identify one procedure version".into(),
                        ));
                    }
                    Ok(Some(selected))
                } else {
                    Ok(Some(current))
                }
            })?
            .ok_or_else(|| {
                EngineError::InvalidInput("missing promotion source procedure".into())
            })?;

        if challenger_id != incumbent_id || challenger_version <= incumbent_version {
            return Err(EngineError::InvalidInput(
                "challenger must be a newer revision of the source procedure".into(),
            ));
        }
        let challenger = self
            .graph
            .get_procedure_version(challenger_id, challenger_version)?
            .ok_or_else(|| {
                EngineError::InvalidInput(format!(
                    "challenger procedure {challenger_id}@{challenger_version} does not exist"
                ))
            })?;
        if !is_current_executable(challenger.lifecycle) {
            return Err(EngineError::InvalidInput(
                "challenger procedure is not executable".into(),
            ));
        }

        let mut replays = Vec::with_capacity(source_episodes.len());
        for episode in source_episodes {
            let trace_json = episode
                .execution_trace
                .clone()
                .ok_or(EngineError::MissingTrace(episode.id))?;
            let trace: ExecTrace = serde_json::from_value(trace_json)?;
            let top = trace
                .steps
                .last()
                .ok_or(EngineError::MissingTopLevelProcedure)?;
            if top.procedure_called != Some(incumbent_id)
                || top.procedure_version != Some(incumbent_version)
            {
                return Err(EngineError::InvalidInput(format!(
                    "promotion source episode {} trace is not pinned to procedure {}@{}",
                    episode.id, incumbent_id, incumbent_version
                )));
            }
            let args = match top.input.as_ref() {
                None => Vec::new(),
                Some(Value::List(values)) => values.clone(),
                Some(_) => {
                    return Err(EngineError::InvalidInput(format!(
                        "promotion source episode {} has non-list procedure inputs",
                        episode.id
                    )));
                }
            };
            let expected = episode.observed_result.as_ref().ok_or_else(|| {
                EngineError::InvalidInput(format!(
                    "promotion source episode {} has no observed result",
                    episode.id
                ))
            })?;

            let mut evaluator = Evaluator::new().with_budget(self.max_steps);
            let mut registered = HashSet::new();
            for step in &trace.steps {
                let (Some(id), Some(version)) = (step.procedure_called, step.procedure_version)
                else {
                    return Err(EngineError::InvalidInput(format!(
                        "promotion source episode {} has an incomplete execution trace",
                        episode.id
                    )));
                };
                if registered.insert(id) {
                    if id == incumbent_id {
                        evaluator.register_procedure(challenger.clone());
                    } else {
                        let exact = self
                            .graph
                            .get_procedure_version(id, version)?
                            .ok_or_else(|| {
                                EngineError::InvalidInput(format!(
                                    "promotion source episode {} references missing procedure {}@{}",
                                    episode.id, id, version
                                ))
                            })?;
                        evaluator.register_procedure(exact);
                    }
                }
            }
            let attempt = evaluator.exec_procedure(&incumbent_id, args).ok();
            let challenger_correct = attempt
                .as_ref()
                .is_some_and(|result| &result.value == expected);
            replays.push(spoon_adapt::PromotionReplay {
                episode_id: episode.id,
                incumbent_correct: true,
                challenger_correct,
                incumbent_trace_steps: u32::try_from(trace.len()).ok(),
                challenger_trace_steps: attempt
                    .as_ref()
                    .and_then(|result| u32::try_from(result.trace.len()).ok()),
                incumbent_candidates_explored: None,
                challenger_candidates_explored: None,
                transfer: false,
            });
        }

        let verdict = spoon_adapt::PromotionGate::evaluate(replays);
        self.skills.record_replay_verdict(skill_id, &verdict)
    }

    /// A shadow candidate becomes active only after a successful strong
    /// observation from an authenticated local verifier. The full observation
    /// remains an immutable episode; promotion stores its reference as an
    /// event rather than replacing or deleting any history.
    pub fn record_skill_shadow_live_win(
        &self,
        skill_id: &str,
        observed_result: Value,
        scope: BTreeMap<String, Value>,
        evaluation: Evaluation,
        verifier_identity: &str,
    ) -> Result<crate::ManagedSkill, EngineError> {
        self.require_admin()?;
        if verifier_identity.trim().is_empty() {
            return Err(EngineError::InvalidInput(
                "shadow promotion requires an authenticated verifier identity".into(),
            ));
        }
        if !evaluation.success
            || !matches!(
                evaluation.tier,
                VerifiabilityTier::Hard | VerifiabilityTier::Consensus
            )
        {
            return Err(EngineError::InvalidInput(
                "shadow promotion requires a successful Hard or Consensus observation".into(),
            ));
        }
        let skill = self.skills.get(skill_id)?.ok_or_else(|| {
            EngineError::InvalidInput(format!("unknown managed skill {skill_id}"))
        })?;
        if skill.lifecycle != crate::SkillLifecycle::Shadow {
            return Err(EngineError::InvalidInput(
                "only a shadow skill can be promoted by a live win".into(),
            ));
        }
        let mut episode = Episode::new(format!("live shadow evaluation for skill {skill_id}"));
        episode.action = Some(format!(
            "observation:authenticated-verifier:{}",
            verifier_identity.trim()
        ));
        episode.context.environment = scope.clone();
        episode.observed_result = Some(observed_result.clone());
        episode.evaluation = Some(evaluation);
        episode.observed_facts.push(ObservedFact::new(
            "skill.shadow_live_win",
            observed_result,
            scope,
        ));
        episode.teacher_interaction = Some(serde_json::json!({
            "kind": "skill_shadow_evaluation",
            "skillId": skill_id,
            "verifier": verifier_identity.trim(),
        }));
        self.persist_engine_episode(&episode)?;
        let stored = self.episodes.get(episode.id)?;
        if !stored.succeeded() || self.trust.receipt_for_episode(&stored)?.is_none() {
            return Err(EngineError::InvalidInput(
                "shadow observation was not admitted as trusted evidence".into(),
            ));
        }
        self.skills
            .promote_from_live_shadow(skill_id, &episode.id.to_string())
    }

    /// Executes a promoted skill only when its trusted source episodes identify
    /// one stable, current procedure. Failure critics and unpromoted candidates
    /// remain non-executable evidence.
    pub fn execute_managed_skill(
        &self,
        skill_id: &str,
        inputs: BTreeMap<String, Value>,
        prediction: Option<Value>,
    ) -> Result<ExecutionOutcome, EngineError> {
        let skill = self.skills.get(skill_id)?.ok_or_else(|| {
            EngineError::InvalidInput(format!("unknown managed skill {skill_id}"))
        })?;
        if skill.lifecycle != crate::SkillLifecycle::Promoted {
            return Err(EngineError::InvalidInput(
                "only promoted skills are executable".into(),
            ));
        }
        if skill.candidate.failure_critic {
            return Err(EngineError::InvalidInput(
                "failure-critic skills are guards, not executable procedures".into(),
            ));
        }
        let mut selected: Option<(ProcedureId, u32)> = None;
        for source_id in &skill.candidate.source_episode_ids {
            let episode = self.episodes.get(*source_id)?;
            let Some(action) = episode.action.as_deref() else {
                return Err(EngineError::InvalidInput(
                    "promoted skill source has no executable procedure action".into(),
                ));
            };
            let Some(action) = action.strip_prefix("procedure:") else {
                return Err(EngineError::InvalidInput(
                    "promoted skill source is not a procedure execution".into(),
                ));
            };
            let Some((id, version)) = action.split_once('@') else {
                return Err(EngineError::InvalidInput(
                    "promoted skill source procedure action is unversioned".into(),
                ));
            };
            let procedure_id = ProcedureId(uuid::Uuid::parse_str(id).map_err(|_| {
                EngineError::InvalidInput(
                    "promoted skill source has an invalid procedure id".into(),
                )
            })?);
            let version = version.parse::<u32>().map_err(|_| {
                EngineError::InvalidInput(
                    "promoted skill source has an invalid procedure version".into(),
                )
            })?;
            if let Some((selected_id, selected_version)) = selected {
                if selected_id != procedure_id || selected_version != version {
                    return Err(EngineError::InvalidInput(
                        "promoted skill sources must identify one procedure version".into(),
                    ));
                }
            } else {
                selected = Some((procedure_id, version));
            }
        }
        let Some((procedure_id, _version)) = selected else {
            return Err(EngineError::InvalidInput(
                "promoted skill has no source episodes".into(),
            ));
        };
        let outcome = self.execute_procedure(procedure_id, inputs, prediction)?;
        self.skills
            .record_experience(skill_id, outcome.episode.succeeded())?;
        Ok(outcome)
    }

    pub fn execute_best_managed_skill(
        &self,
        query: &str,
        inputs: BTreeMap<String, Value>,
        prediction: Option<Value>,
    ) -> Result<ExecutionOutcome, EngineError> {
        let skill = self
            .select_executable_managed_skill(query)?
            .ok_or_else(|| {
                EngineError::InvalidInput("no promoted executable skill matches query".into())
            })?;
        self.execute_managed_skill(&skill.id, inputs, prediction)
    }

    /// Retirement changes ranking eligibility but retains the candidate,
    /// evidence, and explicit successor linkage for reconstruction.
    pub fn retire_managed_skill(
        &self,
        skill_id: &str,
        successor_skill: &str,
        reason: &str,
    ) -> Result<crate::ManagedSkill, EngineError> {
        if skill_id == successor_skill {
            return Err(EngineError::InvalidInput(
                "a skill cannot retire in favor of itself".into(),
            ));
        }
        let successor = self.skills.get(successor_skill)?.ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "retirement successor {successor_skill} is not a managed skill"
            ))
        })?;
        let retired = self.skills.get(skill_id)?.ok_or_else(|| {
            EngineError::InvalidInput(format!("unknown managed skill {skill_id}"))
        })?;
        if successor.lifecycle != crate::SkillLifecycle::Promoted {
            return Err(EngineError::InvalidInput(
                "retirement requires a promoted successor with live verification".into(),
            ));
        }
        if successor.candidate.failure_critic {
            return Err(EngineError::InvalidInput(
                "a failure critic cannot be the successor of an executable skill".into(),
            ));
        }
        if successor.shadow_live_wins == 0 {
            return Err(EngineError::InvalidInput(
                "retirement requires a successor with a trusted live behavior check".into(),
            ));
        }
        let retired_source_episodes = retired
            .candidate
            .source_episode_ids
            .iter()
            .map(|episode_id| self.episodes.get(*episode_id))
            .collect::<Result<Vec<_>, _>>()?;
        let successor_source_episodes = successor
            .candidate
            .source_episode_ids
            .iter()
            .map(|episode_id| self.episodes.get(*episode_id))
            .collect::<Result<Vec<_>, _>>()?;
        let retired_source_ids = retired
            .candidate
            .source_episode_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if !retired_source_ids
            .iter()
            .all(|episode_id| successor.candidate.source_episode_ids.contains(episode_id))
        {
            return Err(EngineError::InvalidInput(
                "retirement requires successor evidence covering every retired source episode"
                    .into(),
            ));
        }
        let additional_successor_episode_ids = successor
            .candidate
            .source_episode_ids
            .iter()
            .filter(|episode_id| !retired_source_ids.contains(episode_id))
            .copied()
            .collect::<Vec<_>>();
        if additional_successor_episode_ids.is_empty() {
            return Err(EngineError::InvalidInput(
                "retirement requires additional successor behavior evidence".into(),
            ));
        }
        for episode in retired_source_episodes
            .iter()
            .chain(successor_source_episodes.iter())
        {
            if !episode.succeeded() || self.trust.receipt_for_episode(episode)?.is_none() {
                return Err(EngineError::InvalidInput(format!(
                    "retirement behavior evidence episode {} is not a successful trusted observation",
                    episode.id
                )));
            }
        }
        let retired_digests = retired_source_episodes
            .iter()
            .map(behavior_digest)
            .collect::<Result<Vec<_>, _>>()?;
        let successor_digests = successor_source_episodes
            .iter()
            .map(behavior_digest)
            .collect::<Result<Vec<_>, _>>()?;
        let additional_digests = additional_successor_episode_ids
            .iter()
            .map(|episode_id| {
                successor_source_episodes
                    .iter()
                    .find(|episode| episode.id == *episode_id)
                    .ok_or_else(|| {
                        EngineError::InvalidInput(format!(
                            "missing successor behavior evidence episode {episode_id}"
                        ))
                    })
                    .and_then(behavior_digest)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut covered_behavior_digests = retired_digests.clone();
        covered_behavior_digests.sort();
        covered_behavior_digests.dedup();
        if covered_behavior_digests
            .iter()
            .any(|digest| !successor_digests.contains(digest))
        {
            return Err(EngineError::InvalidInput(
                "retirement successor does not cover every retired behavior shape".into(),
            ));
        }
        if covered_behavior_digests
            .iter()
            .any(|digest| !additional_digests.contains(digest))
        {
            return Err(EngineError::InvalidInput(
                "retirement successor has no new evidence for a retired behavior shape".into(),
            ));
        }
        let record = spoon_adapt::retire_skill_with_evidence(
            skill_id,
            successor_skill,
            reason,
            spoon_adapt::BehavioralSubsumptionEvidence {
                retired_source_episode_ids: retired.candidate.source_episode_ids.clone(),
                successor_source_episode_ids: successor.candidate.source_episode_ids.clone(),
                covered_behavior_digests,
                additional_successor_episode_ids,
            },
        );
        self.skills.retire(skill_id, &record)
    }

    pub fn record_ranking_example(&self, example: &RankingExample) -> Result<(), EngineError> {
        Ok(self.intuition.record_ranking_example(example)?)
    }

    pub fn generate_self_supervision(
        &self,
        source_episode: Option<&str>,
        input: serde_json::Value,
        target: serde_json::Value,
        kind: &str,
        grounded: bool,
    ) -> Result<SupervisionTask, EngineError> {
        if grounded {
            let source = source_episode.ok_or_else(|| {
                EngineError::InvalidInput(
                    "grounded supervision requires a source episode receipt".into(),
                )
            })?;
            let uuid = uuid::Uuid::parse_str(source).map_err(|_| {
                EngineError::InvalidInput("grounded supervision source episode is invalid".into())
            })?;
            let episode = self.episodes.get(EpisodeId(uuid))?;
            if self.trust.receipt_for_episode(&episode)?.is_none() {
                return Err(EngineError::InvalidInput(
                    "grounded supervision requires an exact Engine trust receipt".into(),
                ));
            }
        }
        Ok(self.intuition.generate_self_supervision(
            source_episode,
            input,
            target,
            kind,
            grounded,
        )?)
    }

    /// Derive and immediately terminate one challenge from a successful,
    /// trusted execution episode. The target is the episode's observed result
    /// and the verifier is an exact-version replay with a much smaller local
    /// step budget. This records supervision evidence only; it never promotes
    /// a claim, changes lifecycle, or mints trust.
    pub fn generate_grounded_self_supervision_from_episode(
        &self,
        source_episode: EpisodeId,
    ) -> Result<SupervisionTask, EngineError> {
        let episode = self.episodes.get(source_episode)?;
        let receipt = self.trust.receipt_for_episode(&episode)?;
        if receipt.is_none()
            || !episode.succeeded()
            || !episode.evaluation.as_ref().is_some_and(|evaluation| {
                matches!(
                    evaluation.tier,
                    VerifiabilityTier::Hard | VerifiabilityTier::Consensus
                )
            })
        {
            return Err(EngineError::InvalidInput(
                "grounded self-supervision requires a trusted successful Hard or Consensus episode"
                    .into(),
            ));
        }
        let observed = episode.observed_result.clone().ok_or_else(|| {
            EngineError::InvalidInput(
                "grounded self-supervision requires an externally terminated observed result"
                    .into(),
            )
        })?;
        if episode.observed_facts.is_empty() {
            return Err(EngineError::InvalidInput(
                "grounded self-supervision requires an authenticated observed fact".into(),
            ));
        }
        let trace_json = episode.execution_trace.clone().ok_or_else(|| {
            EngineError::InvalidInput(
                "grounded self-supervision requires a replayable execution trace".into(),
            )
        })?;
        let trace: ExecTrace = serde_json::from_value(trace_json)?;
        let top = trace.steps.last().ok_or_else(|| {
            EngineError::InvalidInput(
                "grounded self-supervision requires a non-empty execution trace".into(),
            )
        })?;
        let procedure = top.procedure_called.ok_or_else(|| {
            EngineError::InvalidInput(
                "grounded self-supervision trace lacks a top-level procedure".into(),
            )
        })?;
        let version = top.procedure_version.ok_or_else(|| {
            EngineError::InvalidInput(
                "grounded self-supervision trace lacks an exact procedure version".into(),
            )
        })?;
        let source = source_episode.to_string();
        let input = serde_json::json!({
            "challenge": "replay_exact_trace",
            "sourceEpisode": source,
            "procedure": procedure.to_string(),
            "procedureVersion": version,
            "inputs": top.input,
        });
        let target = serde_json::json!({
            "observedResult": observed,
        });
        let task = self
            .intuition
            .begin_verified_trace_replay(&source, input, target)?;

        let verifier = "bounded_exact_trace_replay_v1";
        let completion = match self.replay_episode_with_budget(
            source_episode,
            BTreeMap::new(),
            MAX_GROUNDED_SUPERVISION_REPLAY_STEPS,
        ) {
            Ok(replay) => {
                let matches_observation = replay.value == observed;
                (
                    matches_observation,
                    serde_json::json!({
                        "status": if matches_observation { "matched" } else { "mismatched" },
                        "expected": observed,
                        "actual": replay.value,
                        "traceSteps": replay.trace.len(),
                        "maxSteps": MAX_GROUNDED_SUPERVISION_REPLAY_STEPS,
                    }),
                )
            }
            Err(error) => (
                false,
                serde_json::json!({
                    "status": "replay_error",
                    "error": error.to_string(),
                    "maxSteps": MAX_GROUNDED_SUPERVISION_REPLAY_STEPS,
                }),
            ),
        };
        Ok(self.intuition.complete_verified_trace_replay(
            task.id,
            completion.0,
            verifier,
            completion.1,
        )?)
    }

    pub fn intuition_metrics(&self) -> Result<IntuitionMetrics, EngineError> {
        Ok(self.intuition.metrics()?)
    }

    pub fn train_representation_model(
        &self,
        holdout_tasks: usize,
    ) -> Result<spoon_intuition::RepresentationModel, EngineError> {
        Ok(self.intuition.train_representation_model(holdout_tasks)?)
    }

    pub fn activate_representation_model(
        &self,
        model_id: i64,
    ) -> Result<spoon_intuition::RepresentationModel, EngineError> {
        Ok(self.intuition.activate_representation_model(model_id)?)
    }

    pub fn evaluate_representation_model(
        &self,
        model_id: i64,
        holdout_queries: usize,
    ) -> Result<spoon_intuition::RepresentationRegressionEvaluation, EngineError> {
        Ok(self
            .intuition
            .evaluate_representation_model(model_id, holdout_queries)?)
    }

    pub fn latest_representation_model(
        &self,
    ) -> Result<Option<spoon_intuition::RepresentationModel>, EngineError> {
        Ok(self.intuition.latest_representation_model()?)
    }

    pub fn metrics_snapshot(&self) -> Result<MetricsSnapshot, EngineError> {
        let teacher = self.episodes.teacher_interaction_metrics()?;
        let skills = self.list_managed_skills(512)?;
        let phase6 = phase6_evidence_metrics(teacher, &skills);
        Ok(MetricsSnapshot {
            episode_count: self.episodes.count()?,
            verified_answer_count: self.regression.count()?,
            rung_distribution: self.episodes.rung_distribution()?,
            intuition: self.intuition.metrics()?,
            phase6,
            section38: self.telemetry.snapshot()?,
        })
    }

    /// Starts an immutable benchmark/probe run. Measurements added to this run
    /// are validated before persistence and cannot be edited in place.
    pub fn create_falsification_run(
        &self,
        input: crate::FalsificationRunInput,
    ) -> Result<crate::FalsificationRun, EngineError> {
        self.telemetry.create_run(input)
    }

    /// Persists one falsification measurement. The telemetry store rejects
    /// teacher-off leakage and undeclared exact repeats before accepting it.
    pub fn record_falsification_measurement(
        &self,
        run_id: &str,
        input: crate::FalsificationMeasurementInput,
    ) -> Result<crate::FalsificationMeasurement, EngineError> {
        self.telemetry.record(run_id, input)
    }

    pub fn observe_native_primitive(
        &self,
        target: &str,
    ) -> Result<spoon_capability::PrimitiveExecution, EngineError> {
        if target != "clock" {
            return Err(EngineError::InvalidInput(
                "only the local clock observation is enabled".into(),
            ));
        }
        let policy = spoon_capability::PrimitivePolicy {
            observe_targets: std::collections::BTreeSet::from([target.to_owned()]),
            ..spoon_capability::PrimitivePolicy::default()
        };
        spoon_capability::NativePrimitiveExecutor::new(policy)
            .observe(&spoon_capability::PrimitiveRequest::Observe {
                target: target.to_owned(),
            })
            .map_err(|error| EngineError::InvalidInput(format!("native observation: {error}")))
    }

    pub fn generate_epistemic_challenge(
        &self,
        source_episode: Option<&str>,
        kind: EpistemicChallengeKind,
        input: serde_json::Value,
        expected: serde_json::Value,
        grounded: bool,
    ) -> Result<SupervisionTask, EngineError> {
        if grounded {
            let source = source_episode.ok_or_else(|| {
                EngineError::InvalidInput(
                    "grounded challenge requires a source episode receipt".into(),
                )
            })?;
            let uuid = uuid::Uuid::parse_str(source).map_err(|_| {
                EngineError::InvalidInput("grounded challenge source episode is invalid".into())
            })?;
            let episode = self.episodes.get(EpisodeId(uuid))?;
            if self.trust.receipt_for_episode(&episode)?.is_none() {
                return Err(EngineError::InvalidInput(
                    "grounded challenge requires an exact Engine trust receipt".into(),
                ));
            }
        }
        Ok(self.intuition.generate_epistemic_challenge(
            source_episode,
            kind,
            input,
            expected,
            grounded,
        )?)
    }

    pub fn graph(&self) -> crate::GraphView<'_> {
        crate::GraphView { store: &self.graph }
    }

    pub fn episodes(&self) -> crate::EpisodeView<'_> {
        crate::EpisodeView {
            store: &self.episodes,
        }
    }

    pub fn create_session(
        &self,
        name: Option<String>,
        visibility: SessionVisibility,
    ) -> Result<Session, EngineError> {
        Ok(self.episodes.create_session(name, visibility)?)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, EngineError> {
        Ok(self.episodes.list_sessions()?)
    }

    pub fn get_session(&self, id_or_name: &str) -> Result<Option<Session>, EngineError> {
        Ok(self.episodes.get_session(id_or_name)?)
    }

    pub fn end_session(&self, id_or_name: &str) -> Result<Session, EngineError> {
        Ok(self.episodes.end_session(id_or_name)?)
    }

    pub fn enable_admin(&mut self, secret: &str) -> Result<(), EngineError> {
        self.runtime.configure_or_verify_admin(secret)?;
        self.admin_enabled = true;
        Ok(())
    }

    pub(crate) fn require_admin(&self) -> Result<(), EngineError> {
        if self.admin_enabled {
            Ok(())
        } else {
            Err(EngineError::InvalidInput(
                "engine admin authority is required for raw persistence mutation".into(),
            ))
        }
    }

    pub fn admin_insert_concept(&self, concept: &Concept) -> Result<(), EngineError> {
        self.require_admin()?;
        self.graph.insert_concept(concept)?;
        self.index_concept(concept)?;
        Ok(())
    }

    pub fn admin_update_concept(&self, concept: &Concept) -> Result<(), EngineError> {
        self.require_admin()?;
        self.graph.update_concept(concept)?;
        self.index_concept(concept)?;
        Ok(())
    }

    pub fn admin_revise_concept(
        &self,
        concept: &Concept,
        expected_version: u32,
    ) -> Result<u32, EngineError> {
        self.require_admin()?;
        let version = self.graph.revise_concept(concept, expected_version)?;
        self.index_concept(concept)?;
        Ok(version)
    }

    pub fn admin_delete_concept(&self, id: ConceptId) -> Result<(), EngineError> {
        self.require_admin()?;
        self.graph.delete_concept(id)?;
        self.intuition.remove_document(&format!("concept:{id}"))?;
        Ok(())
    }

    pub(crate) fn index_concept(&self, concept: &Concept) -> Result<(), EngineError> {
        self.intuition.index_document(&RecallDocument {
            id: format!("concept:{}", concept.id),
            kind: RecallKind::Concept,
            text: format!(
                "{} {}",
                concept.name,
                concept.description.as_deref().unwrap_or_default()
            ),
            concept_ids: vec![concept.id.to_string()],
            created_at: concept.created_at,
        })?;
        Ok(())
    }

    pub(crate) fn index_procedure(&self, procedure: &Procedure) -> Result<(), EngineError> {
        self.intuition
            .remove_documents_with_prefix(&format!("procedure:{}:", procedure.id))?;
        self.intuition.index_document(&RecallDocument {
            id: format!("procedure:{}:{}", procedure.id, procedure.version),
            kind: RecallKind::Procedure,
            text: format!(
                "{} {:?} {:?}",
                procedure.name, procedure.params, procedure.contract
            ),
            concept_ids: procedure
                .concept
                .map(|id| vec![id.to_string()])
                .unwrap_or_default(),
            created_at: procedure.created_at,
        })?;
        Ok(())
    }

    fn index_episode(&self, episode: &Episode) -> Result<(), EngineError> {
        self.intuition.index_document(&RecallDocument {
            id: format!("episode:{}", episode.id),
            kind: RecallKind::Episode,
            text: format!(
                "{} {}",
                episode.situation,
                episode.action.as_deref().unwrap_or_default()
            ),
            concept_ids: episode
                .context
                .entities
                .iter()
                .map(ToString::to_string)
                .collect(),
            created_at: episode.created_at,
        })?;
        Ok(())
    }

    fn rebuild_intuition_index(&self) -> Result<(), EngineError> {
        self.intuition.clear_documents()?;
        for concept in self.graph.list_concepts()? {
            self.index_concept(&concept)?;
        }
        for procedure in self.graph.list_procedures()? {
            self.index_procedure(&procedure)?;
        }
        for episode in self.episodes.list_recent(u32::MAX)? {
            self.index_episode(&episode)?;
        }
        Ok(())
    }

    pub fn admin_insert_relationship(
        &self,
        relationship: &Relationship,
    ) -> Result<(), EngineError> {
        self.require_admin()?;
        Ok(self.graph.insert_relationship(relationship)?)
    }

    pub fn admin_update_relationship(
        &self,
        relationship: &Relationship,
    ) -> Result<(), EngineError> {
        self.require_admin()?;
        Ok(self.graph.update_relationship(relationship)?)
    }

    pub fn admin_revise_relationship(
        &self,
        relationship: &Relationship,
        expected_version: u32,
    ) -> Result<u32, EngineError> {
        self.require_admin()?;
        Ok(self
            .graph
            .revise_relationship(relationship, expected_version)?)
    }

    pub fn admin_delete_relationship(&self, id: RelationshipId) -> Result<(), EngineError> {
        self.require_admin()?;
        Ok(self.graph.delete_relationship(id)?)
    }

    pub fn admin_insert_procedure(&self, procedure: &Procedure) -> Result<(), EngineError> {
        self.require_admin()?;
        self.graph.insert_procedure(procedure)?;
        self.index_procedure(procedure)?;
        Ok(())
    }

    pub fn admin_update_procedure(&self, procedure: &Procedure) -> Result<(), EngineError> {
        self.require_admin()?;
        self.graph.update_procedure(procedure)?;
        self.index_procedure(procedure)?;
        Ok(())
    }

    pub fn admin_revise_procedure(
        &self,
        procedure: &Procedure,
        expected_version: u32,
    ) -> Result<(), EngineError> {
        self.require_admin()?;
        self.graph.revise_procedure(procedure, expected_version)?;
        self.index_procedure(procedure)?;
        Ok(())
    }

    pub fn admin_delete_procedure(&self, id: ProcedureId) -> Result<(), EngineError> {
        self.require_admin()?;
        self.graph.delete_procedure(id)?;
        self.intuition
            .remove_documents_with_prefix(&format!("procedure:{id}:"))?;
        Ok(())
    }

    pub fn admin_insert_episode(&self, episode: &Episode) -> Result<(), EngineError> {
        self.require_admin()?;
        Ok(self.episodes.insert(episode)?)
    }

    pub fn admin_append_feedback(
        &self,
        feedback: &EpisodeFeedback,
    ) -> Result<EpisodeFeedback, EngineError> {
        self.require_admin()?;
        Ok(self.episodes.append_feedback(feedback)?)
    }

    /// Persists a strong verifier observation and binds authority to the exact
    /// feedback bytes. This is deliberately separate from raw/admin feedback:
    /// callers cannot promote a row merely by choosing a strong tier enum.
    pub fn record_authenticated_verifier_feedback(
        &self,
        feedback: &EpisodeFeedback,
        verifier_identity: &str,
    ) -> Result<EpisodeFeedback, EngineError> {
        self.require_admin()?;
        self.runtime.stage_feedback_saga(
            &feedback.id.to_string(),
            &serde_json::to_string(feedback)?,
            verifier_identity,
        )?;
        self.finish_authenticated_feedback_saga(feedback, verifier_identity)
    }

    pub fn record_external_feedback(
        &self,
        feedback: &EpisodeFeedback,
    ) -> Result<EpisodeFeedback, EngineError> {
        if feedback.evaluation.tier != VerifiabilityTier::Deferred {
            return Err(EngineError::InvalidInput(
                "raw external feedback must enter the engine as Deferred evidence".into(),
            ));
        }
        Ok(self.episodes.append_feedback(feedback)?)
    }

    /// Records a verifier-attested semantic observation. Unlike raw/admin
    /// episode insertion, this creates exact episode and fact receipts; its
    /// verifier identity and scoped-environment digest are part of the signed
    /// immutable evidence. The caller still needs local admin authority:
    /// imported trust never transfers automatically.
    pub fn record_authenticated_observation(
        &self,
        predicate: impl Into<String>,
        value: Value,
        scope: BTreeMap<String, Value>,
        evaluation: Evaluation,
        verifier_identity: &str,
    ) -> Result<Episode, EngineError> {
        self.require_admin()?;
        if verifier_identity.trim().is_empty() {
            return Err(EngineError::InvalidInput(
                "authenticated verifier identity must be non-empty".into(),
            ));
        }
        if !matches!(
            evaluation.tier,
            VerifiabilityTier::Hard | VerifiabilityTier::Consensus
        ) {
            return Err(EngineError::InvalidInput(
                "authenticated observations must use Hard or Consensus evidence".into(),
            ));
        }
        let predicate = predicate.into();
        let mut episode = Episode::new(format!("authenticated observation: {predicate}"));
        episode.action = Some(format!(
            "observation:authenticated-verifier:{}",
            verifier_identity.trim()
        ));
        episode.context.environment = scope.clone();
        episode.observed_result = Some(value.clone());
        episode.evaluation = Some(evaluation);
        episode
            .observed_facts
            .push(ObservedFact::new(predicate, value, scope));
        normalize_observed_facts(&mut episode, Some(verifier_identity));
        self.persist_engine_episode(&episode)?;
        Ok(episode)
    }

    /// Returns a durable Engine receipt only when this exact persisted episode
    /// was evaluated by the Engine at a strong verification tier.
    pub fn trust_receipt_for_episode(
        &self,
        episode: &Episode,
    ) -> Result<Option<crate::TrustReceipt>, EngineError> {
        self.trust.receipt_for_episode(episode)
    }

    /// Looks up the exact receipt for one immutable observed fact. The fact
    /// must be the embedded value from its source episode; reconstructed or
    /// altered values do not match its digest.
    pub fn trust_receipt_for_fact(
        &self,
        episode: &Episode,
        fact: &ObservedFact,
    ) -> Result<Option<crate::TrustReceipt>, EngineError> {
        self.trust.verified_fact(episode, fact)
    }

    /// Returns the exact verifier receipt for authenticated late feedback.
    pub fn trust_receipt_for_feedback(
        &self,
        feedback: &EpisodeFeedback,
    ) -> Result<Option<crate::TrustReceipt>, EngineError> {
        self.trust
            .verified_feedback(feedback, feedback.evaluation.tier)
    }

    pub fn execute_procedure(
        &self,
        procedure_id: ProcedureId,
        inputs: BTreeMap<String, Value>,
        prediction: Option<Value>,
    ) -> Result<ExecutionOutcome, EngineError> {
        let procedure = self
            .graph
            .get_procedure(procedure_id)?
            .ok_or_else(|| SpoonError::NotFound(format!("procedure {procedure_id}")))?;
        if !is_current_executable(procedure.lifecycle) {
            return Err(EngineError::InvalidInput(format!(
                "procedure {procedure_id} is not executable in lifecycle {:?}",
                procedure.lifecycle
            )));
        }
        let args = bind_inputs(&procedure, &inputs, None)?;
        let mut evaluator = self.evaluator_for_procedure(&procedure)?;
        let attempt = evaluator.exec_procedure_captured(&procedure_id, args);
        let steps_used = evaluator.budget().steps_used;
        match attempt.result {
            Ok(value) => {
                let episode = self.record_execution(
                    &procedure,
                    &inputs,
                    prediction,
                    Some(value.clone()),
                    &attempt.trace,
                    None,
                    steps_used,
                )?;
                Ok(ExecutionOutcome {
                    value,
                    trace: attempt.trace,
                    episode,
                })
            }
            Err(source) => {
                let episode = self.record_execution(
                    &procedure,
                    &inputs,
                    prediction,
                    None,
                    &attempt.trace,
                    Some(&source),
                    steps_used,
                )?;
                Err(EngineError::ExecutionFailed {
                    episode_id: episode.id,
                    source,
                })
            }
        }
    }

    pub fn replay_episode(
        &self,
        episode_id: EpisodeId,
        substitutions: BTreeMap<String, Value>,
    ) -> Result<ReplayOutcome, EngineError> {
        self.replay_episode_with_budget(episode_id, substitutions, self.max_steps)
    }

    fn replay_episode_with_budget(
        &self,
        episode_id: EpisodeId,
        substitutions: BTreeMap<String, Value>,
        max_steps: u32,
    ) -> Result<ReplayOutcome, EngineError> {
        let episode = self.episodes.get(episode_id)?;
        let trace_json = episode
            .execution_trace
            .ok_or(EngineError::MissingTrace(episode_id))?;
        let trace: ExecTrace = serde_json::from_value(trace_json)?;
        let top = trace
            .steps
            .last()
            .ok_or(EngineError::MissingTopLevelProcedure)?;
        let procedure_id = top
            .procedure_called
            .ok_or(EngineError::MissingTopLevelProcedure)?;
        let version = top
            .procedure_version
            .ok_or(EngineError::MissingTopLevelProcedure)?;
        let procedure = self
            .graph
            .get_procedure_version(procedure_id, version)?
            .ok_or_else(|| SpoonError::NotFound(format!("procedure {procedure_id} v{version}")))?;
        let original = top.input.as_ref().and_then(Value::as_list);
        let args = bind_inputs(&procedure, &substitutions, original)?;

        let mut evaluator = Evaluator::new().with_budget(max_steps.max(1));
        let mut registered = HashSet::new();
        for step in &trace.steps {
            let (Some(id), Some(version)) = (step.procedure_called, step.procedure_version) else {
                continue;
            };
            if registered.insert(id) {
                let exact = self
                    .graph
                    .get_procedure_version(id, version)?
                    .ok_or_else(|| SpoonError::NotFound(format!("procedure {id} v{version}")))?;
                evaluator.register_procedure(exact);
            }
        }

        let replayed = evaluator.replay(&trace, args)?;
        Ok(ReplayOutcome {
            value: replayed.value,
            trace: replayed.trace,
            source_episode: episode_id,
        })
    }

    pub(crate) fn current_evaluator(&self) -> Result<Evaluator, EngineError> {
        let mut evaluator = Evaluator::new().with_budget(self.max_steps);
        for procedure in self.graph.list_procedures()? {
            if is_current_executable(procedure.lifecycle) {
                evaluator.register_procedure(procedure);
            }
        }
        Ok(evaluator)
    }

    /// Build an evaluator for a stored procedure while restoring every exact
    /// dependency snapshot it declares. Current procedures remain available
    /// for legacy calls; an exact dependency intentionally overrides its
    /// current revision so a learned composition cannot drift after revision.
    pub(crate) fn evaluator_for_procedure(
        &self,
        procedure: &Procedure,
    ) -> Result<Evaluator, EngineError> {
        let mut evaluator = self.current_evaluator()?;
        let mut dependencies = HashSet::new();
        collect_exact_calls(&procedure.body, &mut dependencies);
        for condition in procedure
            .contract
            .requires
            .iter()
            .chain(&procedure.contract.promises)
            .chain(&procedure.contract.fails_when)
        {
            if let Some(check) = &condition.check {
                collect_exact_calls(check, &mut dependencies);
            }
        }
        for (id, version) in dependencies {
            let current = self.graph.get_procedure(id)?.ok_or_else(|| {
                EngineError::InvalidInput(format!("exact dependency {id}@{version} is absent"))
            })?;
            if !is_current_executable(current.lifecycle) {
                return Err(EngineError::InvalidInput(format!(
                    "exact dependency {id}@{version} is not currently executable"
                )));
            }
            let exact = self
                .graph
                .get_procedure_version(id, version)?
                .ok_or_else(|| {
                    EngineError::InvalidInput(format!(
                        "exact dependency {id}@{version} no longer has a stored revision"
                    ))
                })?;
            evaluator.register_procedure(exact);
        }
        Ok(evaluator)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_execution(
        &self,
        procedure: &Procedure,
        inputs: &BTreeMap<String, Value>,
        prediction: Option<Value>,
        observed: Option<Value>,
        trace: &ExecTrace,
        failure: Option<&SpoonError>,
        steps_used: u32,
    ) -> Result<Episode, EngineError> {
        let mut episode = Episode::new(format!("execute {}", procedure.name));
        if let Some(concept) = procedure.concept {
            episode.context.entities.push(concept);
        }
        episode.context.environment = inputs.clone();
        episode.prediction = prediction.clone();
        episode.action = Some(format!("procedure:{}@{}", procedure.id, procedure.version));
        episode.observed_result = observed.clone();
        episode.evaluation = match (failure, prediction.as_ref(), observed.as_ref()) {
            (Some(error), _, _) => Some(Evaluation {
                tier: VerifiabilityTier::Hard,
                success: false,
                details: error.to_string(),
                surprise: prediction.as_ref().map(|_| 1.0),
            }),
            (None, Some(expected), Some(actual)) => Some(evaluate_deterministic(expected, actual)),
            _ => None,
        };
        if episode.evaluation.as_ref().is_some_and(|evaluation| {
            evaluation.success
                && matches!(
                    evaluation.tier,
                    VerifiabilityTier::Hard | VerifiabilityTier::Consensus
                )
        }) && let Some(value) = observed
        {
            episode.observed_facts.push(observed_fact_for_procedure(
                procedure,
                value.clone(),
                inputs.clone(),
            ));
        }
        episode.reasoning_trace = reasoning_trace(trace);
        episode.execution_trace = Some(serde_json::to_value(trace)?);
        episode.cost = EpisodeCost {
            rung_reached: EscalationRung::Run,
            steps_taken: trace.len() as u32,
            budget_spent: f64::from(steps_used),
        };
        normalize_observed_facts(&mut episode, None);
        self.persist_engine_episode(&episode)?;
        Ok(episode)
    }

    pub(crate) fn persist_engine_episode(&self, episode: &Episode) -> Result<(), EngineError> {
        let mut episode = episode.clone();
        let verifier = authenticated_observation_verifier(&episode).map(str::to_owned);
        normalize_observed_facts(&mut episode, verifier.as_deref());
        validate_observed_facts(&episode)?;
        self.runtime.stage_episode_saga(
            &episode.id.to_string(),
            &serde_json::to_string(&episode)?,
            None,
        )?;
        self.finish_engine_episode_saga(&episode)?;
        Ok(())
    }

    pub(crate) fn persist_engine_episode_with_pending(
        &self,
        episode: &Episode,
        cycle_id: crate::CycleId,
        pending: &crate::cycle::PendingCycle,
    ) -> Result<(), EngineError> {
        let mut episode = episode.clone();
        let verifier = authenticated_observation_verifier(&episode).map(str::to_owned);
        normalize_observed_facts(&mut episode, verifier.as_deref());
        validate_observed_facts(&episode)?;
        let pending_json = serde_json::to_string(pending)?;
        self.runtime.stage_episode_saga(
            &episode.id.to_string(),
            &serde_json::to_string(&episode)?,
            Some((cycle_id, self.instance_id, &pending_json)),
        )?;
        self.finish_engine_episode_saga(&episode)
    }

    fn recover_pending_episode_sagas(&self) -> Result<(), EngineError> {
        for episode_json in self.runtime.pending_episode_sagas()? {
            let episode: Episode = serde_json::from_str(&episode_json)?;
            validate_observed_facts(&episode)?;
            self.finish_engine_episode_saga(&episode)?;
        }
        Ok(())
    }

    fn recover_pending_feedback_sagas(&self) -> Result<(), EngineError> {
        for (feedback_json, verifier_identity) in self.runtime.pending_feedback_sagas()? {
            let feedback: EpisodeFeedback = serde_json::from_str(&feedback_json)?;
            self.finish_authenticated_feedback_saga(&feedback, &verifier_identity)?;
        }
        Ok(())
    }

    fn finish_authenticated_feedback_saga(
        &self,
        feedback: &EpisodeFeedback,
        verifier_identity: &str,
    ) -> Result<EpisodeFeedback, EngineError> {
        let stored = self.episodes.append_feedback(feedback)?;
        self.trust
            .mint_authenticated_feedback(&stored, verifier_identity)?;
        self.runtime
            .complete_feedback_saga(&stored.id.to_string())?;
        Ok(stored)
    }

    fn finish_engine_episode_saga(&self, episode: &Episode) -> Result<(), EngineError> {
        match self.episodes.get(episode.id) {
            Ok(stored) if serde_json::to_vec(&stored)? == serde_json::to_vec(episode)? => {}
            Ok(_) => {
                return Err(EngineError::InvalidInput(format!(
                    "episode persistence saga conflicts with existing episode {}",
                    episode.id
                )));
            }
            Err(SpoonError::NotFound(_)) => self.episodes.insert(episode)?,
            Err(error) => return Err(error.into()),
        }
        if let Some(verifier) = authenticated_observation_verifier(episode) {
            self.trust.mint_authenticated_episode(episode, verifier)?;
        } else {
            self.trust.mint_engine_episode(episode)?;
        }
        self.trust.mint_episode_facts(episode)?;
        // A saga remains until every derived authority item and contradiction
        // has been written, so a restart completes this exact immutable work.
        self.detect_contradictions_for_trusted_episode(episode)?;
        self.index_episode(episode)?;
        self.record_episode_learning(episode)?;
        self.record_verified_regression(episode)?;
        self.regression.record(episode)?;
        self.record_episode_curiosity(episode)?;
        self.runtime
            .complete_episode_saga(&episode.id.to_string())?;
        Ok(())
    }

    /// Successful deterministic procedure episodes become local regression
    /// evidence only after the episode has passed the full Engine trust saga.
    /// This keeps the suite useful for promotion without allowing callers to
    /// inject arbitrary expected outputs or verification tiers.
    fn record_verified_regression(&self, episode: &Episode) -> Result<(), EngineError> {
        let Some(evaluation) = episode.evaluation.as_ref() else {
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
        let Some(observed_result) = episode.observed_result.clone() else {
            return Ok(());
        };
        let Some(action) = episode.action.as_deref() else {
            return Ok(());
        };
        let Some(versioned_procedure) = action.strip_prefix("procedure:") else {
            return Ok(());
        };
        let Some((procedure_id, version)) = versioned_procedure.split_once('@') else {
            return Ok(());
        };
        let Ok(procedure_id) = uuid::Uuid::parse_str(procedure_id) else {
            return Ok(());
        };
        let Ok(procedure_version) = version.parse::<u32>() else {
            return Ok(());
        };
        let test_case = TestCase {
            inputs: episode
                .context
                .environment
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            expected_output: observed_result,
            from_episode: Some(episode.id),
            tier: evaluation.tier,
        };
        self.episodes
            .record_verified_regression_case(&VerifiedRegressionCase {
                episode_id: episode.id,
                procedure_id: ProcedureId(procedure_id),
                procedure_version,
                test_case,
            })?;
        Ok(())
    }

    fn record_episode_curiosity(&self, episode: &Episode) -> Result<(), EngineError> {
        // Episode records are finalized before this saga runs.  Derivation is
        // therefore read-only over immutable evidence, and the deterministic
        // gap IDs make recovery/retry idempotent.
        let recent = self.episodes.list_recent(256)?;
        for gap in crate::goals::derive_episode_curiosity_gaps(episode, &recent) {
            self.goals.record_gap(&gap)?;
        }
        // Contradictions are derived from trusted immutable facts above.  A
        // held contradiction is also a curiosity gap, but this merely records
        // the missing discriminator; it does not refine or mutate a claim.
        for contradiction in self.contradictions.list_held()? {
            let gap = crate::goals::held_contradiction_gap(&contradiction);
            self.goals.record_gap(&gap)?;
        }
        Ok(())
    }

    /// Convert the episode's explicit considered/used material into ranking
    /// supervision. This is representation learning only: it records what
    /// happened, but never changes graph truth, lifecycle, or trust receipts.
    fn record_episode_learning(&self, episode: &Episode) -> Result<(), EngineError> {
        let query = episode.situation.clone();
        let succeeded = episode.succeeded();
        let rung = episode.cost.rung_reached as u8;
        for candidate in &episode.knowledge_considered {
            self.record_ranking_example(&RankingExample {
                query: query.clone(),
                candidate_id: format!("concept:{}", candidate.concept),
                used: candidate.was_used,
                succeeded,
                rung,
            })?;
        }
        for procedure in &episode.context.relevant_procedures {
            self.record_ranking_example(&RankingExample {
                query: query.clone(),
                candidate_id: format!("procedure:{}:{}", procedure.id, procedure.version),
                used: episode
                    .action
                    .as_deref()
                    .is_some_and(|action| action.contains(&procedure.id.to_string())),
                succeeded,
                rung,
            })?;
        }
        Ok(())
    }

    pub(crate) fn observed_fact_for_procedure(
        &self,
        procedure: &Procedure,
        value: Value,
        scope: BTreeMap<String, Value>,
    ) -> ObservedFact {
        observed_fact_for_procedure(procedure, value, scope)
    }

    fn reconcile_observed_fact_contradictions(&self) -> Result<(), EngineError> {
        for episode in self.episodes.list_recent(u32::MAX)? {
            self.detect_contradictions_for_episode(&episode)?;
        }
        Ok(())
    }

    fn detect_contradictions_for_episode(&self, episode: &Episode) -> Result<(), EngineError> {
        if episode.observed_facts.is_empty() || self.trust.receipt_for_episode(episode)?.is_none() {
            return Ok(());
        }
        self.detect_contradictions_for_trusted_episode(episode)
    }

    fn detect_contradictions_for_trusted_episode(
        &self,
        episode: &Episode,
    ) -> Result<(), EngineError> {
        for fact in &episode.observed_facts {
            if self.trust.verified_fact(episode, fact)?.is_none() {
                continue;
            }
            for prior in self.episodes.find_by_observed_predicate(&fact.predicate)? {
                if prior.id == episode.id || self.trust.receipt_for_episode(&prior)?.is_none() {
                    continue;
                }
                for prior_fact in prior
                    .observed_facts
                    .iter()
                    .filter(|candidate| candidate.predicate == fact.predicate)
                {
                    if self.trust.verified_fact(&prior, prior_fact)?.is_none() {
                        continue;
                    }
                    if prior_fact.value == fact.value {
                        continue;
                    }
                    let left = claim_from_observed_fact(&prior, prior_fact);
                    let right = claim_from_observed_fact(episode, fact);
                    let contradiction = self.contradictions.record(
                        left,
                        right,
                        &self.episodes,
                        episode.created_at,
                    )?;
                    let demonstrated = prior_fact
                        .scope
                        .iter()
                        .filter_map(|(feature, left_value)| {
                            fact.scope
                                .get(feature)
                                .filter(|right_value| *right_value != left_value)
                                .map(|right_value| {
                                    (feature.clone(), left_value.clone(), right_value.clone())
                                })
                        })
                        .collect::<Vec<_>>();
                    // A unique recorded discriminator is enough to split the
                    // two exact observations. Multiple correlated differences
                    // remain held until further evidence identifies which
                    // feature actually controls the claim.
                    if contradiction.status == spoon_adapt::ContradictionStatus::Held
                        && demonstrated.len() == 1
                    {
                        let (feature, left_value, right_value) = &demonstrated[0];
                        let discriminator = spoon_adapt::DemonstratedFeature::new(
                            feature,
                            left_value.clone(),
                            prior.id,
                            right_value.clone(),
                            episode.id,
                        )?;
                        self.contradictions.refine(
                            contradiction.id,
                            discriminator,
                            &self.episodes,
                            episode.created_at,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn observed_fact_for_procedure(
    procedure: &Procedure,
    value: Value,
    scope: BTreeMap<String, Value>,
) -> ObservedFact {
    match procedure.concept {
        Some(concept) => ObservedFact::for_concept(concept, value, scope),
        None => ObservedFact::for_procedure(procedure.id, value, scope),
    }
}

fn phase6_evidence_metrics(
    teacher: TeacherInteractionMetrics,
    skills: &[crate::ManagedSkill],
) -> Phase6EvidenceMetrics {
    let mut metrics = Phase6EvidenceMetrics {
        teacher_interaction_episodes: teacher.teacher_interaction_episodes,
        teacher_assisted_successes: teacher.teacher_assisted_successes,
        teacher_free_successes: teacher.teacher_free_successes,
        managed_skill_records_examined: skills.len() as u64,
        ..Phase6EvidenceMetrics::default()
    };
    for skill in skills {
        metrics.currently_promoted_skills +=
            u64::from(skill.lifecycle == crate::SkillLifecycle::Promoted);
        // These counters can only be incremented by `execute_managed_skill`,
        // which accepts promoted skills exclusively.
        metrics.post_promotion_skill_uses += u64::from(skill.experience_uses);
        metrics.post_promotion_skill_successes += u64::from(skill.experience_successes);
        match skill.promotion_verdict.as_ref() {
            Some(spoon_adapt::PromotionVerdict::NoMeasuredWin) => {
                metrics.replay_preserved_skill_verdicts += 1;
            }
            Some(spoon_adapt::PromotionVerdict::ShadowEligible { wins }) => {
                metrics.replay_preserved_skill_verdicts += 1;
                metrics.transfer_eligible_skill_verdicts +=
                    u64::from(wins.contains(&spoon_adapt::PromotionWin::Transfer));
            }
            Some(spoon_adapt::PromotionVerdict::Regression { .. }) => {
                metrics.replay_regressions += 1;
            }
            Some(spoon_adapt::PromotionVerdict::InsufficientEvidence) | None => {}
        }
    }
    metrics
}

fn claim_from_observed_fact(episode: &Episode, fact: &ObservedFact) -> spoon_adapt::Claim {
    spoon_adapt::Claim::new(
        format!("observed:{}", fact.id),
        format!("{} = {}", fact.predicate, fact.value),
        spoon_adapt::Implication::new(&fact.predicate, fact.value.clone()),
        vec![episode.id],
    )
}

fn validate_observed_facts(episode: &Episode) -> Result<(), EngineError> {
    let mut values = HashMap::<&str, &Value>::new();
    for fact in &episode.observed_facts {
        if fact.predicate.trim().is_empty()
            || fact.predicate.chars().count() > 512
            || fact.predicate.chars().any(char::is_control)
        {
            return Err(EngineError::InvalidInput(
                "observed-fact predicates must be non-empty, bounded, and control-free".into(),
            ));
        }
        if let Some(existing) = values.insert(&fact.predicate, &fact.value)
            && existing != &fact.value
        {
            return Err(EngineError::InvalidInput(format!(
                "episode {} contains conflicting values for observed predicate {:?}",
                episode.id, fact.predicate
            )));
        }
    }
    Ok(())
}

fn normalize_observed_facts(episode: &mut Episode, verifier: Option<&str>) {
    let tier = episode
        .evaluation
        .as_ref()
        .map(|evaluation| evaluation.tier);
    for (ordinal, fact) in episode.observed_facts.iter_mut().enumerate() {
        if fact.id.is_empty() {
            fact.id = format!("{}:{ordinal}", episode.id);
        }
        fact.source_episode.get_or_insert(episode.id);
        if fact.verifier.is_none() {
            fact.verifier = verifier.map(str::to_owned);
        }
        fact.tier = tier;
        fact.environment_digest = Some(scope_digest(&fact.scope));
    }
}

fn scope_digest(scope: &BTreeMap<String, Value>) -> String {
    let bytes = serde_json::to_vec(scope).expect("BTreeMap<Value> must serialize");
    let mut digest = Sha256::new();
    digest.update(b"spoon:observed-fact-environment:v1\0");
    digest.update(bytes);
    format!("sha256:{:x}", digest.finalize())
}

/// Return a stable audit digest without retaining caller input or policy
/// material in an episode. `serde_json::Map` uses key order in this workspace,
/// so equivalent typed request values have a reproducible digest.
fn json_digest(value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let mut digest = Sha256::new();
    digest.update(b"spoon:capability-invocation:v1\0");
    digest.update(bytes);
    format!("sha256:{:x}", digest.finalize())
}

fn text_digest(value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"spoon:capability-invocation-error:v1\0");
    digest.update(value.as_bytes());
    format!("sha256:{:x}", digest.finalize())
}

fn capability_procedure_summary(
    procedure: Option<&CapabilityProcedure>,
    procedure_id: &str,
) -> serde_json::Value {
    match procedure {
        Some(procedure) => serde_json::json!({
            "id": procedure.id,
            "name": procedure.name,
            "version": procedure.version,
            "primitive": procedure.primitive,
            "bounds": procedure.bounds,
        }),
        None => serde_json::json!({
            "id": procedure_id,
            "available": false,
        }),
    }
}

fn redacted_json_output(digest: &str, value: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "redacted": true,
        "digest": digest,
        "shape": json_shape(value),
    })
}

fn redacted_capability_output(digest: &str, value: &serde_json::Value) -> Value {
    Value::Map(BTreeMap::from([
        ("redacted".into(), Value::Bool(true)),
        ("digest".into(), Value::Text(digest.into())),
        ("shape".into(), Value::Text(json_shape(value))),
    ]))
}

fn json_shape(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(_) => "boolean".into(),
        serde_json::Value::Number(_) => "number".into(),
        serde_json::Value::String(_) => "string".into(),
        serde_json::Value::Array(items) => format!("array({})", items.len()),
        // Field names can themselves be sensitive (for example, a response
        // may expose an account identifier or secret-shaped key), so durable
        // redaction keeps only the object arity.
        serde_json::Value::Object(fields) => format!("object({})", fields.len()),
    }
}

fn authenticated_observation_verifier(episode: &Episode) -> Option<&str> {
    episode
        .action
        .as_deref()?
        .strip_prefix("observation:authenticated-verifier:")
        .filter(|identity| !identity.trim().is_empty())
}

pub(crate) fn is_current_executable(lifecycle: Lifecycle) -> bool {
    !matches!(
        lifecycle,
        Lifecycle::Invalid
            | Lifecycle::Retired
            | Lifecycle::Stale
            | Lifecycle::Superseded
            | Lifecycle::UnderReview
    )
}

pub(crate) fn collect_exact_calls(expression: &Expr, calls: &mut HashSet<(ProcedureId, u32)>) {
    match expression {
        Expr::Literal(_) | Expr::Var(_) => {}
        Expr::BinOp { left, right, .. } => {
            collect_exact_calls(left, calls);
            collect_exact_calls(right, calls);
        }
        Expr::UnOp { operand, .. } => collect_exact_calls(operand, calls),
        Expr::Call { args, .. } => {
            for argument in args {
                collect_exact_calls(argument, calls);
            }
        }
        Expr::CallExact {
            procedure,
            version,
            args,
        } => {
            calls.insert((*procedure, *version));
            for argument in args {
                collect_exact_calls(argument, calls);
            }
        }
        Expr::If { cond, then, else_ } => {
            collect_exact_calls(cond, calls);
            collect_exact_calls(then, calls);
            collect_exact_calls(else_, calls);
        }
        Expr::Let { value, body, .. } => {
            collect_exact_calls(value, calls);
            collect_exact_calls(body, calls);
        }
        Expr::Block(expressions) | Expr::ListExpr(expressions) => {
            for item in expressions {
                collect_exact_calls(item, calls);
            }
        }
        Expr::Index { collection, index } => {
            collect_exact_calls(collection, calls);
            collect_exact_calls(index, calls);
        }
        Expr::FieldAccess { object, .. } => collect_exact_calls(object, calls),
        Expr::Map {
            collection, body, ..
        } => {
            collect_exact_calls(collection, calls);
            collect_exact_calls(body, calls);
        }
        Expr::Filter {
            collection,
            predicate,
            ..
        } => {
            collect_exact_calls(collection, calls);
            collect_exact_calls(predicate, calls);
        }
        Expr::Reduce {
            collection,
            init,
            body,
            ..
        } => {
            collect_exact_calls(collection, calls);
            collect_exact_calls(init, calls);
            collect_exact_calls(body, calls);
        }
        Expr::Intrinsic { args, .. } => {
            for argument in args {
                collect_exact_calls(argument, calls);
            }
        }
    }
}

pub(crate) fn bind_inputs(
    procedure: &Procedure,
    supplied: &BTreeMap<String, Value>,
    defaults: Option<&[Value]>,
) -> Result<Vec<Value>, EngineError> {
    let known: HashSet<&str> = procedure
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    if let Some(extra) = supplied.keys().find(|name| !known.contains(name.as_str())) {
        return Err(EngineError::InvalidInput(format!(
            "unexpected input '{extra}'"
        )));
    }

    procedure
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            supplied
                .get(&param.name)
                .cloned()
                .or_else(|| defaults.and_then(|values| values.get(index)).cloned())
                .ok_or_else(|| EngineError::InvalidInput(format!("missing input '{}'", param.name)))
        })
        .collect()
}

/// Returns a stable, input/output-independent fingerprint of an observed
/// execution shape. Retirement uses this only to prove that new successor
/// evidence exercises every retired behavior path again; it is not a claim
/// that two different inputs produce the same answer.
fn behavior_digest(episode: &Episode) -> Result<String, EngineError> {
    let trace_json = episode
        .execution_trace
        .clone()
        .ok_or(EngineError::MissingTrace(episode.id))?;
    let trace: ExecTrace = serde_json::from_value(trace_json)?;
    if trace.steps.is_empty() {
        return Err(EngineError::MissingTopLevelProcedure);
    }
    let shape = trace
        .steps
        .iter()
        .map(|step| {
            (
                step.procedure_called.map(|id| id.to_string()),
                step.procedure_version,
                step.expr_description.clone(),
                matches!(step.status, ExecStepStatus::Succeeded),
                step.contract_checks.requires.len(),
                step.contract_checks.promises.len(),
                step.contract_checks.fails_when.len(),
            )
        })
        .collect::<Vec<_>>();
    let canonical = serde_json::to_vec(&(episode.action.as_deref(), shape))?;
    Ok(format!("{:x}", Sha256::digest(canonical)))
}

fn parse_procedure_action(action: Option<&str>) -> Option<(ProcedureId, u32)> {
    let action = action?.strip_prefix("procedure:")?;
    let (id, version) = action.split_once('@')?;
    Some((
        ProcedureId(uuid::Uuid::parse_str(id).ok()?),
        version.parse().ok()?,
    ))
}

pub(crate) fn reasoning_trace(trace: &ExecTrace) -> ReasoningTrace {
    ReasoningTrace {
        steps: trace
            .steps
            .iter()
            .map(|step| {
                let requires_violations: Vec<String> = step
                    .contract_checks
                    .requires
                    .iter()
                    .filter(|check| check.status == ConditionCheckStatus::Violated)
                    .map(|check| check.description.clone())
                    .collect();
                let promise_violations: Vec<String> = step
                    .contract_checks
                    .promises
                    .iter()
                    .filter(|check| check.status == ConditionCheckStatus::Violated)
                    .map(|check| check.description.clone())
                    .collect();
                let failure_conditions: Vec<String> = step
                    .contract_checks
                    .fails_when
                    .iter()
                    .filter(|check| check.status == ConditionCheckStatus::Violated)
                    .map(|check| check.description.clone())
                    .collect();
                let violations = requires_violations
                    .iter()
                    .chain(promise_violations.iter())
                    .chain(failure_conditions.iter())
                    .cloned()
                    .collect();
                let (status, output) = match &step.status {
                    ExecStepStatus::Succeeded => {
                        (TraceStepStatus::Succeeded, Some(step.output.clone()))
                    }
                    ExecStepStatus::Failed { error } => (
                        TraceStepStatus::Failed {
                            error: error.clone(),
                        },
                        None,
                    ),
                };
                TraceStep {
                    description: step.expr_description.clone(),
                    procedure_used: step.procedure_called,
                    contract_check: Some(ContractCheckResult {
                        all_requires_met: requires_violations.is_empty(),
                        all_promises_met: promise_violations.is_empty(),
                        no_failure_conditions_met: failure_conditions.is_empty(),
                        violations,
                    }),
                    input: step.input.clone(),
                    output,
                    rung: EscalationRung::Run,
                    status,
                }
            })
            .collect(),
    }
}
