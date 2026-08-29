//! Seed curriculum manifests: the Rust mirror of `seeds/curriculum.schema.json`.
//!
//! Deserialization is strict and every closed value set is an enum, so an
//! unknown `kind`, a stray property, or a bad variant fails at parse time.
//! Cardinality and cross-field constraints that the type system cannot carry
//! are re-checked in [`Curriculum::validate`]. A manifest that survives both is
//! one the runner can execute without guessing.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ForgeError;

/// The only manifest schema revision this crate understands.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CurriculumKind {
    #[serde(rename = "spoon-seed-curriculum")]
    SeedCurriculum,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Curriculum {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
    pub schema_version: u32,
    pub kind: CurriculumKind,
    pub id: String,
    pub version: String,
    pub title: String,
    pub domain: String,
    pub evidence: Evidence,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    pub lesson_contract: LessonContract,
    pub required_native_operations: Vec<NativeOperation>,
    pub required_capabilities: Vec<Capability>,
    pub demonstrations: Vec<Activity>,
    pub counterexamples: Vec<Activity>,
    pub exercises: Vec<Activity>,
    pub held_out_generalization: Vec<Activity>,
    pub expected_learned_structures: Vec<LearnedStructure>,
    pub teacher_off_gates: Vec<TeacherOffGate>,
    pub export_privacy: ExportPrivacy,
    pub independent_clean_import_validation: CleanImportValidation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceLevel {
    #[serde(rename = "Declared/design-only")]
    DeclaredDesignOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunnerStatus {
    NotImplemented,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Evidence {
    pub level: EvidenceLevel,
    pub runner_status: RunnerStatus,
    pub claim_boundary: String,
    pub independent_review_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub included: Vec<String>,
    pub excluded: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeacherProtocol {
    PureExprV2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DraftSection {
    Concepts,
    Relationships,
    Procedures,
    Invocation,
    Contracts,
    Interpretations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestCasePolicy {
    EngineGeneratedAndReplayedFromCurriculum,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LessonContract {
    pub teacher_protocol: TeacherProtocol,
    pub draft_shape: Vec<DraftSection>,
    pub teacher_supplied_fields: Vec<String>,
    pub engine_owned_fields: Vec<String>,
    pub test_case_policy: TestCasePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Determinism {
    Deterministic,
    Observation,
    Effectful,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclaredStatus {
    DeclaredNotVerified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeOperation {
    pub name: String,
    pub role: String,
    pub determinism: Determinism,
    pub status: DeclaredStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Authority {
    None,
    ScopedRead,
    ScopedWrite,
    Sandbox,
    Observation,
}

/// The effect vocabulary a curriculum may declare. It is deliberately its own
/// enum: the manifest describes intent, while [`spoon_capability::Effect`]
/// describes what a bundle can actually do, and the export filter compares them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CurriculumEffect {
    None,
    FileRead,
    FileWrite,
    ProcessExec,
    Network,
    Observation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GrantPolicy {
    LocalExplicitGrant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capability {
    pub name: String,
    pub purpose: String,
    pub authority: Authority,
    pub effects: Vec<CurriculumEffect>,
    pub grant_policy: GrantPolicy,
    pub status: DeclaredStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_boundary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeacherMode {
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExpectedDisposition {
    Accept,
    Clarify,
    Reject,
    Abstain,
    FailClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceTier {
    Hard,
    Consensus,
    DeferredWeak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Oracle {
    Deterministic,
    IndependentJudge,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceVariation {
    None,
    ParaphraseFamilies,
    StructuralVariants,
    RepositoryVariants,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueVariation {
    None,
    HeldOutValues,
    HeldOutSchemas,
    HeldOutRepository,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StructureVariation {
    None,
    NewComposition,
    NewErrorBoundary,
    NewToolchain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VariationPolicy {
    pub surface_variation: SurfaceVariation,
    pub value_variation: ValueVariation,
    pub structure_variation: StructureVariation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskModel {
    pub input_shape: String,
    pub operation: String,
    pub variation_policy: VariationPolicy,
    pub no_answer_dump: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ambiguity_dimensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityEvidence {
    pub tier: EvidenceTier,
    pub oracle: Oracle,
    pub assertions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Activity {
    pub id: String,
    pub purpose: String,
    pub teacher_mode: TeacherMode,
    pub task_model: TaskModel,
    pub expected_behavior: Vec<String>,
    pub expected_disposition: ExpectedDisposition,
    pub evidence: ActivityEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_signal: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StructureType {
    Concept,
    Relationship,
    Procedure,
    Contract,
    DependencyGraph,
    TestSet,
    IntentFrame,
    ResponsePlan,
    RepositoryModel,
    Workflow,
    SemanticLowering,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityPolicy {
    SemanticPropertiesOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceExpectation {
    pub teacher_off: bool,
    pub replayable: bool,
    pub local_validation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LearnedStructure {
    pub structure_type: StructureType,
    pub identity_policy: IdentityPolicy,
    pub semantic_properties: Vec<String>,
    pub composition_role: String,
    pub evidence_expectation: EvidenceExpectation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateStage {
    Retention,
    Composition,
    HeldOutGeneralization,
    CleanImport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateStore {
    SameCleanCurriculumStore,
    CleanImportTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailurePolicy {
    FailClosedAndPreserveEvidence,
    QuarantineAndReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeacherOffGate {
    pub id: String,
    pub stage: GateStage,
    pub teacher_mode: TeacherMode,
    pub independent_store: GateStore,
    pub requires: Vec<String>,
    pub pass_criteria: Vec<String>,
    pub failure_policy: FailurePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportMode {
    ReconstructibleOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MachinePathPolicy {
    Omit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TeacherStatePolicy {
    OmitPromptsAndProviderState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretHandling {
    pub never_export_values: bool,
    pub export_kinds_only: bool,
    pub machine_path_policy: MachinePathPolicy,
    pub teacher_state_policy: TeacherStatePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportPrivacy {
    pub mode: ExportMode,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub redactions: Vec<String>,
    pub secret_handling: SecretHandling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourcePurpose {
    CurriculumAcquisitionOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetPurpose {
    IndependentReconstruction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportLifecycle {
    QuarantineProvisional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportStep {
    VerifyManifestAndContentHashes,
    ResolveDependencyClosure,
    CheckLocalPermissions,
    ReconstructAndRunDeterministicTests,
    RunTeacherOffGates,
    PromoteOnlyLocalEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceStore {
    pub clean: bool,
    pub purpose: SourcePurpose,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetStore {
    pub clean: bool,
    pub new_instance: bool,
    pub purpose: TargetPurpose,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromotionGate {
    pub local_evidence_required: bool,
    pub teacher_off_required: bool,
    pub authority_transferred: bool,
    pub failure_is_atomic: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanImportValidation {
    pub source_store: SourceStore,
    pub target_store: TargetStore,
    pub import_lifecycle: ImportLifecycle,
    pub steps: Vec<ImportStep>,
    pub promotion_gate: PromotionGate,
}

impl Curriculum {
    /// Parse and fully validate a manifest.
    pub fn from_json_str(source: &str) -> Result<Self, ForgeError> {
        let curriculum: Self = serde_json::from_str(source)
            .map_err(|error| ForgeError::Manifest(error.to_string()))?;
        curriculum.validate()?;
        Ok(curriculum)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ForgeError> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path).map_err(|error| {
            ForgeError::Manifest(format!("cannot read {}: {error}", path.display()))
        })?;
        Self::from_json_str(&source)
            .map_err(|error| ForgeError::Manifest(format!("{}: {error}", path.display())))
    }

    /// Every activity the runner can drive, in curriculum order.
    pub fn activities(&self) -> impl Iterator<Item = &Activity> {
        self.demonstrations
            .iter()
            .chain(&self.counterexamples)
            .chain(&self.exercises)
            .chain(&self.held_out_generalization)
    }

    /// Re-checks the schema constraints that serde cannot carry in the type
    /// system: cardinality floors, identifier patterns, non-empty prose, and
    /// the `const: true` flags the schema uses as policy switches.
    pub fn validate(&self) -> Result<(), ForgeError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ForgeError::Manifest(format!(
                "schemaVersion must be {SCHEMA_VERSION}, found {}",
                self.schema_version
            )));
        }
        kebab_id("id", &self.id)?;
        semver("version", &self.version)?;
        for (field, value) in [
            ("title", &self.title),
            ("domain", &self.domain),
            ("objective", &self.objective),
            ("evidence.claimBoundary", &self.evidence.claim_boundary),
        ] {
            text(field, value)?;
        }
        flag(
            "evidence.independentReviewRequired",
            self.evidence.independent_review_required,
        )?;
        if let Some(scope) = &self.scope {
            strings("scope.included", &scope.included, 1)?;
            strings("scope.excluded", &scope.excluded, 1)?;
        }
        self.validate_lesson_contract()?;
        self.validate_requirements()?;
        self.validate_activities()?;
        self.validate_structures()?;
        self.validate_gates()?;
        self.validate_export_privacy()?;
        self.validate_clean_import()
    }

    fn validate_lesson_contract(&self) -> Result<(), ForgeError> {
        let contract = &self.lesson_contract;
        min_items("lessonContract.draftShape", contract.draft_shape.len(), 5)?;
        let unique: BTreeSet<_> = contract.draft_shape.iter().collect();
        if unique.len() != contract.draft_shape.len() {
            return Err(ForgeError::Manifest(
                "lessonContract.draftShape must not repeat a section".into(),
            ));
        }
        strings(
            "lessonContract.teacherSuppliedFields",
            &contract.teacher_supplied_fields,
            1,
        )?;
        strings(
            "lessonContract.engineOwnedFields",
            &contract.engine_owned_fields,
            1,
        )
    }

    fn validate_requirements(&self) -> Result<(), ForgeError> {
        min_items(
            "requiredNativeOperations",
            self.required_native_operations.len(),
            1,
        )?;
        for operation in &self.required_native_operations {
            operation_name("requiredNativeOperations[].name", &operation.name)?;
            text("requiredNativeOperations[].role", &operation.role)?;
        }
        for capability in &self.required_capabilities {
            operation_name("requiredCapabilities[].name", &capability.name)?;
            text("requiredCapabilities[].purpose", &capability.purpose)?;
            if let Some(boundary) = &capability.fixture_boundary {
                text("requiredCapabilities[].fixtureBoundary", boundary)?;
            }
        }
        Ok(())
    }

    fn validate_activities(&self) -> Result<(), ForgeError> {
        for (field, group) in [
            ("demonstrations", &self.demonstrations),
            ("counterexamples", &self.counterexamples),
            ("exercises", &self.exercises),
            ("heldOutGeneralization", &self.held_out_generalization),
        ] {
            min_items(field, group.len(), 1)?;
            for activity in group {
                activity.validate(field)?;
            }
        }
        let mut seen = BTreeSet::new();
        for activity in self.activities() {
            if !seen.insert(activity.id.as_str()) {
                return Err(ForgeError::Manifest(format!(
                    "activity id '{}' is used more than once",
                    activity.id
                )));
            }
        }
        for activity in &self.held_out_generalization {
            if activity.teacher_mode != TeacherMode::Off {
                return Err(ForgeError::Manifest(format!(
                    "heldOutGeneralization activity '{}' must run with the Teacher off",
                    activity.id
                )));
            }
        }
        Ok(())
    }

    fn validate_structures(&self) -> Result<(), ForgeError> {
        min_items(
            "expectedLearnedStructures",
            self.expected_learned_structures.len(),
            1,
        )?;
        for structure in &self.expected_learned_structures {
            strings(
                "expectedLearnedStructures[].semanticProperties",
                &structure.semantic_properties,
                2,
            )?;
            text(
                "expectedLearnedStructures[].compositionRole",
                &structure.composition_role,
            )?;
            let expectation = &structure.evidence_expectation;
            flag("evidenceExpectation.teacherOff", expectation.teacher_off)?;
            flag("evidenceExpectation.replayable", expectation.replayable)?;
            flag(
                "evidenceExpectation.localValidation",
                expectation.local_validation,
            )?;
        }
        Ok(())
    }

    fn validate_gates(&self) -> Result<(), ForgeError> {
        min_items("teacherOffGates", self.teacher_off_gates.len(), 2)?;
        let mut seen = BTreeSet::new();
        for gate in &self.teacher_off_gates {
            kebab_id("teacherOffGates[].id", &gate.id)?;
            if !seen.insert(gate.id.as_str()) {
                return Err(ForgeError::Manifest(format!(
                    "teacher-off gate id '{}' is used more than once",
                    gate.id
                )));
            }
            if gate.teacher_mode != TeacherMode::Off {
                return Err(ForgeError::Manifest(format!(
                    "teacher-off gate '{}' must declare teacherMode 'off'",
                    gate.id
                )));
            }
            strings("teacherOffGates[].requires", &gate.requires, 1)?;
            strings("teacherOffGates[].passCriteria", &gate.pass_criteria, 2)?;
        }
        Ok(())
    }

    fn validate_export_privacy(&self) -> Result<(), ForgeError> {
        let privacy = &self.export_privacy;
        strings("exportPrivacy.allow", &privacy.allow, 1)?;
        strings("exportPrivacy.deny", &privacy.deny, 5)?;
        strings("exportPrivacy.redactions", &privacy.redactions, 1)?;
        flag(
            "exportPrivacy.secretHandling.neverExportValues",
            privacy.secret_handling.never_export_values,
        )?;
        flag(
            "exportPrivacy.secretHandling.exportKindsOnly",
            privacy.secret_handling.export_kinds_only,
        )
    }

    fn validate_clean_import(&self) -> Result<(), ForgeError> {
        let validation = &self.independent_clean_import_validation;
        flag(
            "independentCleanImportValidation.sourceStore.clean",
            validation.source_store.clean,
        )?;
        flag(
            "independentCleanImportValidation.targetStore.clean",
            validation.target_store.clean,
        )?;
        flag(
            "independentCleanImportValidation.targetStore.newInstance",
            validation.target_store.new_instance,
        )?;
        min_items(
            "independentCleanImportValidation.steps",
            validation.steps.len(),
            5,
        )?;
        let gate = &validation.promotion_gate;
        flag(
            "promotionGate.localEvidenceRequired",
            gate.local_evidence_required,
        )?;
        flag(
            "promotionGate.teacherOffRequired",
            gate.teacher_off_required,
        )?;
        flag("promotionGate.failureIsAtomic", gate.failure_is_atomic)?;
        if gate.authority_transferred {
            return Err(ForgeError::Manifest(
                "promotionGate.authorityTransferred must be false; installation never transfers authority".into(),
            ));
        }
        Ok(())
    }
}

impl Activity {
    fn validate(&self, group: &str) -> Result<(), ForgeError> {
        kebab_id(&format!("{group}[].id"), &self.id)?;
        text(&format!("{group}[{}].purpose", self.id), &self.purpose)?;
        text(
            &format!("{group}[{}].taskModel.inputShape", self.id),
            &self.task_model.input_shape,
        )?;
        text(
            &format!("{group}[{}].taskModel.operation", self.id),
            &self.task_model.operation,
        )?;
        flag(
            &format!("{group}[{}].taskModel.noAnswerDump", self.id),
            self.task_model.no_answer_dump,
        )?;
        strings(
            &format!("{group}[{}].expectedBehavior", self.id),
            &self.expected_behavior,
            1,
        )?;
        strings(
            &format!("{group}[{}].evidence.assertions", self.id),
            &self.evidence.assertions,
            1,
        )?;
        if let Some(signal) = &self.failure_signal {
            text(&format!("{group}[{}].failureSignal", self.id), signal)?;
        }
        Ok(())
    }
}

fn text(field: &str, value: &str) -> Result<(), ForgeError> {
    if value.trim().is_empty() {
        return Err(ForgeError::Manifest(format!("{field} must not be empty")));
    }
    Ok(())
}

fn strings(field: &str, values: &[String], min: usize) -> Result<(), ForgeError> {
    min_items(field, values.len(), min)?;
    for value in values {
        text(field, value)?;
    }
    Ok(())
}

fn min_items(field: &str, count: usize, min: usize) -> Result<(), ForgeError> {
    if count < min {
        return Err(ForgeError::Manifest(format!(
            "{field} requires at least {min} entries, found {count}"
        )));
    }
    Ok(())
}

fn flag(field: &str, value: bool) -> Result<(), ForgeError> {
    if !value {
        return Err(ForgeError::Manifest(format!("{field} must be true")));
    }
    Ok(())
}

fn kebab_id(field: &str, value: &str) -> Result<(), ForgeError> {
    let valid = !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        });
    if !valid {
        return Err(ForgeError::Manifest(format!(
            "{field} '{value}' must be lowercase kebab-case"
        )));
    }
    Ok(())
}

fn operation_name(field: &str, value: &str) -> Result<(), ForgeError> {
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && value.len() >= 2
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '.' | '-')
        });
    if !valid {
        return Err(ForgeError::Manifest(format!(
            "{field} '{value}' must be a portable lowercase operation name"
        )));
    }
    Ok(())
}

fn semver(field: &str, value: &str) -> Result<(), ForgeError> {
    let parts: Vec<&str> = value.split('.').collect();
    let valid = parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if !valid {
        return Err(ForgeError::Manifest(format!(
            "{field} '{value}' must be major.minor.patch"
        )));
    }
    Ok(())
}
