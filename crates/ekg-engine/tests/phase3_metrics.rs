use ekg_core::{Concept, Expr, MutabilityClass, Param, Procedure, Value};
use ekg_engine::{Engine, RankingExample};

#[test]
fn larger_corpus_keeps_recall_bounded_and_tracks_grounding() {
    let engine = Engine::in_memory_with_admin("phase3-metrics").unwrap();
    for index in 0..128 {
        let name = if index % 2 == 0 {
            format!("arithmetic procedure family {index}")
        } else {
            format!("weather observation family {index}")
        };
        engine
            .admin_insert_concept(&Concept::new(name, MutabilityClass::Definitional))
            .unwrap();
    }

    for _ in 0..12 {
        let candidates = engine.recall_candidates("arithmetic family", 8).unwrap();
        assert!(!candidates.is_empty());
        assert!(candidates.len() <= 8);
    }
    let metrics = engine.intuition_metrics().unwrap();
    assert_eq!(metrics.indexed_documents, 128);
    assert_eq!(metrics.retrieval_queries, 12);
    assert!(metrics.candidates_examined <= 12 * 8);

    for candidate in engine.recall_candidates("arithmetic family", 4).unwrap() {
        engine
            .record_ranking_example(&RankingExample {
                query: "arithmetic family".into(),
                candidate_id: candidate.id,
                used: true,
                succeeded: true,
                rung: 1,
            })
            .unwrap();
    }
    let ranked = engine
        .rank_recall_candidates("arithmetic family", 4)
        .unwrap();
    assert!(
        ranked
            .iter()
            .all(|candidate| candidate.learned_score.is_finite())
    );

    let procedure = Procedure::new("ECHO", vec![Param::named("x")], Expr::Var("x".into()));
    engine.admin_insert_procedure(&procedure).unwrap();
    let mut inputs = std::collections::BTreeMap::new();
    inputs.insert("x".into(), Value::Int(1));
    let source_episode = engine
        .execute_procedure(procedure.id, inputs, Some(Value::Int(1)))
        .unwrap()
        .episode
        .id
        .to_string();

    engine
        .generate_self_supervision(
            Some(&source_episode),
            serde_json::json!({"situation": "arithmetic family"}),
            serde_json::json!({"target": "validated candidate"}),
            "predict_useful_knowledge",
            true,
        )
        .unwrap();
    let metrics = engine.intuition_metrics().unwrap();
    assert_eq!(metrics.grounded_tasks, 1);
    assert_eq!(metrics.grounding_ratio, 1.0);
}
