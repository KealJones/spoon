//! What evaluation can produce, including the ways it can decline to finish.

use spoon_concept::{Concept, Effect, NativeId};
use std::sync::Arc;

/// A concept the evaluator reached but could not operationalize.
///
/// A gap is not a failure. `Height<Greg>` with no way to resolve a height is a
/// perfectly meaningful concept that Spoon simply cannot reduce today. Gaps are
/// the signal that drives Teacher-assisted self-extension: they name exactly
/// what is missing, so something else can go find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// The concept whose head had no usable realization.
    pub concept: Concept,
    /// Why it could not be reduced.
    pub reason: GapReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GapReason {
    /// Nothing in the store realizes this head.
    NoRealization,
    /// Realizations exist but every one was excluded: deprecated, or barred by
    /// the effect authority in force.
    AllExcluded,
    /// Every applicable realization was tried and each one failed.
    AllFailed,
}

impl Gap {
    pub fn no_realization(concept: Concept) -> Self {
        Gap {
            concept,
            reason: GapReason::NoRealization,
        }
    }
}

/// Which budget ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Nodes,
    Time,
    Depth,
    ResultSize,
}

impl Limit {
    pub fn as_str(self) -> &'static str {
        match self {
            Limit::Nodes => "nodes",
            Limit::Time => "time",
            Limit::Depth => "depth",
            Limit::ResultSize => "result size",
        }
    }
}

/// Why an evaluation step could not complete.
///
/// `Stuck` and `Exhausted` are carried here rather than as a separate success
/// channel so that a native can use `?` and have the reason propagate intact.
/// The top-level [`crate::Outcome`] sorts them back out.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EvalError {
    /// Meaningful, not computable. Carries how far it did reduce.
    #[error("no way to evaluate {}", .concept.content_id().short())]
    Stuck { concept: Concept, gaps: Vec<Gap> },

    /// A budget ran out. The partial result is preserved.
    #[error("budget exhausted: {}", .limit.as_str())]
    Exhausted { concept: Concept, limit: Limit },

    /// The active permission mode forbids this effect outright.
    #[error("{} effect refused under the active permission mode", .effect.as_str())]
    Refused { concept: Concept, effect: Effect },

    /// The effect needs confirmation before it can run. Evaluation suspends
    /// rather than assuming the answer would have been yes.
    #[error("{} effect needs confirmation", .effect.as_str())]
    NeedsPermission {
        concept: Concept,
        effect: Effect,
        realization: Arc<str>,
    },

    /// A stored realization names a native that is not registered.
    ///
    /// This is deliberately fatal rather than a silent skip. A brain
    /// referencing a native Spoon no longer ships is broken, and saying so
    /// beats quietly losing a capability and looking merely dumber.
    #[error("realization {realization} references unregistered native {}", .native.as_str())]
    MissingNative {
        realization: Arc<str>,
        native: NativeId,
    },

    /// A native was handed arguments it cannot work with.
    #[error("{native}: expected {expected}, got {}", .got.content_id().short())]
    Type {
        native: Arc<str>,
        expected: String,
        got: Concept,
    },

    /// A native failed for its own reasons.
    #[error("{native}: {message}")]
    Native { native: Arc<str>, message: String },

    /// A rule or composition re-entered a goal already being derived.
    #[error("cycle re-entering {}", .concept.content_id().short())]
    Cycle { concept: Concept },

    /// Wrapped in an `Arc` because `StoreError` is not `Clone` (it carries a
    /// `rusqlite::Error`), and an outcome has to be cloneable so a caller can
    /// hold onto one while continuing to evaluate.
    #[error(transparent)]
    Store(Arc<spoon_store::StoreError>),
}

impl From<spoon_store::StoreError> for EvalError {
    fn from(err: spoon_store::StoreError) -> Self {
        EvalError::Store(Arc::new(err))
    }
}

impl EvalError {
    /// The concept this error is about, when it names one. Credit assignment
    /// needs something to point at.
    pub fn concept(&self) -> Option<&Concept> {
        match self {
            EvalError::Stuck { concept, .. }
            | EvalError::Exhausted { concept, .. }
            | EvalError::Refused { concept, .. }
            | EvalError::NeedsPermission { concept, .. }
            | EvalError::Cycle { concept } => Some(concept),
            EvalError::Type { got, .. } => Some(got),
            _ => None,
        }
    }

    /// Whether retrying with a different realization could plausibly help.
    ///
    /// A budget overrun or a refused effect will not change if you try the next
    /// realization, so the evaluator stops instead of burning the rest of the
    /// candidate list on the same wall.
    pub fn is_retryable(&self) -> bool {
        match self {
            EvalError::Exhausted { .. }
            | EvalError::Refused { .. }
            | EvalError::NeedsPermission { .. }
            | EvalError::Cycle { .. } => false,
            EvalError::Stuck { .. }
            | EvalError::MissingNative { .. }
            | EvalError::Type { .. }
            | EvalError::Native { .. }
            | EvalError::Store(_) => true,
        }
    }
}

/// The result of a top-level evaluation.
///
/// Evaluation never returns "nothing". Every path lands in exactly one of these,
/// and three of the five are not errors.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Reduced to a value.
    Value(Concept),
    /// Meaningful but not computable, with the gaps named.
    Stuck { concept: Concept, gaps: Vec<Gap> },
    /// A limit was hit. The partial result is preserved.
    Exhausted { concept: Concept, limit: Limit },
    /// Suspended awaiting confirmation of a side effect.
    NeedsPermission {
        concept: Concept,
        effect: Effect,
        realization: Arc<str>,
    },
    /// Execution was refused or a realization failed outright.
    Failed(EvalError),
}

impl Outcome {
    /// The reduced concept when evaluation produced one.
    pub fn value(&self) -> Option<&Concept> {
        match self {
            Outcome::Value(c) => Some(c),
            _ => None,
        }
    }

    /// The concept as far as it reduced, for every outcome that has one.
    /// Useful for reporting: a partial answer beats no answer.
    pub fn partial(&self) -> Option<&Concept> {
        match self {
            Outcome::Value(c)
            | Outcome::Stuck { concept: c, .. }
            | Outcome::Exhausted { concept: c, .. }
            | Outcome::NeedsPermission { concept: c, .. } => Some(c),
            Outcome::Failed(e) => e.concept(),
        }
    }

    pub fn is_value(&self) -> bool {
        matches!(self, Outcome::Value(_))
    }

    /// Gaps encountered, empty unless the outcome is `Stuck`. This is what
    /// Stage 6 reads to decide what to ask the Teacher for.
    pub fn gaps(&self) -> &[Gap] {
        match self {
            Outcome::Stuck { gaps, .. } => gaps,
            _ => &[],
        }
    }
}

impl From<Result<Concept, EvalError>> for Outcome {
    fn from(result: Result<Concept, EvalError>) -> Self {
        match result {
            Ok(c) => Outcome::Value(c),
            Err(EvalError::Stuck { concept, gaps }) => Outcome::Stuck { concept, gaps },
            Err(EvalError::Exhausted { concept, limit }) => Outcome::Exhausted { concept, limit },
            Err(EvalError::NeedsPermission {
                concept,
                effect,
                realization,
            }) => Outcome::NeedsPermission {
                concept,
                effect,
                realization,
            },
            Err(other) => Outcome::Failed(other),
        }
    }
}

pub type EvalResult = Result<Concept, EvalError>;
