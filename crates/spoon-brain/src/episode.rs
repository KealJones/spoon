//! What one turn leaves behind.
//!
//! An episode is not a transcript. It is the structured evidence needed to
//! answer "why did that happen", which is what makes learning from a mistake
//! something other than wishful thinking. Without the path that produced an
//! answer, a correction can only say the answer was wrong, not which step was.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use spoon_concept::Concept;

/// Counters for one turn, and the guarantee the design rests on.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TurnMetrics {
    pub ears_native: u64,
    pub ears_model: u64,
    pub ears_failed: u64,
    pub mouth_model: u64,
    pub mouth_template: u64,
    pub teacher_calls: u64,
    /// Must stay zero. Asserted in tests, because the moment a model makes a
    /// choice in the middle of the system the whole premise is gone.
    pub interior_model_calls: u64,
    pub eval_nodes: u64,
    pub derive_steps: u64,
    pub millis_total: u64,
    pub millis_ears: u64,
    pub millis_interior: u64,
    pub millis_mouth: u64,
}

/// Which path the ears took, which is the weaning curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EarsPath {
    Native,
    Model,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouthPath {
    Template,
    Model,
}

/// One decision the interior made.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceStep {
    /// What was being reduced, rendered with the names in force at the time.
    pub concept: String,
    /// What was applied, if anything.
    pub realization: Option<String>,
    /// What it was chosen over, with the score each got. This is the part that
    /// cannot be reconstructed later.
    pub alternatives: Vec<(String, f64)>,
    /// True when exploration overrode the top-ranked candidate, so a failure
    /// can be read as the cost of looking rather than a bad realization.
    pub explored: bool,
    pub effect: String,
    pub depth: u32,
    pub outcome: String,
}

/// One turn, start to finish.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: u64,
    pub at: DateTime<Utc>,
    pub session: String,
    pub user_text: String,
    /// What the ears made of it.
    pub steps: Vec<Concept>,
    pub ears_path: EarsPath,
    pub unknown_words: Vec<String>,
    /// The goal the interior actually tried to satisfy.
    pub goal: Option<Concept>,
    /// What came back.
    pub result: Option<Concept>,
    /// Concepts reached that nothing could realize. The capability-gap report,
    /// and what the Teacher is pointed at.
    pub gaps: Vec<Concept>,
    /// Realizations applied, in order, with whether each worked.
    pub realizations: Vec<(String, bool)>,
    /// The interior's own reasoning, step by step.
    ///
    /// Knowing a realization ran is not enough to diagnose anything. The
    /// question after a wrong answer is which alternatives existed and why this
    /// one beat them, and that is unrecoverable afterwards: scores depend on
    /// activation at the time, and the store has moved on since. Written down
    /// or lost.
    #[serde(default)]
    pub trace: Vec<TraceStep>,
    /// Rules that fired, when the answer came from derivation rather than
    /// evaluation.
    pub rules: Vec<String>,
    /// What Spoon learned this turn, and from where.
    ///
    /// Worth surfacing rather than leaving in the store, because "3" and "3,
    /// and the Teacher had to correct how I read that" are different answers to
    /// the reader even when the number is the same. It is also the only way to
    /// see the Teacher earning its cost.
    #[serde(default)]
    pub learning: Vec<String>,
    pub reply: String,
    pub mouth_path: MouthPath,
    pub metrics: TurnMetrics,
    /// Set on a later turn when the user says this one was wrong. Left open on
    /// purpose: silence is weak evidence, not approval, because a user may
    /// simply move on after a bad answer.
    pub correction: Option<String>,
}

impl Episode {
    /// Did anything about this turn suggest it went badly?
    ///
    /// Deliberately conservative. Answering "I do not know" is correct
    /// behaviour rather than a defect, so an honest gap does not count against
    /// the turn.
    pub fn looks_unsatisfying(&self) -> bool {
        self.correction.is_some()
            || self.ears_path == EarsPath::Failed
            || self.realizations.iter().any(|(_, ok)| !ok)
    }
}
