//! `spoon bench` through the library: the demo corpus is the pick-up
//! regression suite and must pass offline; babi must run end to end with an
//! LLM-free interior (its score is expected to be low).

use std::sync::Arc;

use spoon::bench::run_corpus;
use spoon_mind::brain::{Brain, BrainConfig};

async fn offline_brain() -> Arc<Brain> {
    Brain::open(BrainConfig { db_path: None, offline: true, debug: false, ..Default::default() })
        .await
        .expect("Brain::open")
}

#[tokio::test]
async fn demo_corpus_passes_offline() {
    let brain = offline_brain().await;
    let record = run_corpus(brain, "demo").await.expect("demo bench");
    assert_eq!(record.total, 19, "demo.json holds the 19 STATUS lines");
    assert_eq!(record.passed, record.total, "demo regression:\n{}", record.table);
    assert_eq!(record.weaning.turns, 19);
    assert_eq!(record.weaning.ears_llm, 0);
    assert_eq!(record.weaning.ears_failed, 0);
    assert_eq!(record.weaning.mouth_llm, 0);
    assert_eq!(record.weaning.interior_llm_calls, 0);
    assert_eq!(record.model, "offline");
}

#[tokio::test]
async fn babi_corpus_runs_offline() {
    let brain = offline_brain().await;
    let record = run_corpus(brain, "babi").await.expect("babi bench");
    assert_eq!(record.total, 21);
    assert_eq!(record.weaning.interior_llm_calls, 0);
    assert!(record.weaning.turns > record.total as u64, "story lines are turns too");
    assert!(record.table.contains("babi: "), "table has the total line:\n{}", record.table);
    assert!(record.report["families"].is_object());
}

#[tokio::test]
async fn convo20_corpus_runs_offline() {
    let brain = offline_brain().await;
    let record = run_corpus(brain, "convo20").await.expect("convo20 bench");
    assert_eq!(record.total, 20);
    assert_eq!(record.passed, 20, "convo20 is native 20/20:\n{}", record.table);
    assert_eq!(record.weaning.ears_llm_calls, 0);
}

#[tokio::test]
async fn unknown_corpus_is_an_error() {
    let brain = offline_brain().await;
    let err = run_corpus(brain, "nope").await.err().expect("unknown corpus must fail");
    assert!(err.to_string().contains("convo20 | ace | babi | demo"), "got: {err}");
}
