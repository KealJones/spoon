use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use spoon_capability::{
    BUNDLE_FORMAT_VERSION, CapabilityBundle, CapabilityProcedure, CapabilityTest, Effect,
    NativePrimitive, NeutralProcedureKind, NeutralProcedureMetadata, Permission, Provenance,
    ProvenanceIdentity, ProvenanceIdentityKind, ReconstructionRecipe, ReconstructionStep,
    ResourceBounds, bundle_content_id, export_bundle,
};
use spoon_core::{
    Concept, Condition, Contract, Expr, Lifecycle, MutabilityClass, Param, Procedure, ProcedureId,
    Value,
};
use spoon_engine::Engine;

use crate::ForgeError;
use crate::curriculum::{Curriculum, ExportMode, MachinePathPolicy, TeacherStatePolicy};

/// Neutral IR version carried in the bundle. Bumping it is a persistence
/// break, so the importer refuses anything it does not recognize.
const SEED_IR_VERSION: u16 = 1;

/// One recorded execution the exported seed must be able to reproduce.
///
/// These come from the acquisition run's own traces, so the seed ships with
/// the evidence that produced it rather than with fixtures someone wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplayCase {
    pub inputs: BTreeMap<String, Value>,
    pub expected: Value,
}

/// An acquired procedure stripped of every local identity, so a clean target
/// can rebuild it under its own ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NeutralProcedure {
    pub ir_version: u16,
    pub name: String,
    pub concept: String,
    pub concept_description: String,
    pub mutability: MutabilityClass,
    pub params: Vec<NeutralParam>,
    pub body: Expr,
    pub requires: Vec<NeutralCondition>,
    pub promises: Vec<NeutralCondition>,
    pub fails_when: Vec<NeutralCondition>,
    pub cases: Vec<ReplayCase>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NeutralParam {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NeutralCondition {
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Expr>,
}

/// A privacy-filtered, content-addressed seed ready to leave the machine.
#[derive(Debug, Clone)]
pub struct SeedBundle {
    pub bundle: CapabilityBundle,
    pub bytes: Vec<u8>,
}

impl SeedBundle {
    pub fn content_id(&self) -> &str {
        &self.bundle.content_id
    }

    pub fn procedure_names(&self) -> Vec<String> {
        self.bundle
            .procedures
            .iter()
            .map(|procedure| procedure.name.clone())
            .collect()
    }
}

/// The export rules a curriculum's `exportPrivacy` block resolves to.
///
/// The manifest's `allow`, `deny`, and `redactions` lists are prose and are
/// reported verbatim rather than parsed. The typed switches below are the ones
/// that become checks, and each is matched exhaustively so a new schema
/// variant forces a decision here instead of silently weakening the filter.
#[derive(Debug, Clone, Copy)]
pub struct ExportPolicy {
    reconstructible_only: bool,
    omit_machine_paths: bool,
    omit_teacher_state: bool,
    secret_values_only_as_kinds: bool,
}

impl ExportPolicy {
    pub fn from_curriculum(curriculum: &Curriculum) -> Self {
        let privacy = &curriculum.export_privacy;
        Self {
            reconstructible_only: match privacy.mode {
                ExportMode::ReconstructibleOnly => true,
            },
            omit_machine_paths: match privacy.secret_handling.machine_path_policy {
                MachinePathPolicy::Omit => true,
            },
            omit_teacher_state: match privacy.secret_handling.teacher_state_policy {
                TeacherStatePolicy::OmitPromptsAndProviderState => true,
            },
            secret_values_only_as_kinds: privacy.secret_handling.never_export_values
                && privacy.secret_handling.export_kinds_only,
        }
    }

    /// Refuse a document that carries anything the policy excludes.
    ///
    /// Refusal is the point. A seed that cannot be exported without leaking is
    /// a seed that must not be exported.
    pub fn enforce(&self, subject: &str, document: &Json) -> Result<(), ForgeError> {
        let mut violation = None;
        walk(document, "", &mut |key, text| {
            if violation.is_some() {
                return;
            }
            violation = self.inspect(key, text);
        });
        match violation {
            Some(violation) => Err(ForgeError::ExportRefused {
                subject: subject.into(),
                violation,
            }),
            None => Ok(()),
        }
    }

    fn inspect(&self, key: &str, text: &str) -> Option<String> {
        let key_words = key.to_ascii_lowercase().replace(['_', '-'], "");
        if self.omit_teacher_state
            && ["teacher", "prompt", "provider", "transcript", "completion"]
                .iter()
                .any(|needle| key_words.contains(needle))
        {
            return Some(format!("Teacher or provider state at '{key}'"));
        }
        if self.secret_values_only_as_kinds && looks_like_secret(text) {
            return Some(format!("a secret value at '{key}'"));
        }
        if self.omit_machine_paths && looks_machine_local(text) {
            return Some(format!("a machine-local path at '{key}'"));
        }
        if self.reconstructible_only && is_uuid(text) {
            return Some(format!(
                "a local episode or knowledge identifier at '{key}'"
            ));
        }
        None
    }
}

/// Project the engine's acquired procedures into a portable bundle, filter it,
/// and encode it.
///
/// The seed ships as inert `Observe` procedures: the only native primitive
/// whose permission names a subject rather than a host, path, or executable.
/// Nothing in the bundle can act; the neutral IR in each contract is what the
/// target rebuilds.
pub fn build_seed_bundle(
    curriculum: &Curriculum,
    engine: &Engine,
    cases: &HashMap<ProcedureId, Vec<ReplayCase>>,
) -> Result<SeedBundle, ForgeError> {
    let graph = engine.graph();
    let procedures = graph.list_procedures()?;
    if procedures.is_empty() {
        return Err(ForgeError::ExportRefused {
            subject: curriculum.id.clone(),
            violation: "no acquired procedure to export".into(),
        });
    }
    let fingerprint = format!("{}@{}", curriculum.id, curriculum.version);
    let acquired_at = procedures
        .iter()
        .map(|procedure| procedure.created_at)
        .max()
        .filter(|created| *created > 0)
        .unwrap_or(1);
    let provenance = Provenance {
        source: curriculum.id.clone(),
        discovered_at: acquired_at,
        interface_fingerprint: fingerprint.clone(),
        identities: vec![
            ProvenanceIdentity {
                kind: ProvenanceIdentityKind::Author,
                scheme: "spoon_seed_curriculum".into(),
                identifier: curriculum.id.clone(),
            },
            ProvenanceIdentity {
                kind: ProvenanceIdentityKind::Discoverer,
                scheme: "spoon_forge".into(),
                identifier: fingerprint,
            },
        ],
        // Episode references would make the bundle a trust receipt. The target
        // produces its own evidence or refuses to promote.
        validation_episodes: Vec::new(),
        evidence_references: Vec::new(),
    };

    let mut exported = Vec::with_capacity(procedures.len());
    for procedure in &procedures {
        let neutral = neutralize(
            &graph,
            procedure,
            cases.get(&procedure.id).map(Vec::as_slice).unwrap_or(&[]),
        )?;
        exported.push(capability_procedure(
            curriculum,
            &neutral,
            provenance.clone(),
        )?);
    }

    let mut bundle = CapabilityBundle {
        format_version: BUNDLE_FORMAT_VERSION,
        name: curriculum.id.clone(),
        version: curriculum.version.clone(),
        content_id: String::new(),
        procedures: exported,
        dependencies: Vec::new(),
        provenance,
        reconstruction: ReconstructionRecipe {
            kind: "native_primitive_procedure".into(),
            recipe_version: 1,
            compatibility: vec!["spoon-capability-neutral-ir-v1".into()],
            steps: [
                "verify_canonical_manifest",
                "map_neutral_procedures",
                "run_portable_fixtures",
            ]
            .iter()
            .enumerate()
            .map(|(index, operation)| ReconstructionStep {
                sequence: (index + 1) as u16,
                operation: (*operation).into(),
                artifact_digest: None,
            })
            .collect(),
        },
    };
    bundle.content_id = bundle_content_id(&bundle)?;

    ExportPolicy::from_curriculum(curriculum)
        .enforce(&curriculum.id, &serde_json::to_value(&bundle)?)?;
    let bytes = export_bundle(&bundle)?;
    Ok(SeedBundle { bundle, bytes })
}

/// Rebuild a procedure from a bundle entry under the target's own identities.
///
/// The target mints fresh concept and procedure ids and installs everything as
/// Provisional, which is what makes an import a proposal rather than a
/// transfer of standing.
pub(crate) fn install_seed(
    engine: &Engine,
    procedure: &CapabilityProcedure,
) -> Result<(ProcedureId, String, Vec<ReplayCase>), ForgeError> {
    let neutral = neutral_from(procedure)?;
    let mut concept = Concept::new(&neutral.concept, neutral.mutability);
    concept.description = Some(neutral.concept_description.clone());
    concept.lifecycle = Lifecycle::Provisional;
    engine.admin_insert_concept(&concept)?;

    let params = neutral
        .params
        .iter()
        .map(|param| Param {
            name: param.name.clone(),
            description: param.description.clone(),
            value_type: None,
        })
        .collect();
    let mut rebuilt = Procedure::new(&neutral.name, params, neutral.body.clone())
        .with_contract(Contract {
            requires: conditions(&neutral.requires),
            promises: conditions(&neutral.promises),
            fails_when: conditions(&neutral.fails_when),
            ..Contract::default()
        })
        .with_concept(concept.id);
    rebuilt.lifecycle = Lifecycle::Provisional;
    engine.admin_insert_procedure(&rebuilt)?;
    Ok((rebuilt.id, neutral.name.clone(), neutral.cases.clone()))
}

pub(crate) fn neutral_from(
    procedure: &CapabilityProcedure,
) -> Result<NeutralProcedure, ForgeError> {
    let raw = procedure.contract.get("seedProcedure").ok_or_else(|| {
        ForgeError::Manifest(format!(
            "bundle procedure '{}' carries no neutral seed IR",
            procedure.name
        ))
    })?;
    let neutral: NeutralProcedure = serde_json::from_value(raw.clone())?;
    if neutral.ir_version != SEED_IR_VERSION {
        return Err(ForgeError::Manifest(format!(
            "seed IR version {} is unsupported",
            neutral.ir_version
        )));
    }
    Ok(neutral)
}

/// Every procedure a body invokes by stored identity.
///
/// Shared with structural inspection: the same walk that tells the exporter a
/// body is not portable tells the inspector the body has dependencies.
pub(crate) fn procedure_references(expr: &Expr) -> Vec<ProcedureId> {
    fn visit(expr: &Expr, found: &mut Vec<ProcedureId>) {
        match expr {
            Expr::Call { procedure, args }
            | Expr::CallExact {
                procedure, args, ..
            } => {
                found.push(*procedure);
                args.iter().for_each(|arg| visit(arg, found));
            }
            Expr::Literal(_) | Expr::Var(_) => {}
            Expr::BinOp { left, right, .. } => {
                visit(left, found);
                visit(right, found);
            }
            Expr::UnOp { operand, .. } => visit(operand, found),
            Expr::CapabilityCall { input, .. } => visit(input, found),
            Expr::If { cond, then, else_ } => {
                visit(cond, found);
                visit(then, found);
                visit(else_, found);
            }
            Expr::Let { value, body, .. } => {
                visit(value, found);
                visit(body, found);
            }
            Expr::Block(items) | Expr::ListExpr(items) | Expr::Intrinsic { args: items, .. } => {
                items.iter().for_each(|item| visit(item, found));
            }
            Expr::Index { collection, index } => {
                visit(collection, found);
                visit(index, found);
            }
            Expr::FieldAccess { object, .. } => visit(object, found),
            Expr::Map {
                collection, body, ..
            } => {
                visit(collection, found);
                visit(body, found);
            }
            Expr::Filter {
                collection,
                predicate,
                ..
            } => {
                visit(collection, found);
                visit(predicate, found);
            }
            Expr::Reduce {
                collection,
                init,
                body,
                ..
            } => {
                visit(collection, found);
                visit(init, found);
                visit(body, found);
            }
        }
    }
    let mut found = Vec::new();
    visit(expr, &mut found);
    found
}

fn neutralize(
    graph: &spoon_engine::GraphView<'_>,
    procedure: &Procedure,
    cases: &[ReplayCase],
) -> Result<NeutralProcedure, ForgeError> {
    // A body that names another procedure by stored id cannot be rebuilt in a
    // store that has never seen that id. Composition across exported
    // procedures needs a name-based reference in the IR, which this format
    // does not have yet, so the export refuses rather than shipping a bundle
    // the target would fail to reconstruct.
    if !procedure_references(&procedure.body).is_empty() {
        return Err(ForgeError::ExportRefused {
            subject: procedure.name.clone(),
            violation: "a call to another procedure by local identity".into(),
        });
    }
    if cases.is_empty() {
        return Err(ForgeError::ExportRefused {
            subject: procedure.name.clone(),
            violation: "no recorded case, so the seed would not be replayable".into(),
        });
    }
    let concept = procedure
        .concept
        .and_then(|id| graph.get_concept(id).ok().flatten());
    let concept_name = concept
        .as_ref()
        .map(|concept| concept.name.clone())
        .unwrap_or_else(|| procedure.name.clone());
    Ok(NeutralProcedure {
        ir_version: SEED_IR_VERSION,
        name: procedure.name.clone(),
        concept: concept_name,
        concept_description: concept
            .as_ref()
            .and_then(|concept| concept.description.clone())
            .unwrap_or_default(),
        mutability: concept
            .as_ref()
            .map_or(MutabilityClass::Procedural, |concept| concept.mutability),
        params: procedure
            .params
            .iter()
            .map(|param| NeutralParam {
                name: param.name.clone(),
                description: param.description.clone(),
            })
            .collect(),
        body: procedure.body.clone(),
        requires: neutral_conditions(&procedure.contract.requires),
        promises: neutral_conditions(&procedure.contract.promises),
        fails_when: neutral_conditions(&procedure.contract.fails_when),
        cases: cases.to_vec(),
    })
}

fn capability_procedure(
    curriculum: &Curriculum,
    neutral: &NeutralProcedure,
    provenance: Provenance,
) -> Result<CapabilityProcedure, ForgeError> {
    let target = format!("seed:{}/{}", curriculum.id, slug(&neutral.name));
    let fixture = json!({ "procedure": neutral.name, "cases": neutral.cases.len() });
    Ok(CapabilityProcedure {
        id: format!("{}:{}", curriculum.id, slug(&neutral.name)),
        name: neutral.name.clone(),
        version: 1,
        primitive: NativePrimitive::Observe,
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        contract: json!({
            "target": target,
            "replayable": true,
            "seedProcedure": serde_json::to_value(neutral)?,
        }),
        neutral_metadata: NeutralProcedureMetadata {
            kind: NeutralProcedureKind::NativePrimitive,
            ir_version: SEED_IR_VERSION,
            fixture_format: "json".into(),
        },
        permissions: vec![Permission::ObserveTarget { target }],
        effects: vec![Effect::Observation],
        bounds: ResourceBounds::default(),
        dependencies: Vec::new(),
        tests: vec![CapabilityTest {
            name: format!("{} seed inventory", neutral.name),
            input: json!({}),
            expected_output: fixture.clone(),
            fixture_output: fixture,
        }],
        provenance,
    })
}

fn neutral_conditions(conditions: &[Condition]) -> Vec<NeutralCondition> {
    conditions
        .iter()
        .map(|condition| NeutralCondition {
            description: condition.description.clone(),
            check: condition.check.clone(),
        })
        .collect()
}

fn conditions(neutral: &[NeutralCondition]) -> Vec<Condition> {
    neutral
        .iter()
        .map(|condition| Condition {
            description: condition.description.clone(),
            check: condition.check.clone(),
        })
        .collect()
}

fn slug(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

fn walk(value: &Json, key: &str, visit: &mut impl FnMut(&str, &str)) {
    match value {
        Json::Object(fields) => {
            for (name, child) in fields {
                visit(name, "");
                walk(child, name, visit);
            }
        }
        Json::Array(items) => items.iter().for_each(|item| walk(item, key, visit)),
        Json::String(text) => visit(key, text),
        _ => {}
    }
}

fn looks_like_secret(text: &str) -> bool {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("bearer ")
        || lower.starts_with("basic ")
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("glpat-")
        || lower.starts_with("xoxb-")
        || lower.starts_with("xoxp-")
        || (lower.starts_with("sk-") && trimmed.len() >= 20)
        || (trimmed.starts_with("AKIA") && trimmed.len() >= 16)
}

fn looks_machine_local(text: &str) -> bool {
    let trimmed = text.trim();
    let bytes = trimmed.as_bytes();
    let windows_drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    trimmed.starts_with('/')
        || trimmed.starts_with("~/")
        || trimmed.starts_with("\\\\")
        || windows_drive
        || trimmed.to_ascii_lowercase().starts_with("file://")
        || trimmed.contains("${")
}

fn is_uuid(text: &str) -> bool {
    let text = text.trim();
    text.len() == 36
        && text.split('-').map(str::len).eq([8, 4, 4, 4, 12])
        && text
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
}
