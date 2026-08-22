use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::concept::ConceptId;
use crate::evidence::VerifiabilityTier;
use crate::procedure::ProcedureId;
use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EpisodeId(pub Uuid);

impl EpisodeId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for EpisodeId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EpisodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A structured record of a complete cognitive event. Not a log entry.
/// The raw material of every learning mechanism downstream. (section 18)
///
/// Several details exist for specific downstream reasons:
/// - Losing interpretations: needed to distinguish interpretation error
///   from reasoning error
/// - What was surfaced and rejected: distinguishes recall failure from
///   ranking failure
/// - Assumptions: prevents "fixing" something that was never broken
/// - Prediction: without it, nothing can be surprising
/// - Cost: needed to measure whether the system is getting cheaper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: EpisodeId,
    pub situation: String,
    pub interpretations: Vec<Interpretation>,
    pub context: AssembledContext,
    pub knowledge_considered: Vec<KnowledgeCandidate>,
    pub reasoning_trace: ReasoningTrace,
    pub prediction: Option<Value>,
    pub action: Option<String>,
    pub observed_result: Option<Value>,
    pub evaluation: Option<Evaluation>,
    /// Lossless serialized execution trace used for deterministic replay.
    /// Kept as neutral JSON here so the core data model does not depend on a
    /// particular execution runtime crate.
    #[serde(default)]
    pub execution_trace: Option<serde_json::Value>,
    pub cost: EpisodeCost,
    pub created_at: i64,
}

impl Episode {
    pub fn new(situation: impl Into<String>) -> Self {
        Self {
            id: EpisodeId::new(),
            situation: situation.into(),
            interpretations: Vec::new(),
            context: AssembledContext::default(),
            knowledge_considered: Vec::new(),
            reasoning_trace: ReasoningTrace::default(),
            prediction: None,
            action: None,
            observed_result: None,
            evaluation: None,
            execution_trace: None,
            cost: EpisodeCost::default(),
            created_at: now_unix(),
        }
    }

    pub fn succeeded(&self) -> bool {
        self.evaluation.as_ref().is_some_and(|e| e.success)
    }

    pub fn failed(&self) -> bool {
        self.evaluation.as_ref().is_some_and(|e| !e.success)
    }
}

/// Candidate meaning with weight. Weights sum to 1.
/// Ambiguity is preserved, not prematurely collapsed. (section 12)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interpretation {
    pub meaning: ConceptId,
    pub weight: f64,
    pub chosen: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssembledContext {
    pub goal: Option<String>,
    pub entities: Vec<ConceptId>,
    pub assumptions: Vec<Assumption>,
}

/// An assumption is marked so credit assignment can distinguish
/// "the procedure was wrong" from "the assumption was wrong."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assumption {
    pub description: String,
    /// "observed", "inferred", "assumed"
    pub basis: String,
    pub concept: Option<ConceptId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeCandidate {
    pub concept: ConceptId,
    pub relevance_score: f64,
    pub was_used: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReasoningTrace {
    pub steps: Vec<TraceStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStep {
    pub description: String,
    pub procedure_used: Option<ProcedureId>,
    pub contract_check: Option<ContractCheckResult>,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub rung: EscalationRung,
    #[serde(default)]
    pub status: TraceStepStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TraceStepStatus {
    #[default]
    Succeeded,
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractCheckResult {
    pub all_requires_met: bool,
    #[serde(default = "default_true")]
    pub all_promises_met: bool,
    #[serde(default = "default_true")]
    pub no_failure_conditions_met: bool,
    pub violations: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// The escalation ladder. Attempts ordered cheapest-first.
/// The rung reached is itself a measurement - a system whose problems
/// increasingly resolve at rungs 1-3 is getting smarter. (section 17)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EscalationRung {
    /// Do I already know the answer? Direct retrieval.
    #[default]
    Recall = 1,
    /// Do I have a skill for this? Execute a known procedure.
    Run = 2,
    /// Do I have a skill that almost fits? Adjust the nearest one.
    Adapt = 3,
    /// Can I build it from things I have? Contract-guided search.
    Compose = 4,
    /// Can I build it from primitives? Search the primitive space.
    Synthesize = 5,
    /// Can something else tell me? Escalate to a teacher.
    Ask = 6,
    /// Say so. A correct and underrated answer.
    Abstain = 7,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    pub tier: VerifiabilityTier,
    pub success: bool,
    pub details: String,
    pub surprise: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpisodeCost {
    pub rung_reached: EscalationRung,
    pub steps_taken: u32,
    pub budget_spent: f64,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
