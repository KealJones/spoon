//! The checked-in seed manifests must load through the same loader a run uses,
//! and a manifest that lies about its shape must fail loudly.

use std::path::PathBuf;

use spoon_forge::Curriculum;
use spoon_forge::curriculum::{CurriculumKind, TeacherMode};

fn seeds() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("seeds")
}

fn manifest(name: &str) -> String {
    std::fs::read_to_string(seeds().join(name)).expect("seed manifest is readable")
}

const CHECKED_IN: [&str; 3] = [
    "language-kernel-intent.json",
    "structured-data-transforms.json",
    "programming-foundations.json",
];

#[test]
fn every_checked_in_manifest_loads_and_validates() {
    for name in CHECKED_IN {
        let curriculum = Curriculum::from_path(seeds().join(name))
            .unwrap_or_else(|error| panic!("{name} should load: {error}"));
        assert_eq!(curriculum.kind, CurriculumKind::SeedCurriculum);
        assert_eq!(curriculum.schema_version, 1);
        assert!(curriculum.teacher_off_gates.len() >= 2, "{name}");
        assert!(!curriculum.expected_learned_structures.is_empty(), "{name}");
        assert!(
            curriculum
                .held_out_generalization
                .iter()
                .all(|activity| activity.teacher_mode == TeacherMode::Off),
            "{name} must hold out its generalization probes from the Teacher"
        );
    }
}

#[test]
fn manifest_ids_are_distinct_and_match_their_file_names() {
    let ids: Vec<String> = CHECKED_IN
        .iter()
        .map(|name| Curriculum::from_path(seeds().join(name)).unwrap().id)
        .collect();
    let expected: Vec<&str> = CHECKED_IN
        .iter()
        .map(|name| name.trim_end_matches(".json"))
        .collect();
    assert_eq!(ids, expected);
}

#[test]
fn a_missing_required_field_fails_with_a_clear_error() {
    let mut document: serde_json::Value =
        serde_json::from_str(&manifest("language-kernel-intent.json")).unwrap();
    document
        .as_object_mut()
        .unwrap()
        .remove("teacherOffGates")
        .expect("the fixture must actually have the field being removed");

    let error = Curriculum::from_json_str(&document.to_string())
        .expect_err("a manifest without teacher-off gates must not load")
        .to_string();
    assert!(
        error.contains("teacherOffGates"),
        "the error should name the missing field, got: {error}"
    );
}

#[test]
fn an_unknown_kind_is_rejected() {
    let mut document: serde_json::Value =
        serde_json::from_str(&manifest("programming-foundations.json")).unwrap();
    document["kind"] = serde_json::json!("spoon-lesson-plan");

    let error = Curriculum::from_json_str(&document.to_string())
        .expect_err("an unknown kind must not load")
        .to_string();
    assert!(
        error.contains("spoon-lesson-plan") && error.contains("spoon-seed-curriculum"),
        "the error should name both the bad kind and the expected one, got: {error}"
    );
}

#[test]
fn cardinality_floors_from_the_schema_are_enforced() {
    let mut document: serde_json::Value =
        serde_json::from_str(&manifest("structured-data-transforms.json")).unwrap();
    document["teacherOffGates"] = serde_json::json!([document["teacherOffGates"][0]]);

    let error = Curriculum::from_json_str(&document.to_string())
        .expect_err("one teacher-off gate is below the schema floor of two")
        .to_string();
    assert!(error.contains("at least 2"), "got: {error}");
}

#[test]
fn a_promotion_gate_that_transfers_authority_is_rejected() {
    let mut document: serde_json::Value =
        serde_json::from_str(&manifest("language-kernel-intent.json")).unwrap();
    document["independentCleanImportValidation"]["promotionGate"]["authorityTransferred"] =
        serde_json::json!(true);

    let error = Curriculum::from_json_str(&document.to_string())
        .expect_err("installation must never transfer authority")
        .to_string();
    assert!(error.contains("authorityTransferred"), "got: {error}");
}
