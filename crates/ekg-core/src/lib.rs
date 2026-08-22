pub mod value;
pub mod expr;
pub mod concept;
pub mod relationship;
pub mod contract;
pub mod procedure;
pub mod episode;
pub mod evidence;
pub mod error;

pub use value::Value;
pub use expr::{Expr, BinOp, UnOp};
pub use concept::{Concept, ConceptId, MutabilityClass, Lifecycle};
pub use relationship::{Relationship, RelationshipId};
pub use contract::{Contract, Condition, CostEstimate};
pub use procedure::{Procedure, ProcedureId, Param, TestCase};
pub use episode::{
    Episode, EpisodeId, Interpretation, AssembledContext, Assumption,
    KnowledgeCandidate, ReasoningTrace, TraceStep, ContractCheckResult,
    EscalationRung, Evaluation, EpisodeCost,
};
pub use evidence::{VerifiabilityTier, Confidence, ScopeCondition, Source, SourceKind};
pub use error::EkgError;
