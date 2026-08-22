use std::collections::HashSet;

use ekg_core::{ConceptId, Interpretation};
use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

/// Default tolerance used when checking that interpretation weights sum to one.
pub const DEFAULT_WEIGHT_TOLERANCE: f64 = 1e-9;
/// Largest tolerance accepted at any construction or deserialization boundary.
pub const MAX_WEIGHT_TOLERANCE: f64 = 1e-6;
/// Absolute ceiling preventing an interpretation payload from becoming context.
pub const MAX_INTERPRETATION_CANDIDATES: usize = 64;

/// One possible graph-backed meaning for an input.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct InterpretationCandidate {
    pub meaning: ConceptId,
    pub weight: f64,
}

/// A validated distribution over possible meanings.
///
/// `chosen` is deliberately optional: an unresolved ambiguity is data, not an
/// error. An explicit `UNKNOWN` concept is represented like any other meaning.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InterpretationSet {
    candidates: Vec<InterpretationCandidate>,
    chosen: Option<ConceptId>,
    tolerance: f64,
}

impl<'de> Deserialize<'de> for InterpretationSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SerializedInterpretationSet {
            candidates: Vec<InterpretationCandidate>,
            chosen: Option<ConceptId>,
            #[serde(default = "default_weight_tolerance")]
            tolerance: f64,
        }

        let serialized = SerializedInterpretationSet::deserialize(deserializer)?;
        Self::try_new_with_tolerance(
            serialized.candidates,
            serialized.chosen,
            serialized.tolerance,
        )
        .map_err(de::Error::custom)
    }
}

impl InterpretationSet {
    pub fn try_new(
        candidates: Vec<InterpretationCandidate>,
        chosen: Option<ConceptId>,
    ) -> Result<Self, InterpretationError> {
        Self::try_new_with_tolerance(candidates, chosen, DEFAULT_WEIGHT_TOLERANCE)
    }

    pub fn try_new_with_tolerance(
        candidates: Vec<InterpretationCandidate>,
        chosen: Option<ConceptId>,
        tolerance: f64,
    ) -> Result<Self, InterpretationError> {
        if !tolerance.is_finite() || tolerance < 0.0 {
            return Err(InterpretationError::InvalidTolerance(tolerance));
        }
        if tolerance > MAX_WEIGHT_TOLERANCE {
            return Err(InterpretationError::ToleranceExceedsMaximum {
                tolerance,
                maximum: MAX_WEIGHT_TOLERANCE,
            });
        }
        if candidates.is_empty() {
            return Err(InterpretationError::EmptyCandidates);
        }
        if candidates.len() > MAX_INTERPRETATION_CANDIDATES {
            return Err(InterpretationError::TooManyCandidates {
                count: candidates.len(),
                maximum: MAX_INTERPRETATION_CANDIDATES,
            });
        }

        let mut meanings = HashSet::with_capacity(candidates.len());
        let mut sum = 0.0;
        for candidate in &candidates {
            if !candidate.weight.is_finite() {
                return Err(InterpretationError::NonFiniteWeight {
                    meaning: candidate.meaning,
                    weight: candidate.weight,
                });
            }
            if candidate.weight < 0.0 {
                return Err(InterpretationError::NegativeWeight {
                    meaning: candidate.meaning,
                    weight: candidate.weight,
                });
            }
            if !meanings.insert(candidate.meaning) {
                return Err(InterpretationError::DuplicateMeaning(candidate.meaning));
            }
            sum += candidate.weight;
        }

        if (sum - 1.0).abs() > tolerance {
            return Err(InterpretationError::WeightsDoNotSumToOne { sum, tolerance });
        }
        if let Some(chosen) = chosen
            && !meanings.contains(&chosen)
        {
            return Err(InterpretationError::ChosenCandidateMissing(chosen));
        }

        Ok(Self {
            candidates,
            chosen,
            tolerance,
        })
    }

    pub fn candidates(&self) -> &[InterpretationCandidate] {
        &self.candidates
    }

    pub fn chosen(&self) -> Option<ConceptId> {
        self.chosen
    }

    pub fn tolerance(&self) -> f64 {
        self.tolerance
    }

    /// Convert to the core episode representation without dropping losers.
    pub fn to_episode_interpretations(&self) -> Vec<Interpretation> {
        self.candidates
            .iter()
            .map(|candidate| Interpretation {
                meaning: candidate.meaning,
                weight: candidate.weight,
                chosen: self.chosen == Some(candidate.meaning),
            })
            .collect()
    }
}

fn default_weight_tolerance() -> f64 {
    DEFAULT_WEIGHT_TOLERANCE
}

#[derive(Debug, Error)]
pub enum InterpretationError {
    #[error("interpretation candidates cannot be empty")]
    EmptyCandidates,
    #[error("interpretation has {count} candidates, above the hard maximum {maximum}")]
    TooManyCandidates { count: usize, maximum: usize },
    #[error("weight for concept {meaning} must be finite, got {weight}")]
    NonFiniteWeight { meaning: ConceptId, weight: f64 },
    #[error("weight for concept {meaning} cannot be negative, got {weight}")]
    NegativeWeight { meaning: ConceptId, weight: f64 },
    #[error("concept {0} appears more than once in interpretation candidates")]
    DuplicateMeaning(ConceptId),
    #[error("chosen concept {0} does not appear in interpretation candidates")]
    ChosenCandidateMissing(ConceptId),
    #[error("interpretation weights sum to {sum}, outside tolerance {tolerance}")]
    WeightsDoNotSumToOne { sum: f64, tolerance: f64 },
    #[error("weight tolerance must be finite and nonnegative, got {0}")]
    InvalidTolerance(f64),
    #[error("weight tolerance {tolerance} exceeds the hard maximum {maximum}")]
    ToleranceExceedsMaximum { tolerance: f64, maximum: f64 },
}
