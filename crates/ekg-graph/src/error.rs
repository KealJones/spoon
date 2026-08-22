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

    #[error("invalid uuid: {0}")]
    InvalidUuid(#[from] uuid::Error),
}

pub type Result<T> = std::result::Result<T, GraphError>;
