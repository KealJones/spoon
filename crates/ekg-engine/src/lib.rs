//! Orchestration and cognition-cycle services.

pub mod adaptation;
pub mod credit;
pub mod cycle;
pub mod engine;
pub mod evaluation;
mod lesson;
mod runtime;
mod trust;
mod view;

pub use credit::{
    CounterfactualMutation, FailureAnalysis, FailureAnalysisBudget, FailureAnalysisCost,
    FailureAnalysisRequest, FailureEvidence, FailureEvidenceSource,
    PHASE2_MAX_ATTRIBUTION_COST_RATIO, ProcedureVersionRef, ReplayVerification,
    SimulatedReplayModel, SimulatedReplayObservation, SimulatedReplayRequest,
    VersionPinnedReplayer,
};
pub use cycle::{
    CycleBudget, CycleDisposition, CycleId, CycleInput, CycleOutcome, CycleProgress,
    TeacherProposalWire, TeacherRequestWire,
};
pub use engine::{Engine, EngineError, ExecutionOutcome, ReplayOutcome};
pub use trust::{TrustEvidenceKind, TrustReceipt};
pub use view::{EpisodeView, GraphView};

pub use adaptation::{
    AdaptationAction, AdaptationEvidenceGate, AdaptationEvidenceRef, AdaptationKnowledgeRef,
    AdaptationOutcome, AdaptationPlan, AdaptationPlanId, AdaptationPlanRequest, AdaptationReceipt,
    AdaptationReconciliationEntry, AdaptationReconciliationOutcome, AdaptationReconciliationPlan,
    AdaptationReconciliationReceipt, AdaptationRecord, AdaptationTarget, ApplyAdaptationRequest,
    AttributionSelector, MutationScope, OfflineCapability,
};
pub use ekg_adapt::{
    Claim, Contradiction, ContradictionId, ContradictionStatus, DemonstratedFeature, Implication,
    Refinement, ScopeAssignment,
};
pub use evaluation::{
    CheckableSubgoal, ConsensusObservation, DecompositionError, GoalDecomposition,
    TierThreeJudgment, VerificationMethod, decompose_goal, detect_surprise, evaluate_consensus,
    evaluate_deterministic, evaluate_inverse, evaluate_round_trip, evaluate_tier_three,
};
