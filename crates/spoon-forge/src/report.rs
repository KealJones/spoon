use serde::{Deserialize, Serialize};

use crate::ForgeError;
use crate::curriculum::{ExpectedDisposition, FailurePolicy, GateStage, GateStore, ImportStep};
use crate::inspect::StructureFinding;

/// The acquisition and ablation phases, in the order a run drives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Demonstrations,
    Counterexamples,
    Exercises,
    HeldOutGeneralization,
}

impl Phase {
    /// Whether the Teacher is reachable during this phase. The held-out phase
    /// is the ablation, so it is the one phase the Teacher cannot enter.
    pub fn teacher_allowed(self) -> bool {
        !matches!(self, Self::HeldOutGeneralization)
    }
}

/// What the engine actually did with a probe, collapsed to the distinction the
/// curriculum cares about: did it commit to an answer or decline to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Observed {
    Answered,
    Abstained,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityReport {
    pub id: String,
    /// The exact situation text handed to the engine, so a reader can tell
    /// what was actually probed rather than trusting the activity name.
    pub probe: String,
    pub teacher_allowed: bool,
    pub teacher_calls: u32,
    pub expected: ExpectedDisposition,
    pub observed: Option<Observed>,
    pub passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseReport {
    pub phase: Phase,
    pub teacher_allowed: bool,
    pub teacher_calls: u32,
    pub activities: Vec<ActivityReport>,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateReport {
    pub id: String,
    pub stage: GateStage,
    pub store: GateStore,
    pub teacher_calls: u32,
    pub probes: Vec<ActivityReport>,
    /// Pass criteria the manifest states as prose. The runner records them
    /// verbatim because it checks the mechanical parts of the gate (Teacher
    /// absence, store identity, probe dispositions) and cannot check English.
    pub declared_criteria: Vec<String>,
    pub failure_policy: FailurePolicy,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
    pub procedures: Vec<String>,
    pub byte_length: usize,
    /// Prose deny entries from `exportPrivacy`. The typed `secretHandling`
    /// switches are enforced; these are surfaced so a reviewer can see what
    /// the manifest claimed alongside what the filter actually checked.
    pub declared_deny: Vec<String>,
    pub passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportStepReport {
    pub step: ImportStep,
    pub passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanImportReport {
    pub steps: Vec<ImportStepReport>,
    pub gates: Vec<GateReport>,
    /// Replayed cases that the rebuilt procedures had to reproduce in the
    /// target instance, as `procedure name -> reproduced`.
    pub replayed_cases: Vec<(String, bool)>,
    pub promoted: bool,
    /// Always false. Import grants no authority; the target validates locally
    /// or refuses. Recorded so the report states it rather than implying it.
    pub authority_transferred: bool,
    pub passed: bool,
}

/// A detached signature over a report.
///
/// The forge does not produce one. `spoon-secret` owns HMAC-SHA256 signing;
/// once it lands, its signer implements [`ReportSigner`] and the publication
/// step calls [`ForgeReport::attach_signature`]. Nothing in this crate depends
/// on that crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    pub algorithm: String,
    pub key_id: String,
    pub value: String,
}

/// The seam a publication signer attaches to.
pub trait ReportSigner {
    fn sign(&self, payload: &[u8]) -> Result<Signature, ForgeError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeReport {
    pub curriculum_id: String,
    pub curriculum_version: String,
    pub phases: Vec<PhaseReport>,
    pub structures: Vec<StructureFinding>,
    pub gates: Vec<GateReport>,
    pub export: ExportReport,
    pub clean_import: CleanImportReport,
    /// Absent until a signer attaches one. Publication is gated on `passed`,
    /// never on the presence of a signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    pub passed: bool,
}

impl ForgeReport {
    /// The bytes a signature covers: the whole report with any existing
    /// signature removed, so signing is idempotent and verification does not
    /// have to reason about self-reference.
    pub fn signing_payload(&self) -> Result<Vec<u8>, ForgeError> {
        let mut unsigned = self.clone();
        unsigned.signature = None;
        Ok(serde_json::to_vec(&unsigned)?)
    }

    pub fn attach_signature(&mut self, signer: &dyn ReportSigner) -> Result<(), ForgeError> {
        let payload = self.signing_payload()?;
        self.signature = Some(signer.sign(&payload)?);
        Ok(())
    }
}
