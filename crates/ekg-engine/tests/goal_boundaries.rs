use ekg_engine::{CuriosityGap, Engine, EngineError, GapKind, GoalKind};

fn curiosity_gap(id: &str) -> CuriosityGap {
    CuriosityGap {
        id: id.into(),
        kind: GapKind::FailedPrediction,
        statement: "the prediction mechanism is under-specified".into(),
        blast_radius: 3.0,
        goal_relevance: 2.0,
        learning_progress: 1.0,
        cost_to_close: 1.0,
        value_score: 6.0,
        source_episode: Some("episode-1".into()),
        resolved: false,
        created_at: 10,
    }
}

#[test]
fn derived_goals_cannot_bypass_the_goal_boundary() {
    let engine = Engine::in_memory_with_admin("goal-boundary-admin").unwrap();

    let error = engine
        .create_goal(GoalKind::Learning, "invent a learning goal", None)
        .unwrap_err();
    assert!(matches!(error, EngineError::InvalidInput(_)));

    let error = engine
        .create_goal(GoalKind::Instrumental, "invent an instrumental goal", None)
        .unwrap_err();
    assert!(matches!(error, EngineError::InvalidInput(_)));
    assert!(engine.list_goals().unwrap().is_empty());
}

#[test]
fn learning_goal_and_provenance_are_atomic() {
    let engine = Engine::in_memory_with_admin("goal-atomic-admin").unwrap();
    let standing = engine
        .create_goal(GoalKind::Standing, "remain accurate", None)
        .unwrap();
    engine
        .record_curiosity_gap(&curiosity_gap("prediction-gap"))
        .unwrap();

    let learning = engine
        .create_learning_goal(
            "calibrate the failed predictor",
            &standing.id,
            "prediction-gap",
            "repeated prediction error blocks the standing accuracy goal",
        )
        .unwrap();
    assert_eq!(learning.kind, GoalKind::Learning);
    assert_eq!(learning.parent_id.as_deref(), Some(standing.id.as_str()));

    let records = engine.list_learning_goal_records().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].learning_goal_id, learning.id);
    assert_eq!(records[0].standing_goal_id, standing.id);
    assert_eq!(records[0].source_gap_id, "prediction-gap");

    let goals_before_failed_derivation = engine.list_goals().unwrap();
    let error = engine
        .create_learning_goal(
            "unjustified learning",
            &records[0].standing_goal_id,
            "missing-gap",
            "this must roll back",
        )
        .unwrap_err();
    assert!(matches!(error, EngineError::InvalidInput(_)));
    assert_eq!(engine.list_goals().unwrap(), goals_before_failed_derivation);
    assert_eq!(engine.list_learning_goal_records().unwrap(), records);
}

#[test]
fn learning_goal_provenance_survives_reopen() {
    let path =
        std::env::temp_dir().join(format!("ekg-goal-boundary-{}.sqlite", uuid::Uuid::new_v4()));
    let path_text = path.to_string_lossy().into_owned();

    let expected = {
        let engine = Engine::open_with_admin(&path_text, "durable-goal-admin").unwrap();
        let standing = engine
            .create_goal(GoalKind::Standing, "preserve user intent", None)
            .unwrap();
        engine
            .record_curiosity_gap(&curiosity_gap("durable-gap"))
            .unwrap();
        let learning = engine
            .create_learning_goal(
                "learn the missing boundary",
                &standing.id,
                "durable-gap",
                "the gap is relevant to preserving user intent",
            )
            .unwrap();
        (standing, learning)
    };

    let reopened = Engine::open(&path_text).unwrap();
    let records = reopened.list_learning_goal_records().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].standing_goal_id, expected.0.id);
    assert_eq!(records[0].learning_goal_id, expected.1.id);
    assert_eq!(records[0].source_gap_id, "durable-gap");
    drop(reopened);

    let connection = rusqlite::Connection::open(&path).unwrap();
    let error = connection
        .execute(
            "UPDATE ekg_goals SET statement = 'tampered' WHERE id = ?1",
            rusqlite::params![expected.0.id],
        )
        .unwrap_err();
    assert!(error.to_string().contains("immutable standing goals"));

    std::fs::remove_file(path).unwrap();
}
