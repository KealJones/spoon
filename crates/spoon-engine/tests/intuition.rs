use std::collections::BTreeMap;

use spoon_core::{Concept, Expr, MutabilityClass, Param, Procedure, Relationship, Value};
use spoon_engine::{
    ActivationSeed, ActivationSpreadQuery, Engine, EpistemicChallengeKind, RankingExample,
    TraversalDirection, TypedRelationshipTraversal,
};

#[test]
fn engine_exposes_bounded_typed_activation_candidates() {
    let engine = Engine::in_memory_with_admin("activation-admin").unwrap();
    let source = Concept::new("source", MutabilityClass::Definitional);
    let target = Concept::new("target", MutabilityClass::Definitional);
    engine.admin_insert_concept(&source).unwrap();
    engine.admin_insert_concept(&target).unwrap();
    engine
        .admin_insert_relationship(&Relationship::new(source.id, target.id, "supports"))
        .unwrap();

    let result = engine
        .activation_candidates(&ActivationSpreadQuery {
            seeds: vec![ActivationSeed {
                concept: source.id,
                activation: 1.0,
            }],
            traversals: vec![TypedRelationshipTraversal {
                kind: "supports".into(),
                direction: TraversalDirection::Outgoing,
                decay: 0.5,
            }],
            max_hops: 1,
            max_candidates: 4,
            max_expansions: 8,
            min_activation: 0.01,
        })
        .unwrap();

    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].concept, target.id);
}

#[test]
fn engine_indexes_graph_material_and_ranks_recall_without_scanning_everything() {
    let engine = Engine::in_memory_with_admin("intuition-admin").unwrap();
    let arithmetic = Concept::new("arithmetic", MutabilityClass::Definitional);
    engine.admin_insert_concept(&arithmetic).unwrap();
    let procedure = Procedure::new("DOUBLE", vec![Param::named("x")], Expr::Var("x".into()))
        .with_concept(arithmetic.id);
    engine.admin_insert_procedure(&procedure).unwrap();
    let unrelated = Concept::new("weather", MutabilityClass::DefeasibleGeneral);
    engine.admin_insert_concept(&unrelated).unwrap();

    let candidates = engine.recall_candidates("double", 4).unwrap();
    assert!(!candidates.is_empty());
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.text.contains("DOUBLE"))
    );
    assert!(candidates.len() <= 4);

    engine
        .record_ranking_example(&RankingExample {
            query: "double".into(),
            candidate_id: format!("procedure:{}:1", procedure.id),
            used: true,
            succeeded: true,
            rung: 1,
        })
        .unwrap();
    let ranked = engine.rank_recall_candidates("double", 4).unwrap();
    assert_eq!(ranked[0].id, format!("procedure:{}:1", procedure.id));

    let mut inputs = BTreeMap::new();
    inputs.insert("x".into(), Value::Int(7));
    let source_episode = engine
        .execute_procedure(procedure.id, inputs, Some(Value::Int(7)))
        .unwrap()
        .episode
        .id
        .to_string();

    let task = engine
        .generate_self_supervision(
            Some(&source_episode),
            serde_json::json!({"situation":"double 7"}),
            serde_json::json!({"candidate":"arithmetic"}),
            "predict_validated_interpretation",
            true,
        )
        .unwrap();
    assert!(task.grounded);
    assert!(
        engine
            .generate_epistemic_challenge(
                Some(&source_episode),
                EpistemicChallengeKind::HiddenComputation,
                serde_json::json!({"expression":"7 * 2"}),
                serde_json::json!({"expected":14}),
                true,
            )
            .is_ok()
    );
    assert!(
        engine
            .generate_epistemic_challenge(
                Some(&source_episode),
                EpistemicChallengeKind::HiddenComputation,
                serde_json::json!({"expression":"7 * 2"}),
                serde_json::json!({"expected":14}),
                false,
            )
            .is_err()
    );
    let metrics = engine.intuition_metrics().unwrap();
    assert!(metrics.indexed_documents >= 4);
    assert_eq!(metrics.ranking_examples, 1);
    assert_eq!(metrics.generated_tasks, 2);
    assert_eq!(metrics.completed_tasks, 0);
    assert_eq!(metrics.grounded_tasks, 0);
}

#[test]
fn trusted_execution_generates_and_verifies_one_bounded_replay_challenge() {
    let engine = Engine::in_memory_with_admin("grounded-supervision").unwrap();
    let procedure = Procedure::new("DOUBLE", vec![Param::named("x")], Expr::Var("x".into()));
    engine.admin_insert_procedure(&procedure).unwrap();
    let mut inputs = BTreeMap::new();
    inputs.insert("x".into(), Value::Int(7));
    let source = engine
        .execute_procedure(procedure.id, inputs, Some(Value::Int(7)))
        .unwrap()
        .episode;
    assert!(engine.trust_receipt_for_episode(&source).unwrap().is_some());

    let task = engine
        .generate_grounded_self_supervision_from_episode(source.id)
        .unwrap();
    let source_id = source.id.to_string();
    assert_eq!(task.kind, "verified_trace_replay");
    assert_eq!(task.source_episode.as_deref(), Some(source_id.as_str()));
    assert!(task.completed);
    assert!(task.grounded);
    assert_eq!(
        task.verifier.as_deref(),
        Some("bounded_exact_trace_replay_v1")
    );
    assert_eq!(
        task.outcome
            .as_ref()
            .and_then(|outcome| outcome.get("status"))
            .and_then(serde_json::Value::as_str),
        Some("matched")
    );
    assert!(
        engine
            .generate_grounded_self_supervision_from_episode(source.id)
            .is_err()
    );
    let metrics = engine.intuition_metrics().unwrap();
    assert_eq!(metrics.generated_tasks, 1);
    assert_eq!(metrics.completed_tasks, 1);
    assert_eq!(metrics.grounded_tasks, 1);
}

#[test]
fn grounded_supervision_rejects_untrusted_episode_sources() {
    let engine = Engine::in_memory_with_admin("grounded-supervision").unwrap();
    let mut forged = spoon_core::Episode::new("forged success");
    forged.observed_result = Some(Value::Int(7));
    forged.observed_facts.push(spoon_core::ObservedFact::new(
        "forged-result",
        Value::Int(7),
        BTreeMap::new(),
    ));
    forged.evaluation = Some(spoon_core::Evaluation {
        tier: spoon_core::VerifiabilityTier::Hard,
        success: true,
        details: "forged".into(),
        surprise: None,
    });
    engine.admin_insert_episode(&forged).unwrap();
    let error = engine
        .generate_grounded_self_supervision_from_episode(forged.id)
        .unwrap_err();
    assert!(error.to_string().contains("trusted successful"));
    assert_eq!(engine.intuition_metrics().unwrap().generated_tasks, 0);
}

#[test]
fn intuition_index_rebuilds_from_a_file_backed_engine() {
    let path =
        std::env::temp_dir().join(format!("spoon-intuition-{}.sqlite", uuid::Uuid::new_v4()));
    let path_text = path.to_string_lossy().into_owned();
    {
        let engine = Engine::open_with_admin(&path_text, "intuition-reopen").unwrap();
        let concept = Concept::new("geometry", MutabilityClass::Definitional);
        engine.admin_insert_concept(&concept).unwrap();
        let procedure = Procedure::new("AREA", vec![Param::named("x")], Expr::Var("x".into()))
            .with_concept(concept.id);
        engine.admin_insert_procedure(&procedure).unwrap();
    }
    let reopened = Engine::open(&path_text).unwrap();
    let candidates = reopened.recall_candidates("geometry area", 8).unwrap();
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.text.contains("AREA"))
    );
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[allow(dead_code)]
fn _keep_imports_stable(_: BTreeMap<String, Value>) {}
