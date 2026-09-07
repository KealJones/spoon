//! Offline and live tests for the teacher module.
//!
//! Run offline tests: `cargo test -p spoon-mind --test teacher`
//! Run live tests:    `SPOON_LLM_TESTS=1 cargo test -p spoon-mind --test teacher -- --ignored`

use spoon_core::{ConceptId, ConceptKind, Type};
use spoon_mind::teacher::{
    parse_concept_json, parse_lessons_json, parse_spec_json, parse_stance_json, parse_type, Lesson,
    Teacher,
};

// ---------------------------------------------------------------------------
// 1. parse_spec_good
// ---------------------------------------------------------------------------

const SPEC_GOOD: &str = r#"{
  "name_hint": "double",
  "description": "Multiply an integer by two",
  "params": ["Int"],
  "param_names": ["n"],
  "ret": "Int",
  "verbs": ["double", "twice"],
  "phrasings": ["double it", "4 twice"],
  "examples": [
    {"inputs": [1], "output": 2},
    {"inputs": [2], "output": 4},
    {"inputs": [3], "output": 6},
    {"inputs": [4], "output": 8}
  ]
}"#;

#[test]
fn parse_spec_good() {
    let spec = parse_spec_json(SPEC_GOOD, "test:double").expect("should parse");
    assert_eq!(spec.name_hint, "double");
    assert_eq!(spec.source, "teacher");
    assert_eq!(spec.id, "test:double");
    assert_eq!(spec.examples.len(), 4);
    spec.validate().expect("should validate");
}

// ---------------------------------------------------------------------------
// 2. parse_spec_rejects_code
// ---------------------------------------------------------------------------

const SPEC_WITH_PROGRAM: &str = r#"{
  "name_hint": "double",
  "program": "fn double(x) { x * 2 }",
  "params": ["Int"],
  "param_names": ["n"],
  "ret": "Int",
  "examples": [
    {"inputs": [1], "output": 2},
    {"inputs": [2], "output": 4},
    {"inputs": [3], "output": 6},
    {"inputs": [4], "output": 8}
  ]
}"#;

#[test]
fn parse_spec_rejects_code() {
    let err = parse_spec_json(SPEC_WITH_PROGRAM, "test:double")
        .expect_err("should be rejected");
    assert!(err.contains("must not write code"), "got: {err}");
}

// ---------------------------------------------------------------------------
// 3. parse_spec_rejects_type_mismatch
// ---------------------------------------------------------------------------

const SPEC_TYPE_MISMATCH: &str = r#"{
  "name_hint": "double",
  "description": "Multiply an integer by two",
  "params": ["Int"],
  "param_names": ["n"],
  "ret": "Int",
  "examples": [
    {"inputs": [1], "output": 2},
    {"inputs": [2], "output": 4},
    {"inputs": [3], "output": 6},
    {"inputs": [4], "output": "four"}
  ]
}"#;

#[test]
fn parse_spec_rejects_type_mismatch() {
    let err = parse_spec_json(SPEC_TYPE_MISMATCH, "test:double")
        .expect_err("should be rejected");
    assert!(
        err.contains("expected Int") || err.contains("output"),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 4. parse_type table
// ---------------------------------------------------------------------------

#[test]
fn parse_type_table() {
    assert_eq!(parse_type("Int"), Ok(Type::Int));
    assert_eq!(
        parse_type("List<Text>"),
        Ok(Type::List(Box::new(Type::Text)))
    );
    assert_eq!(
        parse_type("List<List<Int>>"),
        Ok(Type::List(Box::new(Type::List(Box::new(Type::Int)))))
    );
    assert_eq!(
        parse_type("Person"),
        Ok(Type::Concept(ConceptId("Person".to_string())))
    );
    assert!(
        parse_type("Bogus<").is_err(),
        "malformed generic should error"
    );
}

// ---------------------------------------------------------------------------
// 5. parse_stance_good and parse_stance_bad_confidence
// ---------------------------------------------------------------------------

const STANCE_GOOD: &str = r#"{
  "topic": "coffee drinking",
  "stance": "Coffee is best enjoyed in the morning before work",
  "reasons": ["It provides an energy boost", "Routine improves focus"],
  "counterpoints": ["Caffeine can cause anxiety in some people"],
  "confidence": 0.8
}"#;

const STANCE_BAD_CONFIDENCE: &str = r#"{
  "topic": "coffee drinking",
  "stance": "Coffee is best enjoyed in the morning",
  "reasons": ["Energy boost", "Better focus"],
  "counterpoints": ["Causes anxiety"],
  "confidence": 1.7
}"#;

#[test]
fn parse_stance_good() {
    let s = parse_stance_json(STANCE_GOOD).expect("should parse");
    assert_eq!(s.topic, "coffee drinking");
    assert_eq!(s.reasons.len(), 2);
    assert!((s.confidence - 0.8_f32).abs() < 0.01);
}

#[test]
fn parse_stance_bad_confidence() {
    let err = parse_stance_json(STANCE_BAD_CONFIDENCE).expect_err("should reject");
    assert!(err.contains("out of range") || err.contains("1.7"), "got: {err}");
}

// ---------------------------------------------------------------------------
// 6. parse_concept_structure (entity + structure)
// ---------------------------------------------------------------------------

const CONCEPT_DOG: &str = r#"{
  "id": "Dog",
  "kind": "entity",
  "extends": ["Animal"],
  "nouns": ["dog", "pup"],
  "description": "A domestic canine animal",
  "properties": []
}"#;

const CONCEPT_RECIPE: &str = r#"{
  "id": "Recipe",
  "kind": "structure",
  "extends": [],
  "nouns": ["recipe"],
  "description": "A set of cooking instructions",
  "properties": [
    {"name": "name", "type": "Text"},
    {"name": "minutes", "type": "Int"}
  ]
}"#;

#[test]
fn parse_concept_structure() {
    let dog = parse_concept_json(CONCEPT_DOG).expect("dog should parse");
    assert_eq!(dog.id, ConceptId("Dog".to_string()));
    assert_eq!(dog.kind, ConceptKind::Entity);
    assert_eq!(dog.extends, vec![ConceptId("Animal".to_string())]);

    let recipe = parse_concept_json(CONCEPT_RECIPE).expect("recipe should parse");
    assert_eq!(recipe.id, ConceptId("Recipe".to_string()));
    if let ConceptKind::Structure { properties } = &recipe.kind {
        assert_eq!(properties.len(), 2);
        assert_eq!(properties[0].name, "name");
        assert_eq!(properties[0].ty, Type::Text);
        assert_eq!(properties[1].name, "minutes");
        assert_eq!(properties[1].ty, Type::Int);
    } else {
        panic!("expected Structure kind");
    }
}

// ---------------------------------------------------------------------------
// 7. parse_lessons_mixed (5 distinct + 1 duplicate = 5 after dedup)
// ---------------------------------------------------------------------------

const LESSONS_MIXED: &str = r#"{
  "lessons": [
    {"kind": "capability",  "description": "count words in text",          "signature_hint": "word_count(Text) -> Int"},
    {"kind": "facts",       "sce": ["water is wet", "fire is hot"]},
    {"kind": "phrasings",   "sce": "Assistant double N",                   "verb": "double"},
    {"kind": "opinion",     "topic": "is coffee healthy"},
    {"kind": "concept",     "noun": "recipe"},
    {"kind": "capability",  "description": "count words in text",          "signature_hint": "word_count(Text) -> Int"}
  ]
}"#;

#[test]
fn parse_lessons_mixed() {
    let lessons = parse_lessons_json(LESSONS_MIXED).expect("should parse");
    assert_eq!(lessons.len(), 5, "duplicate should be removed; got {:?}", lessons);

    let has_capability = lessons.iter().any(|l| matches!(l, Lesson::Capability { .. }));
    let has_facts = lessons.iter().any(|l| matches!(l, Lesson::Facts { .. }));
    let has_phrasings = lessons.iter().any(|l| matches!(l, Lesson::Phrasings { .. }));
    let has_opinion = lessons.iter().any(|l| matches!(l, Lesson::Opinion { .. }));
    let has_concept = lessons.iter().any(|l| matches!(l, Lesson::Concept { .. }));
    assert!(has_capability && has_facts && has_phrasings && has_opinion && has_concept);
}

const LESSONS_SLOPPY: &str = r#"{
  "lessons": [
    {"kind": "phrasings",   "sce": "Assistant, reverse \"abc\"!"},
    {"kind": "phrasings",   "sce": "What is the length of \"abc\"?"},
    {"kind": "capability",  "signature_hint": "word_count(Text) -> Int"},
    {"kind": "phrasings",   "sce": "hello there"},
    {"kind": "opinion",     "topic": "remote work"}
  ]
}"#;

/// Small models drop fields: verbs are recovered from the SCE, descriptions
/// from the signature hint, and what cannot be recovered is dropped rather
/// than failing the whole curriculum.
#[test]
fn parse_lessons_recovers_or_drops_sloppy_entries() {
    let lessons = parse_lessons_json(LESSONS_SLOPPY).expect("usable lessons remain");
    assert_eq!(
        lessons,
        vec![
            Lesson::Phrasings { sce: "Assistant, reverse \"abc\"!".into(), verb: "reverse".into() },
            Lesson::Phrasings { sce: "What is the length of \"abc\"?".into(), verb: "length".into() },
            Lesson::Capability { description: "word count".into(), signature_hint: "word_count(Text) -> Int".into() },
            Lesson::Opinion { topic: "remote work".into() },
        ]
    );
    assert_eq!(lessons[3].key(), "opinion:remote work");
    assert_eq!(lessons[3].kind(), "opinion");

    let err = parse_lessons_json(r#"{"lessons": [{"kind": "phrasings", "sce": "hello there"}]}"#).expect_err("no usable lesson");
    assert!(err.contains("missing 'verb'"), "got: {err}");
}

// ---------------------------------------------------------------------------
// 8. code_fence_stripped
// ---------------------------------------------------------------------------

const FENCED_STANCE: &str = r#"```json
{"topic":"dessert","stance":"Dessert before dinner is perfectly fine","reasons":["Life is short","It does not spoil appetite much"],"counterpoints":["May reduce appetite for main course"],"confidence":0.7}
```"#;

#[test]
fn code_fence_stripped() {
    let result = parse_stance_json(FENCED_STANCE);
    assert!(result.is_ok(), "fenced JSON should parse; got: {:?}", result);
}

// ---------------------------------------------------------------------------
// 9. prompts contain "JSON", "no code", no em-dash
// ---------------------------------------------------------------------------

#[test]
fn prompts_have_no_em_dash_and_forbid_code() {
    let prompts: &[(&str, &str)] = &[
        ("spec",       include_str!("../../../data/prompts/teacher_spec.md")),
        ("phrasings",  include_str!("../../../data/prompts/teacher_phrasings.md")),
        ("concept",    include_str!("../../../data/prompts/teacher_concept.md")),
        ("stance",     include_str!("../../../data/prompts/teacher_stance.md")),
        ("curriculum", include_str!("../../../data/prompts/teacher_curriculum.md")),
    ];
    for (name, text) in prompts {
        let lower = text.to_lowercase();
        assert!(
            lower.contains("json"),
            "prompt '{name}' must mention JSON"
        );
        assert!(
            lower.contains("no code"),
            "prompt '{name}' must say 'no code'"
        );
        assert!(
            !text.contains('\u{2014}'),
            "prompt '{name}' must not contain an em-dash"
        );
    }
}

// ---------------------------------------------------------------------------
// Live tests (ignored by default; run with SPOON_LLM_TESTS=1 -- --ignored)
// ---------------------------------------------------------------------------

use spoon_core::{LlmClient, LlmConfig};

#[tokio::test]
#[ignore]
async fn live_spec_double() {
    if std::env::var("SPOON_LLM_TESTS").unwrap_or_default() != "1" {
        return;
    }
    let teacher = Teacher::new(LlmClient::new(), LlmConfig::ollama("qwen3.5:4b"));
    let spec = teacher
        .spec_for("double", "The user said: double 4", &[])
        .await
        .expect("live spec_for failed");
    println!("live spec:\n{}", serde_json::to_string_pretty(&spec).unwrap());
    spec.validate().expect("live spec did not validate");
}

#[tokio::test]
#[ignore]
async fn live_stance() {
    if std::env::var("SPOON_LLM_TESTS").unwrap_or_default() != "1" {
        return;
    }
    let teacher = Teacher::new(LlmClient::new(), LlmConfig::ollama("qwen3.5:4b"));
    let stance = teacher
        .stance_for("is it ok to eat dessert before dinner", "")
        .await
        .expect("live stance_for failed");
    println!("live stance: {stance:?}");
    assert!((0.0..=1.0).contains(&stance.confidence));
}

#[tokio::test]
#[ignore]
async fn live_curriculum() {
    if std::env::var("SPOON_LLM_TESTS").unwrap_or_default() != "1" {
        return;
    }
    let teacher = Teacher::new(LlmClient::new(), LlmConfig::ollama("qwen3.5:4b"));
    let lessons = teacher
        .curriculum(5, &[], &["math".to_string(), "cooking".to_string()])
        .await
        .expect("live curriculum failed");
    for l in &lessons {
        println!("  lesson: {l:?}");
    }
    assert!(!lessons.is_empty());
}
