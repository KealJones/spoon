//! Episodes: the never-lossy autobiographical log, plus per-turn metrics.

use serde::{Deserialize, Serialize};

use super::clause::{Clause, EarsPath};
use super::intent::Plan;
use super::response::ResponsePlan;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TurnMetrics {
    pub ears_path: Option<EarsPath>,
    /// Must always be 0. Asserted in tests and reported in /debug/metrics.
    pub interior_llm_calls: u32,
    pub ears_llm_calls: u32,
    pub mouth_llm_calls: u32,
    pub teacher_llm_calls: u32,
    pub plan_steps: usize,
    pub synthesis_attempted: bool,
    pub synthesis_succeeded: bool,
    pub teacher_fallback: bool,
    pub reused_learned_action: bool,
    pub ms_ears: u64,
    pub ms_interior: u64,
    pub ms_mouth: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub id: i64,
    pub session_id: String,
    pub at: i64,
    pub user_text: String,
    pub sce: String,
    pub clauses: Vec<Clause>,
    #[serde(default)]
    pub plans: Vec<Plan>,
    pub response: ResponsePlan,
    pub reply_text: String,
    pub metrics: TurnMetrics,
    /// Set by a later correction: -1 wrong, +1 confirmed, 0 unknown.
    #[serde(default)]
    pub credit: i8,
    /// Topic keywords for retrieval (nouns + names + verbs).
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// A (messy utterance, SCE) pair harvested from the LLM normalizer or a user
/// correction. Training data for the native recognizer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    pub id: i64,
    pub utterance: String,
    pub sce: String,
    /// "llm", "user", "teacher", "seed"
    pub source: String,
    pub at: i64,
    /// -1 if a later correction showed this mapping was wrong.
    #[serde(default)]
    pub credit: i8,
}
