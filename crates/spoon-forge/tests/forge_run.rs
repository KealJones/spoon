//! End-to-end forge behavior against a deliberately tiny curriculum authored
//! here. The checked-in seeds are design-only, so they are used for loading
//! and validation elsewhere; the behavior below needs a curriculum whose
//! probes a clean engine can actually route.

use serde_json::{Value as Json, json};
use spoon_engine::{TeacherProposalWire, TeacherRequestWire};
use spoon_forge::curriculum::GateStore;
use spoon_forge::{
    Curriculum, CurriculumTeacher, ExportPolicy, ForgeError, ForgeReport, ForgeRunner, Observed,
    Phase, ReportSigner, Signature, StructureStatus,
};

/// A teacher that only knows how to double, records every situation it was
/// asked about, and declines everything else.
///
/// Declining matters as much as answering: the counterexample only proves the
/// engine did not overgeneralize if the fallback is an abstention rather than
/// a guess.
#[derive(Default)]
struct ScriptedTeacher {
    asked: Vec<String>,
}

impl CurriculumTeacher for ScriptedTeacher {
    fn respond(&mut self, request: &TeacherRequestWire) -> Result<TeacherProposalWire, ForgeError> {
        self.asked.push(request.situation.clone());
        let content = match doubling_target(&request.situation) {
            Some(value) => double_lesson(value),
            None => json!({
                "proposalKind": "abstain",
                "interpretations": [],
                "abstainReason": "this teacher only knows doubling"
            }),
        };
        Ok(TeacherProposalWire {
            content,
            source: "human:scripted-test".into(),
            status: "unverified".into(),
            provenance: json!({
                "provider": "human",
                "teacher": "human:scripted-test",
                "requestId": format!("scripted-{}", self.asked.len()),
                "generatedAt": "2026-08-28T00:00:00Z",
                "situation": request.situation,
            }),
            validation: None,
        })
    }
}

fn doubling_target(situation: &str) -> Option<i64> {
    situation.contains("double").then(|| {
        situation
            .split(|character: char| !character.is_ascii_digit())
            .find_map(|token| token.parse::<i64>().ok())
    })?
}

fn double_lesson(value: i64) -> Json {
    json!({
        "proposalKind": "reusable_lesson",
        "interpretations": [],
        "lesson": {
            "primitiveSet": "pure_rpn_v1",
            "concepts": [{
                "key": "double",
                "name": "DOUBLE",
                "description": "Multiply any numeric input by two"
            }],
            "relationships": [],
            "procedures": [{
                "key": "double-procedure",
                "name": "DOUBLE",
                "concept": { "kind": "new_concept", "key": "double" },
                "parameters": [{ "name": "x", "description": "numeric input", "valueType": "number" }],
                "body": { "instructions": [
                    { "op": "load_parameter", "name": "x" },
                    { "op": "push_literal", "value": 2 },
                    { "op": "multiply" }
                ]},
                "contract": {
                    "requires": [],
                    "promises": [{
                        "description": "result is twice x",
                        "check": { "instructions": [
                            { "op": "load_result" },
                            { "op": "load_parameter", "name": "x" },
                            { "op": "push_literal", "value": 2 },
                            { "op": "multiply" },
                            { "op": "equal" }
                        ]}
                    }],
                    "failsWhen": []
                }
            }],
            "invocation": {
                "procedureKey": "double-procedure",
                "inputs": [{ "name": "x", "value": value }]
            }
        },
        "procedure": null,
        "answer": value * 2,
        "abstainReason": null
    })
}

const HELD_OUT_PROBE: &str = "what is double 21?";

fn activity(id: &str, operation: &str, teacher_mode: &str, disposition: &str) -> Json {
    json!({
        "id": id,
        "purpose": format!("probe {operation}"),
        "teacherMode": teacher_mode,
        "taskModel": {
            "inputShape": "a request naming an operation and one integer",
            "operation": operation,
            "variationPolicy": {
                "surfaceVariation": "none",
                "valueVariation": "held-out-values",
                "structureVariation": "none"
            },
            "noAnswerDump": true
        },
        "expectedBehavior": ["reach the declared disposition without a stored answer"],
        "expectedDisposition": disposition,
        "evidence": {
            "tier": "hard",
            "oracle": "deterministic",
            "assertions": ["the disposition matches the declaration"]
        }
    })
}

fn manifest(structures: Json) -> Json {
    json!({
        "schemaVersion": 1,
        "kind": "spoon-seed-curriculum",
        "id": "forge-doubling",
        "version": "0.1.0",
        "title": "Doubling",
        "domain": "arithmetic",
        "evidence": {
            "level": "Declared/design-only",
            "runnerStatus": "not-implemented",
            "claimBoundary": "A test curriculum covering one procedure.",
            "independentReviewRequired": true
        },
        "objective": "Acquire a doubling procedure and keep it without the Teacher.",
        "lessonContract": {
            "teacherProtocol": "pure_expr_v2",
            "draftShape": ["concepts", "relationships", "procedures", "invocation", "contracts"],
            "teacherSuppliedFields": ["concept names", "procedure bodies"],
            "engineOwnedFields": ["stable IDs", "lifecycle"],
            "testCasePolicy": "engine-generated-and-replayed-from-curriculum"
        },
        "requiredNativeOperations": [{
            "name": "math.multiply",
            "role": "Multiply two numbers.",
            "determinism": "deterministic",
            "status": "declared-not-verified"
        }],
        "requiredCapabilities": [],
        "demonstrations": [activity("demo-double", "what is double 7?", "on", "accept")],
        "counterexamples": [activity("counter-triple", "what is triple 7?", "on", "clarify")],
        "exercises": [activity("exercise-double", "what is double 9?", "on", "accept")],
        "heldOutGeneralization": [activity("heldout-double", HELD_OUT_PROBE, "off", "accept")],
        "expectedLearnedStructures": structures,
        "teacherOffGates": [
            gate("retention-off", "retention", "same-clean-curriculum-store"),
            gate("heldout-off", "held-out-generalization", "same-clean-curriculum-store"),
            gate("clean-import-off", "clean-import", "clean-import-target")
        ],
        "exportPrivacy": {
            "mode": "reconstructible-only",
            "allow": ["neutral procedure drafts"],
            "deny": [
                "provider prompts and responses",
                "API keys and bearer tokens",
                "ambient permission grants",
                "machine-local paths",
                "episode identifiers used as trust receipts"
            ],
            "redactions": ["replace secret values with secret kinds"],
            "secretHandling": {
                "neverExportValues": true,
                "exportKindsOnly": true,
                "machinePathPolicy": "omit",
                "teacherStatePolicy": "omit-prompts-and-provider-state"
            }
        },
        "independentCleanImportValidation": {
            "sourceStore": { "clean": true, "purpose": "curriculum-acquisition-output" },
            "targetStore": {
                "clean": true,
                "newInstance": true,
                "purpose": "independent-reconstruction"
            },
            "importLifecycle": "quarantine-provisional",
            "steps": [
                "verify-manifest-and-content-hashes",
                "resolve-dependency-closure",
                "check-local-permissions",
                "reconstruct-and-run-deterministic-tests",
                "run-teacher-off-gates",
                "promote-only-local-evidence"
            ],
            "promotionGate": {
                "localEvidenceRequired": true,
                "teacherOffRequired": true,
                "authorityTransferred": false,
                "failureIsAtomic": true
            }
        }
    })
}

fn gate(id: &str, stage: &str, store: &str) -> Json {
    json!({
        "id": id,
        "stage": stage,
        "teacherMode": "off",
        "independentStore": store,
        "requires": ["acquisition is complete"],
        "passCriteria": ["no Teacher turns occur", "the learned procedure still runs"],
        "failurePolicy": "fail-closed-and-preserve-evidence"
    })
}

fn attainable_structures() -> Json {
    json!([
        {
            "structureType": "concept",
            "identityPolicy": "semantic-properties-only",
            "semanticProperties": ["double", "multiply any numeric input by two"],
            "compositionRole": "Names the operation.",
            "evidenceExpectation": {
                "teacherOff": true,
                "replayable": true,
                "localValidation": true
            }
        },
        {
            "structureType": "procedure",
            "identityPolicy": "semantic-properties-only",
            "semanticProperties": ["double", "result is twice"],
            "compositionRole": "Executes the operation.",
            "evidenceExpectation": {
                "teacherOff": true,
                "replayable": true,
                "localValidation": true
            }
        }
    ])
}

fn curriculum(structures: Json) -> Curriculum {
    Curriculum::from_json_str(&manifest(structures).to_string()).expect("test manifest is valid")
}

fn run(structures: Json) -> (ForgeReport, ScriptedTeacher) {
    let curriculum = curriculum(structures);
    let mut teacher = ScriptedTeacher::default();
    let report = ForgeRunner::new(&curriculum)
        .run(&mut teacher)
        .expect("the run itself should not fail");
    (report, teacher)
}

#[test]
fn a_full_run_acquires_holds_exports_and_reconstructs() {
    let (report, _) = run(attainable_structures());

    for phase in &report.phases {
        assert!(
            phase.passed,
            "{:?} failed: {:#?}",
            phase.phase, phase.activities
        );
    }
    assert!(
        report
            .structures
            .iter()
            .all(|finding| finding.status == StructureStatus::Matched),
        "{:#?}",
        report.structures
    );
    assert!(report.export.passed, "{:?}", report.export.refusal);
    assert!(
        report.clean_import.passed,
        "{:#?}",
        report.clean_import.steps
    );
    assert!(report.passed);
}

#[test]
fn the_teacher_teaches_the_demonstration_and_declines_the_counterexample() {
    let (report, teacher) = run(attainable_structures());

    let demonstrations = &report.phases[0];
    assert_eq!(demonstrations.phase, Phase::Demonstrations);
    assert_eq!(demonstrations.teacher_calls, 1);
    assert_eq!(
        demonstrations.activities[0].observed,
        Some(Observed::Answered)
    );

    // The engine must not stretch DOUBLE over "triple". It has to ask, and the
    // Teacher's refusal has to surface as an abstention rather than a guess.
    let counterexamples = &report.phases[1];
    assert_eq!(counterexamples.phase, Phase::Counterexamples);
    assert_eq!(
        counterexamples.activities[0].observed,
        Some(Observed::Abstained)
    );
    assert!(counterexamples.passed);
    assert!(teacher.asked.iter().any(|asked| asked.contains("triple")));
}

#[test]
fn the_teacher_off_phase_and_gates_never_reach_the_teacher() {
    let (report, teacher) = run(attainable_structures());

    let held_out = report
        .phases
        .iter()
        .find(|phase| phase.phase == Phase::HeldOutGeneralization)
        .unwrap();
    assert!(!held_out.teacher_allowed);
    assert_eq!(held_out.teacher_calls, 0);
    assert_eq!(held_out.activities[0].observed, Some(Observed::Answered));

    for gate in report.gates.iter().chain(&report.clean_import.gates) {
        assert_eq!(
            gate.teacher_calls, 0,
            "gate {} reached the Teacher",
            gate.id
        );
        assert!(gate.passed, "gate {} failed: {:#?}", gate.id, gate.probes);
        assert!(!gate.declared_criteria.is_empty());
    }

    // The strongest form of the claim: the Teacher itself never saw the
    // held-out probe, in any phase or gate.
    assert!(
        !teacher.asked.iter().any(|asked| asked == HELD_OUT_PROBE),
        "the Teacher was asked about the held-out probe: {:?}",
        teacher.asked
    );
}

#[test]
fn the_clean_import_gate_runs_against_the_second_instance() {
    let (report, _) = run(attainable_structures());

    let clean = &report.clean_import;
    assert!(clean.promoted);
    assert!(!clean.authority_transferred);
    assert_eq!(clean.steps.len(), 6);
    for step in &clean.steps {
        assert!(step.passed, "{:?} failed: {:?}", step.step, step.detail);
    }
    assert!(
        clean
            .replayed_cases
            .iter()
            .all(|(_, reproduced)| *reproduced),
        "{:?}",
        clean.replayed_cases
    );
    assert_eq!(clean.gates.len(), 1);
    assert_eq!(clean.gates[0].store, GateStore::CleanImportTarget);
    assert!(clean.gates[0].passed);
}

#[test]
fn structural_inspection_names_an_expectation_the_engine_did_not_meet() {
    let mut structures = attainable_structures();
    structures.as_array_mut().unwrap().push(json!({
        "structureType": "concept",
        "identityPolicy": "semantic-properties-only",
        "semanticProperties": ["grapheme cluster segmentation", "normalization provenance"],
        "compositionRole": "Nothing in this curriculum teaches it.",
        "evidenceExpectation": {
            "teacherOff": true,
            "replayable": true,
            "localValidation": true
        }
    }));
    let (report, _) = run(structures);

    let missing: Vec<_> = report
        .structures
        .iter()
        .filter(|finding| finding.status != StructureStatus::Matched)
        .collect();
    assert_eq!(missing.len(), 1, "{:#?}", report.structures);
    assert_eq!(missing[0].status, StructureStatus::Missing);
    assert!(
        missing[0]
            .unmatched_properties
            .contains(&"grapheme cluster segmentation".to_string()),
        "{:#?}",
        missing[0]
    );
    assert!(
        !report.passed,
        "an unmet structural expectation must fail the run"
    );
}

#[test]
fn the_export_filter_refuses_content_the_policy_excludes() {
    let curriculum = curriculum(attainable_structures());
    let policy = ExportPolicy::from_curriculum(&curriculum);

    policy
        .enforce(
            "clean",
            &json!({ "name": "DOUBLE", "target": "seed:forge-doubling/double" }),
        )
        .expect("a neutral seed carries nothing the policy excludes");

    for (label, document) in [
        (
            "teacher state",
            json!({ "teacherPrompt": "you are a helpful assistant" }),
        ),
        (
            "a secret",
            json!({ "header": "Bearer aaaaaaaaaaaaaaaaaaaaaaaa" }),
        ),
        (
            "a machine path",
            json!({ "fixture": "/Users/someone/spoon/store.db" }),
        ),
        (
            "an episode identifier",
            json!({ "receipt": "6f1f4b8a-6a1e-4f2e-9b39-1c2d3e4f5a6b" }),
        ),
    ] {
        let error = policy
            .enforce("seed", &document)
            .expect_err("the policy must refuse {label}");
        assert!(
            matches!(error, ForgeError::ExportRefused { .. }),
            "{label} produced {error}"
        );
    }
}

#[test]
fn the_report_round_trips_and_accepts_a_signature() {
    struct FixedSigner;
    impl ReportSigner for FixedSigner {
        fn sign(&self, payload: &[u8]) -> Result<Signature, ForgeError> {
            Ok(Signature {
                algorithm: "test-length".into(),
                key_id: "test".into(),
                value: payload.len().to_string(),
            })
        }
    }

    let (mut report, _) = run(attainable_structures());
    let encoded = serde_json::to_string(&report).unwrap();
    let decoded: ForgeReport = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.curriculum_id, report.curriculum_id);
    assert_eq!(decoded.passed, report.passed);
    assert_eq!(decoded.phases.len(), report.phases.len());
    assert_eq!(decoded.clean_import.steps.len(), 6);
    assert!(decoded.signature.is_none());
    assert_eq!(serde_json::to_string(&decoded).unwrap(), encoded);

    // Signing covers the unsigned report, so attaching twice is stable.
    report.attach_signature(&FixedSigner).unwrap();
    let signature = report.signature.clone().unwrap();
    report.attach_signature(&FixedSigner).unwrap();
    assert_eq!(report.signature, Some(signature));
}
