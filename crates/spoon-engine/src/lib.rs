//! Orchestration and cognition-cycle services.

#![recursion_limit = "512"]

pub mod adaptation;
pub mod admission;
pub mod compose;
mod compression;
pub mod credit;
pub mod cycle;
pub mod engine;
pub mod evaluation;
mod goals;
pub mod intent_catalog;
mod lesson;
pub mod parts;
mod regression;
mod runtime;
mod skills;
mod telemetry;
mod trust;
mod view;

pub use compression::{EpisodeCompressionRecord, EpisodeCompressionResult};
pub use credit::{
    CounterfactualMutation, FailureAnalysis, FailureAnalysisBudget, FailureAnalysisCost,
    FailureAnalysisRequest, FailureEvidence, FailureEvidenceSource,
    PHASE2_MAX_ATTRIBUTION_COST_RATIO, ProcedureVersionRef, ReplayVerification,
    SimulatedReplayModel, SimulatedReplayObservation, SimulatedReplayRequest,
    VersionPinnedReplayer,
};
pub use cycle::{
    CycleBudget, CycleDisposition, CycleId, CycleInput, CycleOutcome, CycleProgress,
    IntentProposalWire, IntentRequestWire, RecallMode, TeacherProposalWire, TeacherRequestWire,
    proposal_schema,
};
pub use engine::{
    CapabilityExecutionOutcome, Engine, EngineError, ExecutionOutcome, MetricsSnapshot,
    Phase6EvidenceMetrics, ReplayOutcome,
};
pub use goals::{
    CuriosityGap, GapKind, Goal, GoalDerivationRecord, GoalKind, GoalLearningRecord,
    LearningActionKind, ScheduledLearningAction,
};
pub use intent_catalog::{
    IntentCatalogEntry, IntentCatalogPattern, IntentCatalogStore, IntentSlotSchema,
    MAX_PATTERNS_PER_KEY, PatternAdmission, normalize_skeleton,
};
pub use regression::{
    MIN_BROAD_REGRESSION_CASES, RegressionSuiteCaseResult, RegressionSuiteCaseStatus,
    RegressionSuiteVerdict, VerifiedAnswerRecord,
};
pub use skills::{ManagedSkill, SkillLifecycle};
pub use spoon_capability::{
    AdapterExecution, AuthorizedPrimitiveInvocation, CapabilityBundle, CapabilityError,
    CapabilityInvocation, CapabilityInvocationAdapter, CapabilityProcedure, CapabilityStatus,
    CapabilityStore, CapabilityTest, Dependency, DiscoveredOperation, Effect, ImportedCapability,
    InterfaceDescription, InvocationReceipt, LocalValidation, NativePrimitive,
    NativePrimitiveExecutor, Permission, PrimitiveExecution, PrimitivePolicy, PrimitiveRequest,
    Provenance, ResourceBounds, ResourceUsage, bundle_content_id, discover_interface,
    export_bundle, import_bundle, run_sandbox_tests,
};
pub use spoon_graph::{
    ActivatedConcept, ActivationHop, ActivationSeed, ActivationSpreadQuery, ActivationSpreadResult,
    MAX_ACTIVATION_CANDIDATES, MAX_ACTIVATION_EXPANSIONS, MAX_ACTIVATION_HOPS,
    MAX_ACTIVATION_SEEDS, MAX_ACTIVATION_TRAVERSALS, RelationshipDirection, TraversalDirection,
    TypedRelationshipTraversal,
};
pub use spoon_intuition::{
    EpistemicChallengeKind, IntuitionMetrics, MAX_AUTO_GROUNDED_SUPERVISION_TASKS,
    RankingEvaluation, RankingExample, RecallCandidate, RecallDocument, RecallKind,
    RepresentationModel, SemanticRecallEvaluation, SupervisionTask,
};
pub use telemetry::{
    FalsificationMeasurement, FalsificationMeasurementInput, FalsificationRun,
    FalsificationRunInput, GroundingTier, MetricEvidenceStatus, ProbeCohort, Section38Metric,
    Section38TelemetrySnapshot, TeacherMode,
};
pub use trust::{TrustEvidenceKind, TrustReceipt};
pub use view::{EpisodeView, GraphView};

pub use adaptation::{
    AdaptationAction, AdaptationEvidenceGate, AdaptationEvidenceRef, AdaptationKnowledgeRef,
    AdaptationOutcome, AdaptationPlan, AdaptationPlanId, AdaptationPlanRequest, AdaptationReceipt,
    AdaptationReconciliationEntry, AdaptationReconciliationOutcome, AdaptationReconciliationPlan,
    AdaptationReconciliationReceipt, AdaptationRecord, AdaptationTarget, ApplyAdaptationRequest,
    AttributionSelector, MutationScope, OfflineCapability,
};
pub use evaluation::{
    CheckableSubgoal, ConsensusObservation, DecompositionError, GoalDecomposition,
    TierThreeJudgment, VerificationMethod, decompose_goal, detect_surprise, evaluate_consensus,
    evaluate_deterministic, evaluate_inverse, evaluate_round_trip, evaluate_tier_three,
};
pub use spoon_adapt::{
    BehavioralSubsumptionEvidence, Claim, Contradiction, ContradictionId, ContradictionStatus,
    DemonstratedFeature, EpisodeCompressionPlan, Implication, PromotionGate, PromotionReplay,
    PromotionVerdict, PromotionWin, Refinement, RetirementRecord, ScopeAssignment, SkillCandidate,
    discover_failure_critic, discover_single_success, discover_skills, plan_episode_compression,
    retire_skill,
};
