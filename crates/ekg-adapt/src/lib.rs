//! Evidence-gated adaptation, reconciliation, and contradiction refinement.

mod consolidation;
mod contradiction;
mod error;
mod policy;
mod promotion;
mod reconciliation;

pub use consolidation::{
    EpisodeCompressionPlan, RetirementRecord, SkillCandidate, discover_failure_critic,
    discover_single_success, discover_skills, plan_episode_compression, retire_skill,
};
pub use contradiction::{
    AppliedPredicateRefinement, Claim, Contradiction, ContradictionId, ContradictionStatus,
    ContradictionStore, DemonstratedFeature, Implication, PredicateRefinementContext, Refinement,
    ScopeAssignment, Uncertainty,
};
pub use error::{AdaptError, Result};
pub use policy::{
    AdaptationPolicy, ApplyOutcome, AttributionStrength, AuthorizedCorrection, CorrectionAction,
    CorrectionApplier, CorrectionDecision, CorrectionRequest, CorrectionTarget, EvidenceGate,
    MutationAuthorizer,
};
pub use promotion::{PromotionGate, PromotionReplay, PromotionVerdict, PromotionWin};
pub use reconciliation::{
    AlternativeSupport, AlternativeSupportVerdict, GraphAlternativeSupport, KnowledgeRef,
    ReconciliationApplier, ReconciliationApplyResult, ReconciliationEntry, ReconciliationOutcome,
    ReconciliationPlan, ReconciliationPlanner, StagedReconciliation,
};
