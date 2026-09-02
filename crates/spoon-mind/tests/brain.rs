//! Integration tests for the Brain turn loop.
//! All tests: offline, in-memory, no network. interior_llm_calls == 0.

use std::sync::Arc;

use spoon_mind::brain::{Brain, BrainConfig};

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
