//! Orchestration and cognition-cycle services.

pub mod engine;
pub mod evaluation;

pub use engine::{Engine, EngineError, ExecutionOutcome, ReplayOutcome};

pub use evaluation::{
    CheckableSubgoal, ConsensusObservation, DecompositionError, GoalDecomposition,
    TierThreeJudgment, VerificationMethod, decompose_goal, detect_surprise, evaluate_consensus,
    evaluate_deterministic, evaluate_inverse, evaluate_round_trip, evaluate_tier_three,
};
