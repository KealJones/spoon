use serde::{Deserialize, Serialize};
use spoon_core::{EpisodeId, ProcedureId};
use spoon_exec::ConditionCheckStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionMechanism {
    ContractViolation,
    StatisticalSuspicion,
    CounterfactualReplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionConfidence {
    Inconclusive,
    Low,
    Medium,
    High,
    Certain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractSection {
    Requires,
    Promises,
    FailsWhen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Suspect {
    pub procedure: ProcedureId,
    pub version: u32,
    pub trace_step: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterfactualMode {
    Deterministic,
    Simulated,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayProvenance {
    #[serde(default)]
    pub source_trace_hash: Option<String>,
    #[serde(default)]
    pub mutation_hash: Option<String>,
    #[serde(default)]
    pub verification: Option<ReplayVerificationProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ReplayVerificationProvenance {
    Deterministic {
        verifier: String,
    },
    Simulated {
        /// Only the engine can mint and resolve this content-addressed receipt.
        /// Generic replayers must continue to treat simulated provenance as
        /// untrusted even when a caller supplies a receipt-shaped string.
        #[serde(default)]
        receipt_id: Option<String>,
        model_id: String,
        model_version: String,
        assumptions: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AttributionLimitation {
    ContractViolationNotSoleCause,
    CorrelationNotCausation,
    CorrelatedCandidates { cooccurrence: f64 },
    NotReplayable { reason: String },
    SingleChangeCannotDetectInteractions { candidate_count: usize },
    UnverifiedReplayProvenance { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AttributionEvidence {
    Contract {
        section: ContractSection,
        description: String,
        status: ConditionCheckStatus,
    },
    Statistics {
        exposures: u32,
        failures: u32,
        support: u32,
        cooccurrence: f64,
        uncertainty: f64,
        #[serde(default)]
        weighted_exposure: f64,
        #[serde(default)]
        weighted_failures: f64,
    },
    Replay {
        mode: CounterfactualMode,
        change_description: String,
        counterfactual_succeeded: Option<bool>,
        steps_used: u32,
        details: String,
        #[serde(default)]
        provenance: ReplayProvenance,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionProvenance {
    pub episode_ids: Vec<EpisodeId>,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attribution {
    pub suspect: Suspect,
    pub mechanism: AttributionMechanism,
    pub confidence: AttributionConfidence,
    pub score: f64,
    pub decisive: bool,
    pub evidence: Vec<AttributionEvidence>,
    pub limitations: Vec<AttributionLimitation>,
    pub provenance: AttributionProvenance,
    pub attribution_cost: f64,
    pub total_cost: f64,
    pub attribution_cost_ratio: f64,
}

impl Attribution {
    pub fn contract_section(&self) -> Option<ContractSection> {
        self.evidence.iter().find_map(|evidence| match evidence {
            AttributionEvidence::Contract { section, .. } => Some(*section),
            _ => None,
        })
    }

    pub fn statistical_counts(&self) -> Option<(u32, u32)> {
        self.evidence.iter().find_map(|evidence| match evidence {
            AttributionEvidence::Statistics {
                exposures,
                failures,
                ..
            } => Some((*exposures, *failures)),
            _ => None,
        })
    }

    pub fn cooccurrence(&self) -> Option<f64> {
        self.evidence.iter().find_map(|evidence| match evidence {
            AttributionEvidence::Statistics { cooccurrence, .. } => Some(*cooccurrence),
            _ => None,
        })
    }

    pub fn uncertainty(&self) -> f64 {
        self.evidence
            .iter()
            .find_map(|evidence| match evidence {
                AttributionEvidence::Statistics { uncertainty, .. } => Some(*uncertainty),
                _ => None,
            })
            .unwrap_or(0.0)
    }
}

pub(crate) fn original_execution_cost(episode_steps: u32, episode_budget_spent: f64) -> f64 {
    if episode_budget_spent.is_finite() && episode_budget_spent > 0.0 {
        episode_budget_spent
    } else {
        f64::from(episode_steps).max(1.0)
    }
}

pub(crate) fn total_with_attribution(original_cost: f64, attribution_cost: f64) -> f64 {
    original_cost + attribution_cost
}

pub(crate) fn cost_ratio(attribution_cost: f64, total_cost: f64) -> f64 {
    if total_cost.is_finite() && total_cost > 0.0 {
        attribution_cost / total_cost
    } else {
        0.0
    }
}
