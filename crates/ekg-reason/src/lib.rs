//! Interpretation and bounded working-context assembly for EKG.

mod context;
mod interpretation;

pub use context::{
    ContextAssembler, ContextConfig, ContextError, ContextLimits, ContextRequest, KnowledgeContext,
    MAX_CONTEXT_COLLECTION_ITEMS, MAX_CONTEXT_GRAPH_HOPS, MAX_CONTEXT_TEXT_CHARS,
    MAX_CONTEXT_VALUE_DEPTH, RecentEpisode, RelevantProcedure, RelevantRelationship,
    RemainingBudget,
};
pub use interpretation::{
    DEFAULT_WEIGHT_TOLERANCE, InterpretationCandidate, InterpretationError, InterpretationSet,
    MAX_INTERPRETATION_CANDIDATES, MAX_WEIGHT_TOLERANCE,
};
