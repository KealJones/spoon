//! The `spoon teach` surface on the Brain, offline: hand-built spec, pairs,
//! stance and facts go in through the teach methods, the store is closed and
//! reopened, and everything is still there and usable in a turn.
//! interior_llm_calls == 0 throughout.

use std::path::PathBuf;
use std::sync::Arc;

use spoon_core::types::*;
use spoon_mind::brain::{Brain, BrainConfig};
use spoon_mind::teacher::TaughtStance;

async fn file_brain(db_path: PathBuf) -> Arc<Brain> {
    Brain::open(BrainConfig { db_path: Some(db_path), offline: true, debug: true, ..Default::default() })
        .await
        .expect("Brain::open")
}

async fn say(brain: &Brain, text: &str) -> spoon_mind::brain::TurnResult {
    let r = brain.turn("teach-test", text).await.unwrap();
    assert_eq!(r.episode.metrics.interior_llm_calls, 0, "interior LLM call on {text:?}");
    r
}

fn double_spec() -> Spec {
    let ex = |i: i64| Example { inputs: vec![Value::Int(i)], output: Value::Int(i * 2) };
    Spec {
        id: "teacher:double:test".into(),
        name_hint: "double".into(),
        verbs: vec!["double".into()],
        phrasings: vec![],
        params: vec![Type::Int],
        param_names: vec!["n".into()],
        ret: Type::Int,
        examples: vec![ex(3), ex(5), ex(4), ex(7)],
        description: "twice the number".into(),
        source: "teacher".into(),
    }
}

fn pair(utterance: &str, sce: &str) -> Pair {
    Pair { id: 0, utterance: utterance.into(), sce: sce.into(), source: "teacher".into(), at: 0, credit: 0 }
}

#[tokio::test]
async fn taught_knowledge_survives_restart() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_path_buf();

    // Phase 1: teach through the Brain methods, no LLM anywhere.
    {
        let brain = file_brain(db_path.clone()).await;
        assert!(brain.teacher().is_none(), "offline brains have no teacher seat");
        assert!(!brain.known_verbs().contains(&"double".to_string()));

        let outcome = brain.learn_from_spec("double", double_spec()).expect("synthesis");
        assert!(brain.known_verbs().contains(&"double".to_string()));
        assert_eq!(outcome.description, "add(x, x)");

        let fresh = brain
            .add_pairs(&[pair("can you dubble 4 pls", "Assistant, double 4!"), pair("dubble 9 for me plz", "Assistant, double 9!")])
            .await
            .unwrap();
        assert_eq!(fresh, 2);
        // The same pairs again are not new.
        assert_eq!(brain.add_pairs(&[pair("can you dubble 4 pls", "Assistant, double 4!")]).await.unwrap(), 0);

        let stance = brain
            .add_stance(&TaughtStance {
                topic: "dogs".into(),
                stance: "Dogs make loyal companions".into(),
                reasons: vec!["they bond with people".into(), "they are easy to read".into()],
                counterpoints: vec!["they need daily time".into()],
                confidence: 0.8,
            })
            .unwrap();
        assert!(stance.id > 0);

        assert_eq!(brain.assert_sce("teach", "Mary is a doctor.", "teacher").await.unwrap(), 1);
        assert_eq!(brain.assert_sce("teach", "Every doctor is a person.", "teacher").await.unwrap(), 1);
        assert!(brain.assert_sce("teach", "Who owns a dog?", "teacher").await.is_err(), "questions are not facts");
        assert!(brain.assert_sce("teach", "zxqv flarp wibble", "teacher").await.is_err(), "garbage is refused");
    }

    // Phase 2: reopen and use all of it through real turns.
    {
        let brain = file_brain(db_path).await;
        assert!(brain.known_verbs().contains(&"double".to_string()), "learned verb reloaded from the store");

        let r = say(&brain, "Assistant, double 7!").await;
        assert!(r.text.contains("14"), "learned action runs after restart, got: {}", r.text);
        assert_eq!(brain.metrics().synthesis_attempted, 0, "no re-synthesis");

        let r = say(&brain, "can you dubble 4 pls").await;
        assert!(
            matches!(r.episode.metrics.ears_path, Some(EarsPath::Phrasing) | Some(EarsPath::Retrieval)),
            "taught pair should take a native learned path, got {:?}",
            r.episode.metrics.ears_path
        );
        assert!(r.text.contains('8'), "got: {}", r.text);

        let stances = brain.store.lock().stances(&["dog".to_string()]).unwrap();
        assert_eq!(stances.len(), 1);
        assert_eq!(stances[0].stance, "Dogs make loyal companions");
        assert_eq!(stances[0].source, "teacher");
        let r = say(&brain, "What does Assistant think about dogs?").await;
        assert!(
            r.episode.response.moves.iter().any(|m| matches!(m, Move::Opinion { .. })),
            "opinion question must find the stance, got {:?}",
            r.episode.response.moves
        );
        assert!(r.text.to_lowercase().contains("loyal"), "got: {}", r.text);

        let r = say(&brain, "Is Mary a doctor?").await;
        assert!(r.text.to_lowercase().starts_with("yes"), "teacher fact answers after restart, got: {}", r.text);
        let r = say(&brain, "Is Mary a person?").await;
        assert!(r.text.to_lowercase().starts_with("yes"), "teacher universal applies after restart, got: {}", r.text);

        // Exportable: the seed carries the action, the pairs, the stance and
        // the teacher facts (user facts stay private).
        let seed = brain.store.lock().export_seed("t").unwrap();
        assert!(seed.actions.iter().any(|a| a.verbs.contains(&"double".to_string())));
        assert!(seed.pairs.iter().any(|p| p.utterance == "can you dubble 4 pls"));
        assert_eq!(seed.stances.len(), 1);
        assert!(seed.facts.iter().any(|f| f.source == "teacher" && f.args.contains(&Value::Name("Mary".into()))));
    }
}
