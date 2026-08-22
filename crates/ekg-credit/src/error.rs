use ekg_core::EpisodeId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CreditError {
    #[error("episode {0} has no execution trace")]
    MissingTrace(EpisodeId),
    #[error("episode {episode} contains an invalid execution trace: {source}")]
    InvalidTrace {
        episode: EpisodeId,
        #[source]
        source: serde_json::Error,
    },
    #[error("trace step {step} does not identify a procedure")]
    MissingProcedure { step: usize },
    #[error("trace step {step} does not pin a procedure version")]
    MissingProcedureVersion { step: usize },
    #[error("counterfactual replay failed: {0}")]
    Replay(String),
    #[error("counterfactual replay used {used} steps with only {allowed} authorized")]
    ReplayExceededStepBudget { used: u32, allowed: u32 },
    #[error("total episode cost must be finite and nonnegative, got {0}")]
    InvalidTotalCost(f64),
}
