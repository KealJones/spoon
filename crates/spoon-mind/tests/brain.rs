//! Integration tests for the Brain turn loop.
//! All tests: offline, in-memory, no network. interior_llm_calls == 0.
//! (`teacher_live` is the one exception: `#[ignore]`, opt in with SPOON_LLM_TESTS=1.)

use std::path::PathBuf;
use std::sync::Arc;

use spoon_core::kernel::{Host, PermissionMode};
use spoon_core::types::*;
use spoon_mind::brain::{Brain, BrainConfig, BrainHost};

async fn test_brain() -> Arc<Brain> {
    Brain::open(BrainConfig {
        db_path: None,
        offline: true,
        debug: true,
        ..Default::default()
    })
    .await
    .expect("Brain::open")
}

async fn file_brain(db_path: PathBuf) -> Arc<Brain> {
    Brain::open(BrainConfig { db_path: Some(db_path), offline: true, debug: true, ..Default::default() })
        .await
        .expect("Brain::open")
}

/// One turn; asserts the interior stayed LLM-free and returns the reply text.
async fn say(brain: &Brain, text: &str) -> String {
    let r = brain.turn("test", text).await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0, "interior LLM call on {text:?}");
    r.text
}

#[tokio::test]
async fn greets() {
    let brain = test_brain().await;
    let r = brain.turn("test", "Hello!").await.unwrap();
    assert!(!r.text.is_empty(), "reply must not be empty");
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
}

#[tokio::test]
async fn facts_and_question() {
    let brain = test_brain().await;
    let r1 = brain.turn("test", "John owns a dog.").await.unwrap();
    assert_eq!(r1.episode.metrics.interior_llm_calls, 0);

    let r2 = brain.turn("test", "Who owns a dog?").await.unwrap();
    assert_eq!(r2.episode.metrics.interior_llm_calls, 0);
    let lower = r2.text.to_lowercase();
    assert!(lower.contains("john"), "reply should mention John, got: {}", r2.text);
}

#[tokio::test]
async fn property_fact() {
    let brain = test_brain().await;
    // Self-model seed already asserts "The name of Assistant is Spoon."
    let r = brain.turn("test", "What is the name of Assistant?").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    let lower = r.text.to_lowercase();
    assert!(lower.contains("spoon"), "reply should contain Spoon, got: {}", r.text);
}

#[tokio::test]
async fn arithmetic() {
    let brain = test_brain().await;
    let r = brain.turn("test", "Assistant, calculate 3 / 500 * 3600!").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert!(r.text.contains("21.6"), "reply should contain 21.6, got: {}", r.text);
}

#[tokio::test]
async fn learns_double_from_examples() {
    let brain = test_brain().await;

    // Ask to double - should ask for examples
    let r1 = brain.turn("test", "Assistant, double 21!").await.unwrap();
    assert_eq!(r1.episode.metrics.interior_llm_calls, 0);

    // Provide first example
    let r2 = brain.turn("test", "The double of 3 is 6.").await.unwrap();
    assert_eq!(r2.episode.metrics.interior_llm_calls, 0);

    // Provide second example - should learn and return 42
    let r3 = brain.turn("test", "The double of 5 is 10.").await.unwrap();
    assert_eq!(r3.episode.metrics.interior_llm_calls, 0);
    assert!(r3.text.contains("42"), "should compute 42 after learning, got: {}", r3.text);

    // Verify learned action exists in snapshot
    let snap = brain.snapshot();
    let has_double = snap.learned_actions.iter().any(|a| a.verbs.contains(&"double".to_string()));
    assert!(has_double, "snapshot should list a learned action with verb 'double'");

    // Use the learned action directly
    let r4 = brain.turn("test", "Assistant, double 100!").await.unwrap();
    assert_eq!(r4.episode.metrics.interior_llm_calls, 0);
    assert!(r4.text.contains("200"), "should compute 200, got: {}", r4.text);

    // Check synthesis metrics
    let m = brain.metrics();
    assert!(m.synthesis_succeeded >= 1, "synthesis_succeeded should be >= 1");
}

#[tokio::test]
async fn learned_action_survives_restart() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_path_buf();

    // Phase 1: learn
    {
        let brain = Brain::open(BrainConfig {
            db_path: Some(db_path.clone()),
            offline: true,
            debug: true,
            ..Default::default()
        })
        .await
        .unwrap();

        brain.turn("test", "Assistant, double 21!").await.unwrap();
        brain.turn("test", "The double of 3 is 6.").await.unwrap();
        let r = brain.turn("test", "The double of 5 is 10.").await.unwrap();
        assert!(r.text.contains("42"), "should learn and compute 42, got: {}", r.text);
    }

    // Phase 2: reopen and use the learned action
    {
        let brain = Brain::open(BrainConfig {
            db_path: Some(db_path),
            offline: true,
            debug: true,
            ..Default::default()
        })
        .await
        .unwrap();

        let r = brain.turn("test", "Assistant, double 7!").await.unwrap();
        assert_eq!(r.episode.metrics.interior_llm_calls, 0);
        assert!(r.text.contains("14"), "should compute 14 from persisted action, got: {}", r.text);

        let m = brain.metrics();
        assert_eq!(m.synthesis_succeeded, 0, "no synthesis this time");
    }
}

#[tokio::test]
async fn ears_failure_is_graceful() {
    let brain = test_brain().await;
    let r = brain.turn("test", "asdf qwerty zzz").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert!(!r.text.is_empty(), "should reply even on garbage input");
}

#[tokio::test]
async fn opinion_question() {
    let brain = test_brain().await;
    let r = brain.turn("test", "What do you think about dogs?").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert!(!r.text.is_empty(), "should reply to opinion question, got: {}", r.text);
}

// ---------------------------------------------------------------------------
// Honest unknowns
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_questions_are_honest() {
    let brain = test_brain().await;
    say(&brain, "John owns a dog.").await;

    let r = brain.turn("test", "Is John happy?").await.unwrap();
    assert_eq!(r.text, "I don't know whether John is happy.");
    assert!(
        matches!(r.episode.response.moves.as_slice(), [Move::Explain { .. }]),
        "one Explain move, got {:?}",
        r.episode.response.moves
    );

    assert_eq!(say(&brain, "Who owns a cat?").await, "I don't know who owns a cat.");
    assert_eq!(say(&brain, "What is the color of the dog?").await, "I don't know the color of the dog.");
    // A dog is not a cat: the open referent constrains the match.
    let r = say(&brain, "Does John own a cat?").await;
    assert!(r.starts_with("I don't know whether John owns a cat"), "got: {r}");
    // And the real fact still answers.
    assert!(say(&brain, "Who owns a dog?").await.contains("John"));
}

// ---------------------------------------------------------------------------
// World state: locations, carrying, and questions over them (babi families)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn location_follows_the_last_move() {
    let brain = test_brain().await;
    say(&brain, "Mary moves to the bathroom.").await;
    say(&brain, "John moves to the hallway.").await;
    say(&brain, "Mary moves to the kitchen.").await;
    let r = say(&brain, "Where is Mary?").await;
    assert!(r.starts_with("the kitchen"), "current location, got: {r}");
    assert!(r.contains("source: memory"), "memory answers say so, got: {r}");
    assert!(say(&brain, "Where is John?").await.starts_with("the hallway"));
}

#[tokio::test]
async fn carried_objects_travel_with_their_carrier() {
    let brain = test_brain().await;
    say(&brain, "John moves to the garden.").await;
    say(&brain, "John picks-up the apple.").await;
    say(&brain, "John moves to the kitchen.").await;
    let r = say(&brain, "Where is the apple?").await;
    assert!(r.starts_with("the kitchen"), "carried object is where its carrier is, got: {r}");
}

#[tokio::test]
async fn dropped_objects_stay_where_they_were_dropped() {
    let brain = test_brain().await;
    say(&brain, "Mary picks-up the milk.").await;
    say(&brain, "Mary moves to the hallway.").await;
    say(&brain, "Mary drops the milk.").await;
    say(&brain, "Mary moves to the office.").await;
    assert!(say(&brain, "Where is the milk?").await.starts_with("the hallway"));
    assert!(say(&brain, "Where is Mary?").await.starts_with("the office"));
}

#[tokio::test]
async fn counts_and_lists_what_is_carried() {
    let brain = test_brain().await;
    say(&brain, "Mary picks-up the apple.").await;
    say(&brain, "Mary picks-up the milk.").await;
    say(&brain, "Mary drops the apple.").await;
    let r = say(&brain, "How many objects does Mary carry?").await;
    assert!(r.starts_with("1."), "one object after the drop, got: {r}");

    say(&brain, "John picks-up the coin.").await;
    say(&brain, "John picks-up the key.").await;
    say(&brain, "John drops the key.").await;
    say(&brain, "John picks-up the key.").await;
    assert!(say(&brain, "How many objects does John carry?").await.starts_with("2."));
    let r = say(&brain, "What does John carry?").await;
    assert!(r.starts_with("the coin and the key"), "list from memory, got: {r}");
    assert!(r.contains("source: memory"), "got: {r}");
}

#[tokio::test]
async fn unknown_location_is_honest_and_well_worded() {
    let brain = test_brain().await;
    say(&brain, "John moves to the hallway.").await;
    assert_eq!(say(&brain, "Where is Mary?").await, "I don't know where Mary is.");
    assert_eq!(say(&brain, "Where is the apple?").await, "I don't know where the apple is.");
}

#[tokio::test]
async fn comparatives_chain_and_read_as_english() {
    let brain = test_brain().await;
    say(&brain, "The elephant is bigger than the dog.").await;
    say(&brain, "The dog is bigger than the cat.").await;
    let r = say(&brain, "Is the elephant bigger than the cat?").await;
    assert!(r.starts_with("yes"), "got: {r}");
    assert!(r.contains("the elephant is bigger than the cat"), "no entity ids, got: {r}");
    let r = say(&brain, "Is the cat bigger than the elephant?").await;
    assert!(r.starts_with("no"), "got: {r}");
}

#[tokio::test]
async fn names_learned_from_context_survive_restart() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_path_buf();
    {
        let brain = file_brain(db_path.clone()).await;
        let r = brain.turn("test", "Sandra moves to the garden.").await.unwrap();
        assert_eq!(r.episode.metrics.interior_llm_calls, 0);
        assert!(!r.text.contains("unknown"), "the name is learned in-turn, got: {}", r.text);
        assert!(say(&brain, "Where is Sandra?").await.starts_with("the garden"));
    }
    {
        let brain = file_brain(db_path).await;
        let r = say(&brain, "Where is Sandra?").await;
        assert!(r.starts_with("the garden"), "name and location must survive restart, got: {r}");
        say(&brain, "Sandra moves to the office.").await;
        assert!(say(&brain, "Where is Sandra?").await.starts_with("the office"));
    }
}

// ---------------------------------------------------------------------------
// Unknown capability: one move, SCE example, readable learned reply
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_capability_asks_with_one_sce_example() {
    let brain = test_brain().await;
    let r = brain.turn("test", "Assistant, double 21!").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert_eq!(r.episode.response.moves.len(), 1, "one move, got {:?}", r.episode.response.moves);
    assert_eq!(r.text, "I can't double yet. Teach me with examples like 'The double of 3 is 6.'");

    let r = say(&brain, "The double of 3 is 6.").await;
    assert_eq!(r, "One more example like 'The double of 3 is 6.' and I can build double.");

    let r = say(&brain, "The double of 5 is 10.").await;
    assert!(r.starts_with("42"), "result first, got: {r}");
    assert!(r.contains("double = add(x, x)"), "readable program, got: {r}");
    assert!(!r.contains("learned: learned"), "no doubled prefix: {r}");
}

#[tokio::test]
async fn unknown_capability_example_uses_text_literal_for_text_args() {
    let brain = test_brain().await;
    let r = say(&brain, "Assistant, shout \"hello\"!").await;
    assert_eq!(r, "I can't shout yet. Teach me with examples like 'The shout of \"abc\" is \"cba\".'");
}

// ---------------------------------------------------------------------------
// Corrections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn synonym_grounds_and_survives_restart() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_path_buf();
    {
        let brain = file_brain(db_path.clone()).await;
        let r = say(&brain, "\"pup\" means \"dog\".").await;
        assert!(r.contains("\"pup\" means \"dog\""), "got: {r}");
        say(&brain, "John owns a pup.").await;
        assert!(say(&brain, "Who owns a dog?").await.contains("John"));
    }
    {
        let brain = file_brain(db_path).await;
        say(&brain, "Mary owns a pup.").await;
        let r = say(&brain, "Who owns a dog?").await;
        assert!(r.contains("Mary"), "synonym must survive restart, got: {r}");
    }
}

#[tokio::test]
async fn user_means_retargets_the_last_fact() {
    let brain = test_brain().await;
    say(&brain, "John owns a dog.").await;
    let r = say(&brain, "User means Mary.").await;
    assert!(r.contains("Mary owns a dog"), "got: {r}");

    let r = say(&brain, "Who owns a dog?").await;
    assert!(r.contains("Mary") && !r.contains("John"), "got: {r}");
    assert_eq!(say(&brain, "Does John own a dog?").await, "I don't know whether John owns a dog.");

    // Nothing to correct: ask instead of guessing.
    let fresh = test_brain().await;
    let r = say(&fresh, "User means Mary.").await;
    assert!(r.contains("what should I change"), "got: {r}");
}

// ---------------------------------------------------------------------------
// Teacher seat: learn_from_spec is the shared, offline-testable half
// ---------------------------------------------------------------------------

fn reverse_spec() -> Spec {
    Spec {
        id: "test:reverse".into(),
        name_hint: "reverse".into(),
        verbs: vec!["reverse".into()],
        phrasings: vec![],
        params: vec![Type::Text],
        param_names: vec!["text".into()],
        ret: Type::Text,
        examples: vec![
            Example { inputs: vec![Value::text("abc")], output: Value::text("cba") },
            Example { inputs: vec![Value::text("spoon")], output: Value::text("noops") },
        ],
        description: "reverse a string".into(),
        source: "test".into(),
    }
}

#[tokio::test]
async fn learn_from_spec_registers_a_usable_action() {
    let brain = test_brain().await;
    let outcome = brain.learn_from_spec("reverse", reverse_spec()).expect("synthesis");
    assert!(outcome.action.verbs.contains(&"reverse".to_string()));
    assert!(outcome.description.contains("reverse"), "got: {}", outcome.description);
    assert!(brain.snapshot().learned_actions.iter().any(|a| a.id == outcome.action.id.0));

    let r = say(&brain, "Assistant, reverse \"hello\"!").await;
    assert!(r.contains("olleh"), "got: {r}");

    let mut bad = reverse_spec();
    bad.examples.clear();
    assert!(brain.learn_from_spec("reverse", bad).is_err(), "invalid specs are rejected");
}

/// Live: the real teacher writes the spec for an unknown verb.
/// `SPOON_LLM_TESTS=1 cargo test -p spoon-mind --test brain teacher_live -- --ignored`
/// (`reverse` is a kernel verb and never reaches the teacher, so the unknown
/// verb here is `double`; `reverse "abc"` is checked on the kernel path.)
#[tokio::test]
#[ignore]
async fn teacher_live() {
    if std::env::var("SPOON_LLM_TESTS").is_err() {
        eprintln!("skipped: set SPOON_LLM_TESTS=1");
        return;
    }
    let brain = Brain::open(BrainConfig { db_path: None, offline: false, debug: true, ..Default::default() })
        .await
        .unwrap();
    let r = brain.turn("test", "Assistant, double 21!").await.unwrap();
    eprintln!("teacher reply: {}\ntrace: {:#?}", r.text, r.trace);
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert!(r.episode.metrics.teacher_fallback, "the teacher seat must have been used");
    assert!(r.episode.metrics.teacher_llm_calls >= 1);
    assert!(r.text.contains("42"), "expected 42, got: {}", r.text);

    let r = brain.turn("test", "Assistant, reverse \"abc\"!").await.unwrap();
    assert!(r.text.contains("cba"), "expected cba, got: {}", r.text);
}

// ---------------------------------------------------------------------------
// Executor placeholder: a missing input becomes a question, the answer resumes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_with_placeholder() {
    let brain = test_brain().await;
    let r = brain.turn("test", "Assistant, concatenate \"ab\"!").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert!(
        matches!(r.episode.response.moves.as_slice(), [Move::Elicit { input_name, .. }] if input_name == "b"),
        "expected Elicit for input b, got {:?}",
        r.episode.response.moves
    );
    assert!(r.text.contains('b'), "reply must name the missing input, got: {}", r.text);

    let r = say(&brain, "\"cd\"").await;
    assert!(r.contains("abcd"), "got: {r}");
}

// ---------------------------------------------------------------------------
// Property questions reach capabilities when memory has no fact
// ---------------------------------------------------------------------------

fn computed_answer(r: &spoon_mind::brain::TurnResult) -> Option<&Value> {
    match r.episode.response.moves.as_slice() {
        [Move::Answer { values, source: Some(src), .. }] if src == "computed" => values.first(),
        _ => None,
    }
}

#[tokio::test]
async fn property_question_runs_the_learned_action() {
    let brain = test_brain().await;
    say(&brain, "Assistant, double 21!").await;
    say(&brain, "The double of 3 is 6.").await;
    assert!(say(&brain, "The double of 5 is 10.").await.contains("42"));

    let r = brain.turn("test", "What is the double of 100?").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert!(r.text.contains("200"), "got: {}", r.text);
    assert_eq!(computed_answer(&r), Some(&Value::Int(200)), "moves: {:?}", r.episode.response.moves);
    assert!(r.episode.metrics.reused_learned_action, "metrics: {:?}", r.episode.metrics);
    assert_eq!(r.episode.metrics.plan_steps, 1);

    // Yes/no: compute and compare, with the computed value as the reason.
    let r = brain.turn("test", "Is the double of 3 6?").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert!(
        matches!(r.episode.response.moves.as_slice(), [Move::YesNo { answer: true, because: Some(b), .. }] if b == "the double of 3 is 6"),
        "got {:?}",
        r.episode.response.moves
    );
    assert!(r.text.starts_with("yes"), "got: {}", r.text);
    let r = say(&brain, "Is the double of 3 7?").await;
    assert!(r.starts_with("no"), "got: {r}");
    // Values never stated as facts compute too.
    assert!(say(&brain, "Is the double of 4 8?").await.starts_with("yes"));
    assert!(say(&brain, "Is the double of 4 9?").await.starts_with("no"));
}

#[tokio::test]
async fn property_question_runs_a_kernel_action() {
    let brain = test_brain().await;
    let r = brain.turn("test", "What is the reverse of \"abc\"?").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert_eq!(computed_answer(&r), Some(&Value::text("cba")), "moves: {:?}", r.episode.response.moves);
    assert!(!r.episode.metrics.reused_learned_action, "kernel actions are not learned");
    assert!(r.text.contains("cba"), "got: {}", r.text);

    let r = brain.turn("test", "What is the length of \"abc\"?").await.unwrap();
    assert_eq!(computed_answer(&r), Some(&Value::Int(3)), "moves: {:?}", r.episode.response.moves);

    // No capability named `color`: the unknown stays honest.
    assert_eq!(say(&brain, "What is the color of the dog?").await, "I don't know the color of the dog.");
    // A stored fact still wins over computing.
    say(&brain, "The reverse of \"xy\" is \"zz\".").await;
    let r = brain.turn("test", "What is the reverse of \"xy\"?").await.unwrap();
    assert!(r.text.contains("zz") && r.text.contains("memory"), "got: {}", r.text);
}

#[tokio::test]
async fn time_questions_reach_the_clock() {
    let brain = test_brain().await;
    let r = brain.turn("test", "What is the time?").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert!(
        matches!(computed_answer(&r), Some(Value::DateTime(_))),
        "moves: {:?}",
        r.episode.response.moves
    );
    let r = brain.turn("test", "What is the hour?").await.unwrap();
    assert!(
        matches!(computed_answer(&r), Some(Value::Int(h)) if (0..24).contains(h)),
        "moves: {:?}",
        r.episode.response.moves
    );
}

// ---------------------------------------------------------------------------
// Failed ears: ask to rephrase without quoting the user
// ---------------------------------------------------------------------------

#[tokio::test]
async fn failed_ears_reply_never_echoes_the_utterance() {
    let brain = test_brain().await;
    let utterance = "zxqv flarp wibble";
    let r = brain.turn("test", utterance).await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert_eq!(r.episode.metrics.ears_path, Some(EarsPath::Failed));
    assert_eq!(r.mouth_path, "template", "failed-ears clarifications skip the mouth LLM");
    assert!(!r.text.contains(utterance), "reply echoes the utterance: {}", r.text);
    assert!(r.text.contains("rephrase"), "got: {}", r.text);
    assert!(matches!(r.episode.response.moves.as_slice(), [Move::Clarify { .. }]), "got {:?}", r.episode.response.moves);
}

// ---------------------------------------------------------------------------
// Opinions: the user's noun, known facts, reported views
// ---------------------------------------------------------------------------

#[tokio::test]
async fn opinion_fallback_uses_the_users_noun_and_known_facts() {
    let brain = test_brain().await;
    let r = say(&brain, "What does Assistant think about dogs?").await;
    assert!(r.contains("view on dogs yet"), "got: {r}");
    assert!(!r.contains("on dog "), "lemma leaked: {r}");

    say(&brain, "John owns a dog.").await;
    let r = say(&brain, "What does Assistant think about dogs?").await;
    assert_eq!(r, "i don't have a view on dogs yet, but i know John owns a dog. what do you think?");
}

#[tokio::test]
async fn reported_view_is_stored_and_read_back() {
    let brain = test_brain().await;
    let r = say(&brain, "User thinks that dogs are great.").await;
    assert!(r.contains("dogs are great"), "got: {r}");

    let r = brain.turn("test", "What does User think about dogs?").await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0);
    assert!(
        matches!(r.episode.response.moves.as_slice(), [Move::Answer { values, source: Some(s), .. }] if s == "memory" && values == &[Value::text("dogs are great")]),
        "got {:?}",
        r.episode.response.moves
    );
    assert_eq!(say(&brain, "What does User think about cats?").await, "i don't know what you think about cats yet");
}

// ---------------------------------------------------------------------------
// BrainHost sees the store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn host_sees_facts_and_recalls() {
    let brain = test_brain().await;
    say(&brain, "John owns a dog.").await;

    let host = BrainHost::with_store(PermissionMode::default(), &brain.store);
    let facts = host.facts(&ActionId("rel.own".into()), &[None, None]);
    assert_eq!(facts.len(), 1, "got {facts:?}");
    assert_eq!(facts[0].args[0], Value::Name("John".into()));

    let recalled = host.recall("dog", 5);
    assert!(recalled.iter().any(|s| s.contains("dog")), "got {recalled:?}");

    let blind = BrainHost::memoryless(PermissionMode::default());
    assert!(blind.facts(&ActionId("rel.own".into()), &[None, None]).is_empty());
}

#[tokio::test]
async fn mem_recall_runs_through_the_kernel() {
    let brain = test_brain().await;
    say(&brain, "John owns a dog.").await;
    let r = say(&brain, "Assistant, recall \"dog\"!").await;
    assert!(r.contains("dog"), "mem.recall should see the store, got: {r}");
}
