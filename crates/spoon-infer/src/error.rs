//! Failure modes of derivation.

use spoon_concept::Concept;

#[derive(Debug, thiserror::Error)]
pub enum InferError {
    #[error("derivation budget exhausted: {limit}")]
    Exhausted { limit: &'static str },

    /// A goal that is nothing but a hole would match every fact in the brain.
    /// Answering it means reading everything, which is a hang rather than an
    /// answer.
    #[error("goal {} is unanchored and would match everything", .goal.content_id().short())]
    Unanchored { goal: Concept },

    #[error(transparent)]
    Store(#[from] spoon_store::StoreError),
}

pub type Result<T> = std::result::Result<T, InferError>;
