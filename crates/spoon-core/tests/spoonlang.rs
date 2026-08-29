use serde_json::{Value as JsonValue, json};
use spoon_core::spoonlang::{SpoonlangKind, parse_expr, parse_proposal};

fn expr(src: &str) -> JsonValue {
    parse_expr(src).unwrap_or_else(|error| panic!("{src:?}: {error}"))
}

fn param(name: &str) -> JsonValue {
    json!({ "kind": "parameter", "name": name })
}

fn lit(value: JsonValue) -> JsonValue {
    json!({ "kind": "literal", "value": value })
}

fn binary(op: &str, left: JsonValue, right: JsonValue) -> JsonValue {
    json!({ "kind": "binary", "op": op, "left": left, "right": right })
}

#[test]
fn percent_of_compiles_to_multiply_then_divide() {
    assert_eq!(
        expr("(percent * of) / 100"),
        binary(
            "divide",
            binary("multiply", param("percent"), param("of")),
            lit(json!(100)),
        )
    );
}

#[test]
fn if_let_field_index_and_intrinsics_parse() {
    assert_eq!(
        expr("if n > 0 then text_trim(name) else \"\""),
        json!({
            "kind": "if",
            "condition": binary("greater_than", param("n"), lit(json!(0))),
            "then": {
                "kind": "intrinsic",
                "version": 1,
                "op": "text_trim",
                "args": [param("name")],
            },
            "else": lit(json!("")),
        })
    );
    assert_eq!(
        expr("let n = items[0].count in n + 1"),
        json!({
            "kind": "let",
            "name": "n",
            "value": {
                "kind": "field",
                "object": {
                    "kind": "index",
                    "collection": param("items"),
                    "index": lit(json!(0)),
                },
                "field": "count",
            },
            "body": binary("add", param("n"), lit(json!(1))),
        })
    );
}

#[test]
fn binders_cap_dep_and_map_literals_parse() {
    assert_eq!(
        expr("map xs x => x * 2"),
        json!({
            "kind": "map",
            "collection": param("xs"),
            "var": "x",
            "body": binary("multiply", param("x"), lit(json!(2))),
        })
    );
    assert_eq!(
        expr(r#"cap("spoon.native", "web.fetch", { url: url })"#),
        json!({
            "kind": "capability_call",
            "contentId": "spoon.native",
            "procedureId": "web.fetch",
            "input": {
                "kind": "intrinsic",
                "version": 1,
                "op": "map_set",
                "args": [lit(json!({})), lit(json!("url")), param("url")],
            },
        })
    );
    assert_eq!(
        expr(r#"dep("lesson:tax", price, rate)"#),
        json!({
            "kind": "dependency",
            "alias": "lesson:tax",
            "args": [param("price"), param("rate")],
        })
    );
}

#[test]
fn reusable_percent_lesson_emits_pure_expr_v2_draft() {
    let parsed = parse_proposal(
        r#"
kind reusable_lesson
concept percent: defeasible_general
  "A proportion of a quantity, expressed as parts per hundred"
proc percent_of(percent: number, of: number)
  name "PERCENT OF"
  (percent * of) / 100
example percent_of(50, 100) => 50
"#,
    )
    .unwrap();

    assert_eq!(parsed.kind, SpoonlangKind::ReusableLesson);
    assert_eq!(parsed.answer, Some(json!(50)));
    let lesson = parsed
        .lesson
        .expect("reusable lesson must include lesson JSON");
    assert_eq!(lesson["primitiveSet"], "pure_expr_v2");
    assert_eq!(lesson["concepts"][0]["key"], "percent");
    assert_eq!(lesson["concepts"][0]["mutability"], "defeasible_general");
    assert_eq!(lesson["procedures"][0]["key"], "percent_of");
    assert_eq!(lesson["procedures"][0]["name"], "PERCENT OF");
    assert_eq!(
        lesson["procedures"][0]["concept"],
        json!({ "kind": "new_concept", "key": "percent" })
    );
    assert_eq!(
        lesson["procedures"][0]["body"],
        binary(
            "divide",
            binary("multiply", param("percent"), param("of")),
            lit(json!(100)),
        )
    );
    assert_eq!(lesson["invocation"]["procedureKey"], "percent_of");
    assert_eq!(
        lesson["invocation"]["inputs"],
        json!([
            { "name": "percent", "value": 50 },
            { "name": "of", "value": 100 }
        ])
    );
}

#[test]
fn live_env_source_parses_if_present() {
    let Ok(source) = std::env::var("SPOONLANG_LIVE") else {
        return;
    };
    if source.trim().is_empty() {
        return;
    }
    let parsed = parse_proposal(&source).unwrap_or_else(|error| panic!("{error}\n{source}"));
    assert_eq!(parsed.kind, SpoonlangKind::ReusableLesson);
    assert!(parsed.lesson.is_some());
}

#[test]
fn answer_only_and_abstain_parse() {
    let answer = parse_proposal("kind answer_only\nanswer 42\n").unwrap();
    assert_eq!(answer.kind, SpoonlangKind::AnswerOnly);
    assert_eq!(answer.answer, Some(json!(42)));
    assert!(answer.lesson.is_none());

    let abstain = parse_proposal("kind abstain\nreason \"unknown fact\"\n").unwrap();
    assert_eq!(abstain.kind, SpoonlangKind::Abstain);
    assert_eq!(abstain.abstain_reason.as_deref(), Some("unknown fact"));
}
