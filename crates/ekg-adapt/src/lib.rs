//! Evidence-gated adaptation, reconciliation, and contradiction refinement.

mod contradiction;
mod error;
mod policy;
mod reconciliation;

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
pub use reconciliation::{
    AlternativeSupport, AlternativeSupportVerdict, GraphAlternativeSupport, KnowledgeRef,
    ReconciliationApplier, ReconciliationApplyResult, ReconciliationEntry, ReconciliationOutcome,
    ReconciliationPlan, ReconciliationPlanner, StagedReconciliation,
};
