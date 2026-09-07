//! What the evaluator records about its own reasoning.
//!
//! Without this, "learning from mistakes" is wishful thinking. A wrong answer
//! could come from the wrong realization, a wrong argument, or a correct
//! execution over a wrong interpretation, and nothing downstream can tell those
//! apart after the fact unless the evaluator wrote down what it did.

use std::sync::Arc;

use spoon_concept::{Concept, Effect};

/// One realization application.
#[derive(Debug, Clone)]
pub struct Step {
    /// The concept being reduced.
    pub concept: Concept,
    /// The realization that was applied, if one was.
    pub realization: Option<Arc<str>>,
    /// The others that were considered, with their scores. Credit assignment
    /// needs to know a better option existed and was passed over.
    pub alternatives: Vec<(Arc<str>, f64)>,
    /// True when exploration picked something other than the top candidate.
    pub explored: bool,
    pub effect: Effect,
    pub depth: u32,
    pub outcome: StepOutcome,
}

#[derive(Debug, Clone)]
pub enum StepOutcome {
    Reduced(Concept),
    /// No realization exists for this head. Not a failure: a concept like
    /// `FriendWith<Greg, Keal>` is data, and reducing it to itself is correct.
    /// Recorded because Stage 6 reads these to decide what to ask the Teacher.
    Irreducible,
    Failed(String),
}

/// A free-form note left by a native.
#[derive(Debug, Clone)]
pub struct Note {
    pub depth: u32,
    pub message: String,
}

/// The full record of one evaluation.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    pub steps: Vec<Step>,
    pub notes: Vec<Note>,
    pub nodes_used: u64,
    pub millis: u64,
}

impl Trace {
    /// Concepts that had no realization, deduplicated, in encounter order.
    /// This is the capability-gap report.
    pub fn irreducible(&self) -> Vec<&Concept> {
        let mut seen = std::collections::HashSet::new();
        self.steps
            .iter()
            .filter(|s| matches!(s.outcome, StepOutcome::Irreducible))
            .filter(|s| seen.insert(s.concept.content_id()))
            .map(|s| &s.concept)
            .collect()
    }

    /// Every realization that failed, with its message.
    pub fn failures(&self) -> Vec<(&Concept, Option<&Arc<str>>, &str)> {
        self.steps
            .iter()
            .filter_map(|s| match &s.outcome {
                StepOutcome::Failed(message) => {
                    Some((&s.concept, s.realization.as_ref(), message.as_str()))
                }
                _ => None,
            })
            .collect()
    }

    /// How often exploration overrode the top-ranked candidate.
    pub fn explorations(&self) -> usize {
        self.steps.iter().filter(|s| s.explored).count()
    }
}
