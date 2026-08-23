//! SQLite-backed storage for the SPOON knowledge graph: concepts,
//! relationships, and procedures, along with the metadata that makes
//! them trustworthy (confidence, contracts, lifecycle).

mod activation;
mod error;
mod schema;
mod store;

pub use activation::{
    ActivatedConcept, ActivationHop, ActivationSeed, ActivationSpreadQuery, ActivationSpreadResult,
    MAX_ACTIVATION_CANDIDATES, MAX_ACTIVATION_EXPANSIONS, MAX_ACTIVATION_HOPS,
    MAX_ACTIVATION_SEEDS, MAX_ACTIVATION_TRAVERSALS, RelationshipDirection, TraversalDirection,
    TypedRelationshipTraversal,
};
pub use error::{GraphError, Result};
pub use store::{
    AppliedLifecycleChange, DependencyReport, DependencyTarget, Dependent, KnowledgeStore,
    LifecycleChange, LifecycleChangeReceipt, LifecycleChangeSet, LifecycleTarget,
    MAX_RELATIONSHIP_LIST_LIMIT, ProcedureDependencyKind, RelationshipDependency,
    RelationshipDependencyDirection,
};
