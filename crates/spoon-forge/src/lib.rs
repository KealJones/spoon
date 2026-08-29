//! The seed forge: run a seed curriculum through the ordinary learning
//! machinery and produce inspectable evidence that the result reconstructs
//! somewhere else.
//!
//! A run walks the workflow the manifest describes. A clean in-memory engine
//! acquires the curriculum with the Teacher answering demonstrations,
//! counterexamples, and exercises; the Teacher is then removed and the
//! held-out probes and teacher-off gates run against the same store; the
//! acquired graph is projected into a neutral, privacy-filtered capability
//! bundle; and a second clean engine imports that bundle, rebuilds the
//! procedures from the neutral IR, replays the recorded cases, and re-runs the
//! clean-import gates. Everything the run observed lands in a [`ForgeReport`].
//!
//! Two boundaries are deliberate. The Teacher is a trait, so a run never
//! reaches a provider on its own. And nothing here signs anything: see
//! [`ReportSigner`] for the seam publication signing attaches to.

pub mod curriculum;

mod export;
mod inspect;
mod report;
mod runner;
mod teacher;

pub use curriculum::Curriculum;
pub use export::{
    ExportPolicy, NeutralCondition, NeutralParam, NeutralProcedure, ReplayCase, SeedBundle,
    build_seed_bundle,
};
pub use inspect::{StructureFinding, StructureStatus, inspect_structures};
pub use report::{
    ActivityReport, CleanImportReport, ExportReport, ForgeReport, GateReport, ImportStepReport,
    Observed, Phase, PhaseReport, ReportSigner, Signature,
};
pub use runner::ForgeRunner;
pub use teacher::CurriculumTeacher;

/// Every way a forge run can fail for a reason that is not itself evidence.
///
/// A probe that abstains when it should have answered is a recorded result,
/// not an error. These variants are reserved for a malformed manifest, a
/// broken store, or a policy refusal that stops the run.
#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("curriculum manifest: {0}")]
    Manifest(String),
    #[error("engine: {0}")]
    Engine(#[from] spoon_engine::EngineError),
    #[error("knowledge graph: {0}")]
    Graph(#[from] spoon_graph::GraphError),
    #[error("episodes: {0}")]
    Episodes(#[from] spoon_core::SpoonError),
    #[error("capability: {0}")]
    Capability(#[from] spoon_capability::CapabilityError),
    #[error("serialization: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The export privacy policy refused to ship something. Refusing is the
    /// correct outcome, so this error is a success signal for the policy and a
    /// failure signal for whatever tried to leave the machine.
    #[error("export refused: {subject} carries {violation}")]
    ExportRefused { subject: String, violation: String },
}
