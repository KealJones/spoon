use std::collections::{BTreeMap, HashMap, HashSet};

use ekg_capability::{
    CapabilityBundle, CapabilityStore, ImportedCapability, LocalValidation, Permission,
};
use ekg_core::{
    Concept, ConceptId, ContractCheckResult, EkgError, Episode, EpisodeCost, EpisodeId,
    EscalationRung, Evaluation, Lifecycle, ObservedFact, Procedure, ProcedureId, ReasoningTrace,
    Relationship, RelationshipId, TestCase, TraceStep, TraceStepStatus, Value, VerifiabilityTier,
};
use ekg_episode::{EpisodeFeedback, EpisodeStore, VerifiedRegressionCase};
use ekg_exec::{ConditionCheckStatus, Evaluator, ExecStepStatus, ExecTrace};
use ekg_graph::{ActivationSpreadQuery, ActivationSpreadResult, GraphError, KnowledgeStore};
use ekg_intuition::{
    EpistemicChallengeKind, IntuitionMetrics, IntuitionStore, RankingExample, RecallCandidate,
    RecallDocument, RecallKind, SupervisionTask,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::evaluate_deterministic;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error(transparent)]
    Core(#[from] EkgError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Credit(#[from] ekg_credit::CreditError),
    #[error(transparent)]
    Adapt(#[from] ekg_adapt::AdaptError),
    #[error(transparent)]
    Intuition(#[from] ekg_intuition::IntuitionError),
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
        source: EkgError,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub episode_count: u64,
    pub rung_distribution: Vec<(String, u32)>,
    pub intuition: IntuitionMetrics,
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
    pub(crate) contradictions: ekg_adapt::ContradictionStore,
    pub(crate) lesson_stages: crate::lesson::LessonStageStore,
    pub(crate) runtime: crate::runtime::RuntimeStore,
    pub(crate) compression: crate::compression::CompressionStore,
    pub(crate) regression: crate::regression::RegressionStore,
    pub(crate) trust: crate::trust::TrustLedger,
    pub(crate) intuition: IntuitionStore,
    pub(crate) capabilities: CapabilityStore,
    pub(crate) goals: crate::goals::GoalStore,
    pub(crate) skills: crate::skills::SkillStore,
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
            contradictions: ekg_adapt::ContradictionStore::open(path)?,
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
            contradictions: ekg_adapt::ContradictionStore::in_memory()?,
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
    ) -> Result<ekg_intuition::RankingEvaluation, EngineError> {
        Ok(self
            .intuition
            .evaluate_ranking(query, candidate_limit, holdout_examples)?)
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
        description: &ekg_capability::InterfaceDescription,
    ) -> Result<CapabilityBundle, EngineError> {
        ekg_capability::discover_interface(description)
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

    pub fn require_capability_procedure(
        &self,
        content_id: &str,
        procedure_id: &str,
    ) -> Result<ekg_capability::CapabilityProcedure, EngineError> {
        self.capabilities
            .require_procedure_permissions(content_id, procedure_id)
            .map_err(|error| {
                EngineError::InvalidInput(format!("capability authorization: {error}"))
            })
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

    pub fn discover_skill_candidates(
        &self,
        limit: u32,
    ) -> Result<Vec<ekg_adapt::SkillCandidate>, EngineError> {
        let episodes = self.episodes.list_recent(limit.clamp(1, 512))?;
        let mut candidates = ekg_adapt::discover_skills(&episodes);
        for episode in &episodes {
            if let Some(candidate) = ekg_adapt::discover_single_success(episode) {
                candidates.push(candidate);
            }
            if let Some(candidate) = ekg_adapt::discover_failure_critic(episode) {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    pub fn plan_episode_compression(
        &self,
        limit: u32,
    ) -> Result<ekg_adapt::EpisodeCompressionPlan, EngineError> {
        let episodes = self.episodes.list_recent(limit.clamp(1, 512))?;
        Ok(ekg_adapt::plan_episode_compression(&episodes))
    }

    /// Materializes a bounded compression plan without mutating or deleting
    /// the source episodes. Failed episodes are rejected by the store.
    pub fn compress_episode_history(
        &self,
        limit: u32,
    ) -> Result<crate::EpisodeCompressionResult, EngineError> {
        let episodes = self.episodes.list_recent(limit.clamp(1, 512))?;
        let plan = ekg_adapt::plan_episode_compression(&episodes);
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
        candidate: &ekg_adapt::SkillCandidate,
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

    pub fn register_single_success_skill(
        &self,
        episode_id: EpisodeId,
    ) -> Result<crate::ManagedSkill, EngineError> {
        let episode = self.episodes.get(episode_id)?;
        let candidate = ekg_adapt::discover_single_success(&episode).ok_or_else(|| {
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
        let candidate = ekg_adapt::discover_failure_critic(&episode).ok_or_else(|| {
            EngineError::InvalidInput("episode is not an eligible failure critic".into())
        })?;
        self.register_skill_candidate(&candidate)
    }

    /// Records a replay verdict and enters shadow only after the conservative
    /// gate has preserved every replayed verified result and measured a win.
    pub fn evaluate_skill_for_shadow(
        &self,
        skill_id: &str,
        replays: impl IntoIterator<Item = ekg_adapt::PromotionReplay>,
    ) -> Result<crate::ManagedSkill, EngineError> {
        let replays: Vec<_> = replays.into_iter().collect();
        for replay in &replays {
            let episode = self.episodes.get(replay.episode_id)?;
            if !episode.succeeded() || self.trust.receipt_for_episode(&episode)?.is_none() {
                return Err(EngineError::InvalidInput(format!(
                    "promotion replay episode {} is not a successful trusted verification",
                    replay.episode_id
                )));
            }
        }
        let verdict = ekg_adapt::PromotionGate::evaluate(replays);
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
        self.execute_procedure(procedure_id, inputs, prediction)
    }

    /// Retirement changes ranking eligibility but retains the candidate,
    /// evidence, and explicit successor linkage for reconstruction.
    pub fn retire_managed_skill(
        &self,
        skill_id: &str,
        successor_skill: &str,
        reason: &str,
    ) -> Result<crate::ManagedSkill, EngineError> {
        let record = ekg_adapt::retire_skill(skill_id, successor_skill, reason);
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

    pub fn intuition_metrics(&self) -> Result<IntuitionMetrics, EngineError> {
        Ok(self.intuition.metrics()?)
    }

    pub fn train_representation_model(
        &self,
        holdout_tasks: usize,
    ) -> Result<ekg_intuition::RepresentationModel, EngineError> {
        Ok(self.intuition.train_representation_model(holdout_tasks)?)
    }

    pub fn activate_representation_model(
        &self,
        model_id: i64,
    ) -> Result<ekg_intuition::RepresentationModel, EngineError> {
        Ok(self.intuition.activate_representation_model(model_id)?)
    }

    pub fn latest_representation_model(
        &self,
    ) -> Result<Option<ekg_intuition::RepresentationModel>, EngineError> {
        Ok(self.intuition.latest_representation_model()?)
    }

    pub fn metrics_snapshot(&self) -> Result<MetricsSnapshot, EngineError> {
        Ok(MetricsSnapshot {
            episode_count: self.episodes.count()?,
            rung_distribution: self.episodes.rung_distribution()?,
            intuition: self.intuition.metrics()?,
        })
    }

    pub fn observe_native_primitive(
        &self,
        target: &str,
    ) -> Result<ekg_capability::PrimitiveExecution, EngineError> {
        if target != "clock" {
            return Err(EngineError::InvalidInput(
                "only the local clock observation is enabled".into(),
            ));
        }
        let policy = ekg_capability::PrimitivePolicy {
            observe_targets: std::collections::BTreeSet::from([target.to_owned()]),
            ..ekg_capability::PrimitivePolicy::default()
        };
        ekg_capability::NativePrimitiveExecutor::new(policy)
            .observe(&ekg_capability::PrimitiveRequest::Observe {
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
            .ok_or_else(|| EkgError::NotFound(format!("procedure {procedure_id}")))?;
        if !is_current_executable(procedure.lifecycle) {
            return Err(EngineError::InvalidInput(format!(
                "procedure {procedure_id} is not executable in lifecycle {:?}",
                procedure.lifecycle
            )));
        }
        let args = bind_inputs(&procedure, &inputs, None)?;
        let mut evaluator = self.current_evaluator()?;
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
            .ok_or_else(|| EkgError::NotFound(format!("procedure {procedure_id} v{version}")))?;
        let original = top.input.as_ref().and_then(Value::as_list);
        let args = bind_inputs(&procedure, &substitutions, original)?;

        let mut evaluator = Evaluator::new().with_budget(self.max_steps);
        let mut registered = HashSet::new();
        for step in &trace.steps {
            let (Some(id), Some(version)) = (step.procedure_called, step.procedure_version) else {
                continue;
            };
            if registered.insert(id) {
                let exact = self
                    .graph
                    .get_procedure_version(id, version)?
                    .ok_or_else(|| EkgError::NotFound(format!("procedure {id} v{version}")))?;
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

    #[allow(clippy::too_many_arguments)]
    fn record_execution(
        &self,
        procedure: &Procedure,
        inputs: &BTreeMap<String, Value>,
        prediction: Option<Value>,
        observed: Option<Value>,
        trace: &ExecTrace,
        failure: Option<&EkgError>,
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
            Err(EkgError::NotFound(_)) => self.episodes.insert(episode)?,
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
        if !episode.failed() {
            return Ok(());
        }
        let cost = f64::from(episode.cost.steps_taken.max(1));
        let blast_radius = if episode.action.is_some() { 2.0 } else { 1.0 };
        let gap = crate::goals::CuriosityGap {
            id: format!("episode:{}:failed-prediction", episode.id),
            kind: crate::goals::GapKind::FailedPrediction,
            statement: format!("failed prediction in {}", episode.situation),
            blast_radius,
            goal_relevance: 1.0,
            learning_progress: 1.0,
            cost_to_close: cost,
            value_score: blast_radius / cost,
            source_episode: Some(episode.id.to_string()),
            resolved: false,
            created_at: episode.created_at,
        };
        self.goals.record_gap(&gap)?;
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
                    if contradiction.status == ekg_adapt::ContradictionStatus::Held
                        && demonstrated.len() == 1
                    {
                        let (feature, left_value, right_value) = &demonstrated[0];
                        let discriminator = ekg_adapt::DemonstratedFeature::new(
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

fn claim_from_observed_fact(episode: &Episode, fact: &ObservedFact) -> ekg_adapt::Claim {
    ekg_adapt::Claim::new(
        format!("observed:{}", fact.id),
        format!("{} = {}", fact.predicate, fact.value),
        ekg_adapt::Implication::new(&fact.predicate, fact.value.clone()),
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
    digest.update(b"ekg:observed-fact-environment:v1\0");
    digest.update(bytes);
    format!("sha256:{:x}", digest.finalize())
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
