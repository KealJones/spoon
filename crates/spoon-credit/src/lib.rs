//! Evidence-bounded credit assignment for failed Spoon episodes.

mod contract;
mod error;
mod replay;
mod statistical;
mod types;

pub use contract::{ContractAttributionReport, attribute_contract_violations};
pub use error::CreditError;
pub use replay::{
    BudgetStopReason, CounterfactualCandidate, CounterfactualChange, CounterfactualReplayer,
    CounterfactualReport, ReplayBudget, ReplayObservation, ReplayOutcome, ReplayRequest,
    run_counterfactual_replays,
};
pub use statistical::{
    StatisticalCost, StatisticalRankingReport, rank_statistical_suspects,
    rank_statistical_suspects_from_aggregates, rank_statistical_suspects_with_cost,
};
pub use types::{
    Attribution, AttributionConfidence, AttributionEvidence, AttributionLimitation,
    AttributionMechanism, AttributionProvenance, ContractSection, CounterfactualMode,
    ReplayProvenance, ReplayVerificationProvenance, Suspect,
};
