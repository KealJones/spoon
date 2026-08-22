use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdaptError {
    #[error("core error: {0}")]
    Core(#[from] ekg_core::EkgError),

    #[error("graph error: {0}")]
    Graph(#[from] ekg_graph::GraphError),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("invalid adaptation: {0}")]
    Invalid(String),

    #[error("adaptation record not found: {0}")]
    NotFound(String),

    #[error("mutation authorization rejected: {0}")]
    Unauthorized(String),

    #[error("trusted offline capability required: {0}")]
    OfflineCapabilityRequired(String),
}

pub type Result<T> = std::result::Result<T, AdaptError>;
