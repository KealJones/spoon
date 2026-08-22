//! Orchestration and cognition-cycle services.

pub mod cycle;
pub mod engine;
pub mod evaluation;

pub use cycle::{
    CycleBudget, CycleDisposition, CycleId, CycleInput, CycleOutcome, CycleProgress,
    TeacherProposalWire, TeacherRequestWire,
};
pub use engine::{Engine, EngineError, ExecutionOutcome, ReplayOutcome};

pub use evaluation::{
    CheckableSubgoal, ConsensusObservation, DecompositionError, GoalDecomposition,
    TierThreeJudgment, VerificationMethod, decompose_goal, detect_surprise, evaluate_consensus,
    evaluate_deterministic, evaluate_inverse, evaluate_round_trip, evaluate_tier_three,
};
