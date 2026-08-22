use std::collections::BTreeMap;

use ekg_core::{BinOp, Concept, Expr, MutabilityClass, Param, Procedure, Value};
use ekg_engine::{CycleBudget, CycleInput, CycleProgress, Engine};
use ekg_episode::{EpisodeFeedback, FeedbackSource};

fn inputs(value: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([("x".into(), Value::Int(value))])
}

#[test]
fn startup_finishes_inserted_episode_saga_before_minting_exact_trust_and_contradiction() {
    let path = std::env::temp_dir().join(format!(
        "ekg-episode-saga-recovery-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().into_owned();
    let second;
    {
        let engine = Engine::open_with_admin(&path_text, "test-admin").unwrap();
        let concept = Concept::new("SAGA_FACT", MutabilityClass::DefeasibleGeneral);
        engine.admin_insert_concept(&concept).unwrap();
        let procedure = Procedure::new("SAGA_FACT", vec![Param::named("x")], Expr::Var("x".into()))
            .with_concept(concept.id);
        engine.admin_insert_procedure(&procedure).unwrap();
        let first = engine
            .execute_procedure(procedure.id, inputs(1), Some(Value::Int(1)))
            .unwrap()
            .episode;
        assert!(engine.trust_receipt_for_episode(&first).unwrap().is_some());

        second = {
            let mut episode = first.clone();
            episode.id = ekg_core::EpisodeId::new();
            episode.created_at = episode.created_at.saturating_add(1);
            // These observations intentionally have no environmental
            // discriminator: their scopes overlap, so the recovery pass must
            // surface the contradictory values rather than treat them as
            // different situations.
            episode.context.environment = BTreeMap::new();
            episode.prediction = Some(Value::Int(2));
            episode.observed_result = Some(Value::Int(2));
            episode.observed_facts[0].id = format!("{}:0", episode.id);
            episode.observed_facts[0].source_episode = Some(episode.id);
            episode.observed_facts[0].value = Value::Int(2);
            episode.observed_facts[0].scope = BTreeMap::new();
            episode
        };

        // Reproduce a crash after finalized insertion but before trust receipt
        // and contradiction detection: the journal already exists and the raw
        // episode row is present, but neither derived authority item is.
        let connection = rusqlite::Connection::open(&path_text).unwrap();
        connection
            .execute(
                "INSERT INTO engine_episode_sagas
                    (episode_id, episode_json, cycle_id, owner_id, pending_json, created_at)
                 VALUES (?1, ?2, NULL, NULL, NULL, ?3)",
                rusqlite::params![
                    second.id.to_string(),
                    serde_json::to_string(&second).unwrap(),
                    second.created_at,
                ],
            )
            .unwrap();
        engine.admin_insert_episode(&second).unwrap();
        assert!(engine.trust_receipt_for_episode(&second).unwrap().is_none());
    }

    let reopened = Engine::open(&path_text).unwrap();
    assert!(
        reopened
            .trust_receipt_for_episode(&second)
            .unwrap()
            .is_some()
    );
    assert_eq!(reopened.list_held_contradictions().unwrap().len(), 1);
    let connection = rusqlite::Connection::open(&path_text).unwrap();
    let pending: i64 = connection
        .query_row("SELECT COUNT(*) FROM engine_episode_sagas", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(pending, 0);
    drop(connection);
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn failed_attempt_and_teacher_continuation_are_durable_together() {
    let path = std::env::temp_dir().join(format!(
        "ekg-failed-attempt-pending-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().into_owned();
    let cycle_id;
    {
        let mut engine = Engine::open_with_admin(&path_text, "test-admin").unwrap();
        let concept = Concept::new("BREAK", MutabilityClass::Definitional);
        engine.admin_insert_concept(&concept).unwrap();
        let procedure = Procedure::new(
            "BREAK",
            vec![Param::named("x")],
            Expr::BinOp {
                op: BinOp::Div,
                left: Box::new(Expr::Var("x".into())),
                right: Box::new(Expr::Literal(Value::Int(0))),
            },
        )
        .with_concept(concept.id);
        engine.admin_insert_procedure(&procedure).unwrap();
        let progress = engine
            .begin_cycle(CycleInput {
                situation: "break 7".into(),
                environment: BTreeMap::new(),
                assumptions: Vec::new(),
                budget: CycleBudget {
                    max_exec_steps: 100,
                    max_context_items: 16,
                    max_teacher_turns: 1,
                },
                teacher_allowed: true,
            })
            .unwrap();
        let CycleProgress::NeedTeacher {
            cycle_id: pending, ..
        } = progress
        else {
            panic!("local failure should preserve a teacher continuation");
        };
        cycle_id = pending;
        let failed = engine
            .episodes()
            .list_recent(10)
            .unwrap()
            .into_iter()
            .find(|episode| episode.action.as_deref() == Some("failed:awaiting-teacher"))
            .expect("failed attempt must be finalized");
        assert!(engine.trust_receipt_for_episode(&failed).unwrap().is_some());
    }

    let connection = rusqlite::Connection::open(&path_text).unwrap();
    let (state, pending_json): (String, String) = connection
        .query_row(
            "SELECT state, pending_json FROM engine_active_cycles WHERE cycle_id = ?1",
            rusqlite::params![cycle_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "pending_teacher");
    assert!(pending_json.contains("prior_failure"));
    let sagas: i64 = connection
        .query_row("SELECT COUNT(*) FROM engine_episode_sagas", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(sagas, 0);
    drop(connection);

    // Startup claims, rather than discards, the exact continuation.
    let reopened = Engine::open(&path_text).unwrap();
    let connection = rusqlite::Connection::open(&path_text).unwrap();
    let still_pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM engine_active_cycles
             WHERE cycle_id = ?1 AND state = 'pending_teacher'",
            rusqlite::params![cycle_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still_pending, 1);
    drop(connection);
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn startup_finishes_an_authenticated_feedback_saga_after_insert_before_receipt() {
    let path = std::env::temp_dir().join(format!(
        "ekg-feedback-saga-recovery-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().into_owned();
    let feedback;
    {
        let engine = Engine::open_with_admin(&path_text, "test-admin").unwrap();
        let procedure = Procedure::new("FEEDBACK", vec![Param::named("x")], Expr::Var("x".into()));
        engine.admin_insert_procedure(&procedure).unwrap();
        let episode = engine
            .execute_procedure(procedure.id, inputs(2), Some(Value::Int(2)))
            .unwrap()
            .episode;
        feedback = EpisodeFeedback::new(
            episode.id,
            Value::Int(3),
            ekg_core::Evaluation {
                tier: ekg_core::VerifiabilityTier::Hard,
                success: false,
                details: "external verifier found a discrepancy".into(),
                surprise: Some(1.0),
            },
            FeedbackSource::new("test", Some("verifier".into())),
            "feedback-saga",
        );
        let connection = rusqlite::Connection::open(&path_text).unwrap();
        connection
            .execute(
                "INSERT INTO engine_feedback_sagas
                    (feedback_id, feedback_json, verifier_identity, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    feedback.id.to_string(),
                    serde_json::to_string(&feedback).unwrap(),
                    "test-verifier",
                    feedback.created_at,
                ],
            )
            .unwrap();
        engine.admin_append_feedback(&feedback).unwrap();
        assert!(
            engine
                .trust_receipt_for_feedback(&feedback)
                .unwrap()
                .is_none()
        );
    }

    let reopened = Engine::open(&path_text).unwrap();
    assert!(
        reopened
            .trust_receipt_for_feedback(&feedback)
            .unwrap()
            .is_some()
    );
    let connection = rusqlite::Connection::open(&path_text).unwrap();
    let sagas: i64 = connection
        .query_row("SELECT COUNT(*) FROM engine_feedback_sagas", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(sagas, 0);
    drop(connection);
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}
