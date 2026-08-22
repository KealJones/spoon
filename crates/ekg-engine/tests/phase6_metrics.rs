use std::collections::BTreeMap;

use ekg_adapt::SkillCandidate;
use ekg_core::{Evaluation, Expr, Param, Procedure, Value, VerifiabilityTier};
use ekg_engine::{Engine, PromotionReplay};

fn inputs(value: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([("x".into(), Value::Int(value))])
}

fn identity(name: &str) -> Procedure {
    Procedure::new(name, vec![Param::named("x")], Expr::Var("x".into()))
}

#[test]
fn phase6_snapshot_reports_only_persisted_teacher_and_skill_evidence() {
    let engine = Engine::in_memory_with_admin("phase6-metrics").unwrap();

    let local = identity("LOCAL");
    let transferred = identity("TRANSFERRED");
    let regressed = identity("REGRESSED");
    for procedure in [&local, &transferred, &regressed] {
        engine.admin_insert_procedure(procedure).unwrap();
    }

    let local_episode = engine
        .execute_procedure(local.id, inputs(1), Some(Value::Int(1)))
        .unwrap()
        .episode;
    let transferred_episode = engine
        .execute_procedure(transferred.id, inputs(2), Some(Value::Int(2)))
        .unwrap()
        .episode;
    let regressed_episode = engine
        .execute_procedure(regressed.id, inputs(3), Some(Value::Int(3)))
        .unwrap()
        .episode;

    let mut teacher_assisted = ekg_core::Episode::new("teacher-assisted answer");
    teacher_assisted.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: true,
        details: "verified".into(),
        surprise: None,
    });
    teacher_assisted.teacher_interaction = Some(serde_json::json!({
        "request": { "situation": "teacher-assisted answer" },
        "proposal": { "source": "human:test" }
    }));
    engine.admin_insert_episode(&teacher_assisted).unwrap();

    let transfer_skill = engine
        .register_skill_candidate(&SkillCandidate {
            name: "transfer skill".into(),
            source_episode_ids: vec![transferred_episode.id],
            support_count: 1,
            rationale: "test evidence".into(),
            failure_critic: false,
        })
        .unwrap();
    engine
        .evaluate_skill_for_shadow(
            &transfer_skill.id,
            [PromotionReplay {
                episode_id: transferred_episode.id,
                incumbent_correct: true,
                challenger_correct: true,
                incumbent_trace_steps: Some(2),
                challenger_trace_steps: Some(1),
                incumbent_candidates_explored: None,
                challenger_candidates_explored: None,
                transfer: true,
            }],
        )
        .unwrap();
    engine
        .record_skill_shadow_live_win(
            &transfer_skill.id,
            Value::Int(2),
            inputs(2),
            Evaluation {
                tier: VerifiabilityTier::Hard,
                success: true,
                details: "live shadow check".into(),
                surprise: None,
            },
            "test-verifier",
        )
        .unwrap();
    engine
        .execute_managed_skill(&transfer_skill.id, inputs(4), Some(Value::Int(4)))
        .unwrap();

    let regression_skill = engine
        .register_skill_candidate(&SkillCandidate {
            name: "regression skill".into(),
            source_episode_ids: vec![regressed_episode.id],
            support_count: 1,
            rationale: "test evidence".into(),
            failure_critic: false,
        })
        .unwrap();
    engine
        .evaluate_skill_for_shadow(
            &regression_skill.id,
            [PromotionReplay {
                episode_id: regressed_episode.id,
                incumbent_correct: true,
                challenger_correct: false,
                incumbent_trace_steps: None,
                challenger_trace_steps: None,
                incumbent_candidates_explored: None,
                challenger_candidates_explored: None,
                transfer: false,
            }],
        )
        .unwrap();

    let snapshot = engine.metrics_snapshot().unwrap();
    assert_eq!(snapshot.phase6.teacher_interaction_episodes, 1);
    assert_eq!(snapshot.phase6.teacher_assisted_successes, 1);
    assert_eq!(snapshot.phase6.teacher_free_successes, 5);
    assert_eq!(snapshot.phase6.managed_skill_records_examined, 2);
    assert_eq!(snapshot.phase6.replay_preserved_skill_verdicts, 1);
    assert_eq!(snapshot.phase6.replay_regressions, 1);
    assert_eq!(snapshot.phase6.transfer_eligible_skill_verdicts, 1);
    assert_eq!(snapshot.phase6.currently_promoted_skills, 1);
    assert_eq!(snapshot.phase6.post_promotion_skill_uses, 1);
    assert_eq!(snapshot.phase6.post_promotion_skill_successes, 1);

    // A successful strong observation is a regression baseline, not a claim
    // that a fresh replay has passed.
    assert_eq!(snapshot.verified_answer_count, 5);
    assert_eq!(local_episode.succeeded(), true);
}
