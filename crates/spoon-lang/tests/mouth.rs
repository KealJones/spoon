//! Integration tests for crates/spoon-lang/src/mouth/.

use spoon_core::llm::{LlmClient, LlmConfig};
use spoon_core::types::can::{ActionId, Effect};
use spoon_core::types::response::{AdviceOption, Move, ResponsePlan};
use spoon_core::types::value::{Type, Value};
use spoon_lang::mouth::{
    faithful::{self, ViolationKind},
    render::RenderContext,
    templates, Mouth,
};

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

fn all_move_plans() -> Vec<ResponsePlan> {
    vec![
        ResponsePlan::single(Move::Greet { returning: false }),
        ResponsePlan::single(Move::Greet { returning: true }),
        ResponsePlan::single(Move::Farewell),
        ResponsePlan::single(Move::Thanks),
        ResponsePlan::single(Move::Ack { summary: "you have 3 apples".into() }),
        ResponsePlan::single(Move::Empathize {
            feeling: "rough".into(),
            about: "losing a job".into(),
        }),
        ResponsePlan::single(Move::Reflect {
            summary: "you want to save the file first".into(),
        }),
        ResponsePlan::single(Move::Answer {
            question: "how many apples?".into(),
            values: vec![Value::Int(5)],
            source: None,
        }),
        ResponsePlan::single(Move::YesNo {
            question: "is it raining?".into(),
            answer: true,
            because: Some("clouds look heavy".into()),
        }),
        ResponsePlan::single(Move::YesNo {
            question: "are you sure?".into(),
            answer: false,
            because: None,
        }),
        ResponsePlan::single(Move::Result {
            action: ActionId("save_file".into()),
            value: Value::Text("saved.txt".into()),
            steps: 3,
        }),
        ResponsePlan::single(Move::Opinion {
            topic: "tabs vs spaces".into(),
            stance: "spaces are fine".into(),
            reasons: vec!["consistent width".into()],
            confidence: 0.8,
        }),
        ResponsePlan::single(Move::Advise {
            situation: "you want to learn Rust".into(),
            options: vec![AdviceOption {
                option: "read the book".into(),
                pros: vec!["comprehensive".into()],
                cons: vec!["slow start".into()],
            }],
            leaning: Some("read the book".into()),
        }),
        ResponsePlan::single(Move::Recall {
            topic: "Rust".into(),
            episodes: vec!["learning Rust last week".into()],
        }),
        ResponsePlan::single(Move::Explain { text: "this is how it works".into() }),
        ResponsePlan::single(Move::Info {
            text: "Rust was created in 2006".into(),
            source: "Wikipedia".into(),
        }),
        ResponsePlan::single(Move::Clarify {
            question: "which file?".into(),
            options: vec!["main.rs".into(), "lib.rs".into()],
            slot_type: None,
        }),
        ResponsePlan::single(Move::Elicit {
            input_name: "filename".into(),
            ty: Type::Text,
            for_action: ActionId("open_file".into()),
        }),
        ResponsePlan::single(Move::AskPermission {
            action: ActionId("delete_file".into()),
            effect: Effect::Write,
            description: "delete all log files".into(),
        }),
        ResponsePlan::single(Move::AskExamples {
            capability: "sort a list".into(),
            signature: "(List<Int>) -> List<Int>".into(),
        }),
        ResponsePlan::single(Move::UnknownWord {
            word: "florp".into(),
            guess: Some("a made-up word".into()),
        }),
        ResponsePlan::single(Move::Refuse { reason: "that would harm the system".into() }),
        ResponsePlan::single(Move::Learned { what: "your name is Alice".into() }),
        ResponsePlan::single(Move::CannotDo {
            what: "access the internet".into(),
            reason: "no network permission".into(),
        }),
        ResponsePlan::single(Move::Error { message: "division by zero".into() }),
    ]
}

// ---------------------------------------------------------------------------
// Template tests
// ---------------------------------------------------------------------------

#[test]
fn templates_all_moves_non_empty_no_emdash_must_mention() {
    for plan in all_move_plans() {
        let text = templates::realize(&plan);
        assert!(!text.is_empty(), "empty output for plan: {plan:?}");

        assert!(
            !text.contains('\u{2014}') && !text.contains('\u{2013}'),
            "em-dash in output '{text}' for plan: {plan:?}"
        );

        // must_mention values must appear
        let violations = faithful::check(&plan, &text);
        let missing: Vec<_> =
            violations.iter().filter(|v| v.kind == ViolationKind::MissingMention).collect();
        assert!(
            missing.is_empty(),
            "missing must_mention in '{text}' for plan {plan:?}: {missing:?}"
        );
    }
}

#[test]
fn templates_answer_int_42_contains_42() {
    let plan = ResponsePlan::single(Move::Answer {
        question: "what is the answer?".into(),
        values: vec![Value::Int(42)],
        source: None,
    });
    let text = templates::realize(&plan);
    assert!(text.contains("42"), "expected '42' in: {text}");
}

#[test]
fn templates_deterministic() {
    let plan = ResponsePlan::single(Move::Greet { returning: false });
    assert_eq!(templates::realize(&plan), templates::realize(&plan));

    let plan2 = ResponsePlan::single(Move::Answer {
        question: "how many?".into(),
        values: vec![Value::Int(99)],
        source: None,
    });
    assert_eq!(templates::realize(&plan2), templates::realize(&plan2));
}

// ---------------------------------------------------------------------------
// Faithfulness tests
// ---------------------------------------------------------------------------

#[test]
fn faithful_correct_passes() {
    let plan = ResponsePlan::single(Move::Answer {
        question: "how many?".into(),
        values: vec![Value::Int(7)],
        source: None,
    });
    let text = templates::realize(&plan);
    let violations = faithful::check(&plan, &text);
    assert!(violations.is_empty(), "template output should pass faithful check: {violations:?}");
}

#[test]
fn faithful_extra_name_paris() {
    // Plan mentions France (in values) but not Paris.
    let plan = ResponsePlan::single(Move::Answer {
        question: "where is the tower?".into(),
        values: vec![Value::Text("France".into())],
        source: None,
    });
    // "Paris" is mid-sentence and not in the plan.
    let text = "It is in Paris, France.";
    let violations = faithful::check(&plan, text);
    let has = violations.iter().any(|v| v.kind == ViolationKind::ExtraName && v.detail.contains("Paris"));
    assert!(has, "expected ExtraName for 'Paris', got: {violations:?}");
}

#[test]
fn faithful_extra_number_17() {
    let plan = ResponsePlan::single(Move::Answer {
        question: "how old?".into(),
        values: vec![Value::Int(4)],
        source: None,
    });
    // "17" is not in the plan.
    let text = "you are 17 years old.";
    let violations = faithful::check(&plan, text);
    let has = violations.iter().any(|v| v.kind == ViolationKind::ExtraNumber);
    assert!(has, "expected ExtraNumber, got: {violations:?}");
}

#[test]
fn faithful_missing_mention() {
    // YesNo answer:true -> must_mention = ["yes"]
    let plan = ResponsePlan::single(Move::YesNo {
        question: "is it done?".into(),
        answer: true,
        because: None,
    });
    // Does not contain "yes" - should fail must_mention check.
    let text = "that's correct.";
    let violations = faithful::check(&plan, text);
    let has = violations.iter().any(|v| v.kind == ViolationKind::MissingMention);
    assert!(has, "expected MissingMention, got: {violations:?}");
}

#[test]
fn faithful_em_dash_flagged() {
    let plan = ResponsePlan::single(Move::Explain { text: "just because".into() });
    let text = "just because\u{2014}that's why.";
    let violations = faithful::check(&plan, text);
    let has = violations.iter().any(|v| v.kind == ViolationKind::EmDash);
    assert!(has, "expected EmDash violation, got: {violations:?}");
}

#[test]
fn faithful_certainly_forbidden() {
    let plan = ResponsePlan::single(Move::Answer {
        question: "can you help?".into(),
        values: vec![Value::Bool(true)],
        source: None,
    });
    let text = "Certainly! Yes.";
    let violations = faithful::check(&plan, text);
    let has = violations.iter().any(|v| v.kind == ViolationKind::ForbiddenPhrase);
    assert!(has, "expected ForbiddenPhrase for 'Certainly!', got: {violations:?}");
}

// ---------------------------------------------------------------------------
// LLM render test (skipped unless SPOON_LLM_TESTS=1 and Ollama is up)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn render_llm_greet_and_answer() {
    if std::env::var("SPOON_LLM_TESTS").as_deref() != Ok("1") {
        println!("skipping LLM render test (set SPOON_LLM_TESTS=1 to enable)");
        return;
    }

    let cfg = LlmConfig::ollama("qwen3.5:4b");
    let client = LlmClient::new();
    if !client.ping(&cfg).await {
        println!("skipping LLM render test (Ollama not reachable at localhost:11434)");
        return;
    }

    let plan = ResponsePlan::new(vec![
        Move::Greet { returning: false },
        Move::Answer {
            question: "How many apples does Ben own?".into(),
            values: vec![Value::Int(10)],
            source: None,
        },
    ]);

    let ctx = RenderContext {
        user_text: "How many apples does Ben own?".into(),
        prior_turns: vec![],
    };

    let mouth = Mouth { cfg: Some(cfg), client };
    let (text, path) = mouth.say(&plan, &ctx).await;

    println!("render path={path:?}  text={text:?}");

    assert!(text.contains("10"), "reply must contain '10', got: {text}");

    let violations = faithful::check(&plan, &text);
    assert!(violations.is_empty(), "unexpected violations after render: {violations:?}");
}
