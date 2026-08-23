use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::concept::{Concept, ConceptId, Lifecycle};
use crate::evidence::VerifiabilityTier;
use crate::procedure::ProcedureId;
use crate::relationship::Relationship;
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
    /// Predicate-bound observations established by this episode. A raw result
    /// is not evidence for an arbitrary semantic claim; downstream reasoning
    /// must match both the predicate and value of one of these facts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed_facts: Vec<ObservedFact>,
    pub evaluation: Option<Evaluation>,
    /// Lossless serialized execution trace used for deterministic replay.
    /// Kept as neutral JSON here so the core data model does not depend on a
    /// particular execution runtime crate.
    #[serde(default)]
    pub execution_trace: Option<serde_json::Value>,
    /// Provider-neutral record of a teacher request, response, and provenance.
    /// The JSON remains inspectable without coupling the core model to a
    /// particular provider SDK.
    #[serde(default)]
    pub teacher_interaction: Option<serde_json::Value>,
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
            observed_facts: Vec::new(),
            evaluation: None,
            execution_trace: None,
            teacher_interaction: None,
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
    #[serde(default)]
    pub goal_reason: Option<String>,
    #[serde(default)]
    pub interpretations: Vec<Interpretation>,
    pub entities: Vec<ConceptId>,
    #[serde(default)]
    pub relevant_knowledge: Vec<ContextRelationship>,
    #[serde(default)]
    pub relevant_procedures: Vec<ContextProcedure>,
    #[serde(default)]
    pub recent_episodes: Vec<ContextEpisode>,
    pub assumptions: Vec<Assumption>,
    #[serde(default)]
    pub environment: BTreeMap<String, Value>,
    #[serde(default)]
    pub budget_remaining: Option<ContextBudget>,
    /// Held contradiction identities inherited by this reasoning context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held_contradictions: Vec<i64>,
    /// Scoped contradiction refinements selected by the current environment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied_refinements: Vec<ContextRefinement>,
    /// Refined predicates for which the current environment matches neither
    /// (or ambiguously matches multiple) demonstrated scopes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_refinements: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextRefinement {
    pub contradiction_id: i64,
    pub claim_id: String,
    pub predicate: String,
    pub value: Value,
}

/// A canonical semantic observation plus the environment in which it held.
/// Scope is retained for later discriminator discovery; it is not silently
/// treated as proof that two disagreeing facts are incomparable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedFact {
    /// Stable identity within the immutable source episode. Engine-created
    /// facts use `<episode-id>:<ordinal>` so claims and receipts can refer to
    /// the observation itself rather than to an ambiguous predicate string.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub predicate: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scope: BTreeMap<String, Value>,
    /// Episode that established the fact. This is deliberately retained even
    /// though the fact is embedded in that episode, because imported or
    /// inspected fact references must be independently reconstructible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_episode: Option<EpisodeId>,
    /// Identity of an authenticated external verifier, when the fact did not
    /// originate from deterministic local execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier: Option<String>,
    /// Verification tier at the time this exact fact was established.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<VerifiabilityTier>,
    /// Canonical digest of the scoped environment. It is metadata for
    /// auditing/import validation, never a transferable environment secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_digest: Option<String>,
}

impl ObservedFact {
    pub fn new(predicate: impl Into<String>, value: Value, scope: BTreeMap<String, Value>) -> Self {
        Self {
            id: String::new(),
            predicate: predicate.into(),
            value,
            scope,
            source_episode: None,
            verifier: None,
            tier: None,
            environment_digest: None,
        }
    }

    pub fn for_concept(concept: ConceptId, value: Value, scope: BTreeMap<String, Value>) -> Self {
        Self::new(format!("concept:{concept}"), value, scope)
    }

    pub fn for_procedure(
        procedure: ProcedureId,
        value: Value,
        scope: BTreeMap<String, Value>,
    ) -> Self {
        Self::new(format!("procedure:{procedure}:result"), value, scope)
    }
}

/// A bounded graph edge retained in active and persisted context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRelationship {
    pub relationship: Relationship,
    pub discovered_from: ConceptId,
    pub adjacent_concept: Concept,
    pub hops: u32,
    pub relevance_score: f64,
}

/// Bounded metadata for a procedure relevant to the active concepts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextProcedure {
    pub id: ProcedureId,
    pub name: String,
    pub params: Vec<String>,
    pub concept: Option<ConceptId>,
    pub version: u32,
    pub lifecycle: Lifecycle,
    pub relevance_score: f64,
}

/// Historical action/result material retained in active context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextEpisode {
    pub episode_id: EpisodeId,
    pub situation: String,
    pub action: Option<String>,
    pub observed_result: Option<Value>,
    pub succeeded: Option<bool>,
    pub created_at: i64,
}

/// Remaining resources visible to the current reasoning cycle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ContextBudget {
    pub steps: u32,
    pub teacher_calls: u32,
    pub cost: f64,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::AssembledContext;

    #[test]
    fn legacy_assembled_context_defaults_new_phase_one_categories() {
        let context: AssembledContext =
            serde_json::from_str(r#"{"goal":"legacy","entities":[],"assumptions":[]}"#).unwrap();

        assert_eq!(context.goal.as_deref(), Some("legacy"));
        assert!(context.goal_reason.is_none());
        assert!(context.interpretations.is_empty());
        assert!(context.relevant_knowledge.is_empty());
        assert!(context.relevant_procedures.is_empty());
        assert!(context.recent_episodes.is_empty());
        assert!(context.environment.is_empty());
        assert!(context.budget_remaining.is_none());
        assert!(context.held_contradictions.is_empty());
        assert!(context.applied_refinements.is_empty());
        assert!(context.unresolved_refinements.is_empty());
    }
}
