//! Failure modes of the store.
//!
//! Every variant here is a distinct thing a caller can act on. A sqlite
//! failure means the file or the schema is wrong; a decode failure means a row
//! was written by an incompatible build; a missing record means the caller
//! referenced something that was never stored. Collapsing those into one
//! opaque error would make every one of them look like the same bug.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// The database was written by a newer build than this one. Opening it
    /// anyway would silently misread rows, so refuse instead.
    #[error("schema version {found} is newer than this build understands ({supported})")]
    SchemaTooNew { found: u32, supported: u32 },

    /// A stored row is structurally wrong: a content id that is not 32 bytes,
    /// a shape tag nobody writes, a timestamp outside the representable range.
    #[error("corrupt row in {table}: {detail}")]
    Corrupt { table: &'static str, detail: String },

    /// The caller referenced something that is not there. Distinct from an
    /// empty query result: this is a lookup by primary key that should have
    /// hit.
    #[error("no {kind} named {key}")]
    MissingRecord { kind: &'static str, key: String },

    /// Holes are the one shape that is not a concept. A bare hole has no
    /// identity worth storing, no participants, and nothing can be truthfully
    /// asserted about it, so storing one is a category error rather than a
    /// no-op we should swallow.
    #[error("a bare hole is not a storable concept")]
    HoleNotStorable,

    /// JSON has no encoding for NaN or infinity. Writing one would produce a
    /// row that reads back as an error forever, so reject it at write time
    /// where the caller can still do something about it.
    #[error("cannot store a non-finite float ({value})")]
    NonFiniteFloat { value: f64 },
}

pub type Result<T> = std::result::Result<T, StoreError>;

impl StoreError {
    pub(crate) fn corrupt(table: &'static str, detail: impl Into<String>) -> Self {
        StoreError::Corrupt {
            table,
            detail: detail.into(),
        }
    }

    pub(crate) fn missing(kind: &'static str, key: impl Into<String>) -> Self {
        StoreError::MissingRecord {
            kind,
            key: key.into(),
        }
    }
}
