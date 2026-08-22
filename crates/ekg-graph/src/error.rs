use thiserror::Error;

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("core error: {0}")]
    Core(#[from] ekg_core::EkgError),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("has dependents: {0}")]
    HasDependents(String),

    #[error("revision conflict for {entity}: expected {expected}, current is {actual}")]
    RevisionConflict {
        entity: String,
        expected: u32,
        actual: u32,
    },

    #[error(
        "non-monotonic revision for {entity}: expected next version {expected_next}, proposed {proposed}"
    )]
    NonMonotonicRevision {
        entity: String,
        expected_next: u32,
        proposed: u32,
    },

    #[error("cannot change immutable field {field} on {entity}")]
    ImmutableFieldChange { entity: String, field: &'static str },

    #[error("{entity} requires an explicit expected version for further updates")]
    ExpectedVersionRequired { entity: String },

    #[error("invalid lifecycle change set: {0}")]
    InvalidChangeSet(String),

    #[error("invalid provisional knowledge bundle: {0}")]
    InvalidKnowledgeBundle(String),

    #[error("invalid activation spread query: {0}")]
    InvalidActivationQuery(String),

    #[error("idempotency key {key} was already used for a different graph change set")]
    IdempotencyConflict { key: String },

    #[error("invalid uuid: {0}")]
    InvalidUuid(#[from] uuid::Error),
}

pub type Result<T> = std::result::Result<T, GraphError>;
