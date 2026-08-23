use std::collections::BTreeMap;

use ekg_core::{BinOp, Evaluation, Expr, Param, Procedure, Value, VerifiabilityTier};
use ekg_engine::{Engine, PromotionReplay, SkillLifecycle};

fn double() -> Procedure {
    Procedure::new(
        "DOUBLE",
        vec![Param::named("x")],
        Expr::BinOp {
            op: BinOp::Mul,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(2))),
        },
    )
}

fn inputs(value: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([("x".into(), Value::Int(value))])
}

#[test]
fn trusted_candidate_moves_through_shadow_promotion_and_reconstructible_retirement() {
    let engine = Engine::in_memory_with_admin("consolidation-lifecycle").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let first = engine
        .execute_procedure(procedure.id, inputs(2), Some(Value::Int(4)))
        .unwrap();
    engine
        .execute_procedure(procedure.id, inputs(3), Some(Value::Int(6)))
        .unwrap();

    let candidate = engine.discover_skill_candidates(32).unwrap().remove(0);
    let managed = engine.register_skill_candidate(&candidate).unwrap();
    assert_eq!(managed.lifecycle, SkillLifecycle::Candidate);
    assert_eq!(
        engine.register_skill_candidate(&candidate).unwrap().id,
        managed.id
    );

    let shadowed = engine
        .evaluate_skill_for_shadow(
            &managed.id,
            [PromotionReplay {
                episode_id: first.episode.id,
                incumbent_correct: true,
                challenger_correct: true,
                incumbent_trace_steps: Some(2),
                challenger_trace_steps: Some(1),
                incumbent_candidates_explored: Some(4),
                challenger_candidates_explored: Some(2),
                transfer: false,
            }],
        )
        .unwrap();
    assert_eq!(shadowed.lifecycle, SkillLifecycle::Shadow);

    let promoted = engine
        .record_skill_shadow_live_win(
            &managed.id,
            Value::Int(4),
            BTreeMap::from([("x".into(), Value::Int(2))]),
            Evaluation {
                tier: VerifiabilityTier::Hard,
                success: true,
                details: "independent shadow check".into(),
                surprise: None,
            },
            "local-test-verifier",
        )
        .unwrap();
    assert_eq!(promoted.lifecycle, SkillLifecycle::Promoted);
    assert_eq!(promoted.shadow_live_wins, 1);

    let ranked = engine
        .rank_active_managed_skills("repeated procedure", 8)
        .unwrap();
    assert_eq!(
        ranked.first().map(|skill| skill.id.as_str()),
        Some(managed.id.as_str())
    );

    let reused = engine
        .execute_managed_skill(&managed.id, inputs(5), Some(Value::Int(10)))
        .unwrap();
    assert_eq!(reused.value, Value::Int(10));
    let best = engine
        .execute_best_managed_skill("repeated procedure", inputs(6), Some(Value::Int(12)))
        .unwrap();
    assert_eq!(best.value, Value::Int(12));
    assert_eq!(
        engine.list_managed_skills(8).unwrap()[0].experience_successes,
        2
    );

    let mut successor_candidate = candidate.clone();
    successor_candidate.name = "DOUBLE successor".into();
    successor_candidate.rationale = "independently verified successor".into();
    let successor = engine
        .register_skill_candidate(&successor_candidate)
        .unwrap();
    engine
        .evaluate_skill_for_shadow(
            &successor.id,
            [PromotionReplay {
                episode_id: first.episode.id,
                incumbent_correct: true,
                challenger_correct: true,
                incumbent_trace_steps: Some(2),
                challenger_trace_steps: Some(1),
                incumbent_candidates_explored: Some(4),
                challenger_candidates_explored: Some(2),
                transfer: false,
            }],
        )
        .unwrap();
    engine
        .record_skill_shadow_live_win(
            &successor.id,
            Value::Int(4),
            BTreeMap::from([("x".into(), Value::Int(2))]),
            Evaluation {
                tier: VerifiabilityTier::Hard,
                success: true,
                details: "successor live check".into(),
                surprise: None,
            },
            "local-test-verifier",
        )
        .unwrap();
    let retired = engine
        .retire_managed_skill(&managed.id, &successor.id, "subsumed")
        .unwrap();
    assert_eq!(retired.lifecycle, SkillLifecycle::Retired);
    assert!(retired.retirement.unwrap().reconstructible);
    assert!(
        engine
            .list_managed_skills(8)
            .unwrap()
            .iter()
            .any(|skill| skill.id == managed.id && skill.lifecycle == SkillLifecycle::Retired)
    );
}

#[test]
fn engine_derived_challenger_replays_trusted_sources_before_shadow() {
    let engine = Engine::in_memory_with_admin("engine-derived-challenger").unwrap();
    let child = double();
    engine.admin_insert_procedure(&child).unwrap();
    let incumbent = Procedure::new(
        "DOUBLE THROUGH CHILD",
        vec![Param::named("x")],
        Expr::Call {
            procedure: child.id,
            args: vec![Expr::Var("x".into())],
        },
    );
    engine.admin_insert_procedure(&incumbent).unwrap();
    let source = engine
        .execute_procedure(incumbent.id, inputs(2), Some(Value::Int(4)))
        .unwrap();
    let candidate = engine
        .register_single_success_skill(source.episode.id)
        .unwrap();

    let mut challenger = incumbent.clone();
    challenger.version = 2;
    challenger.body = Expr::BinOp {
        op: BinOp::Mul,
        left: Box::new(Expr::Var("x".into())),
        right: Box::new(Expr::Literal(Value::Int(2))),
    };
    engine
        .admin_revise_procedure(&challenger, incumbent.version)
        .unwrap();

    let shadowed = engine
        .evaluate_skill_for_shadow_with_challenger(&candidate.id, incumbent.id, 2)
        .unwrap();
    assert_eq!(shadowed.lifecycle, SkillLifecycle::Shadow);
    assert!(shadowed.promotion_verdict.unwrap().shadow_eligible());
}

#[test]
fn retirement_rejects_missing_or_unpromoted_successors() {
    let engine = Engine::in_memory_with_admin("retirement-boundary").unwrap();
    assert!(
        engine
            .retire_managed_skill("old", "missing", "subsumed")
            .is_err()
    );
}

#[test]
fn raw_admin_episode_cannot_back_a_skill_candidate() {
    let engine = Engine::in_memory_with_admin("consolidation-untrusted").unwrap();
    let mut raw = ekg_core::Episode::new("caller supplied row");
    raw.action = Some("procedure:deadbeef@1".into());
    raw.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: true,
        details: "untrusted raw row".into(),
        surprise: None,
    });
    engine.admin_insert_episode(&raw).unwrap();
    let candidate = ekg_engine::SkillCandidate {
        name: "raw".into(),
        source_episode_ids: vec![raw.id],
        support_count: 1,
        rationale: "must not be admitted".into(),
        failure_critic: false,
    };
    assert!(engine.register_skill_candidate(&candidate).is_err());
}

#[test]
fn compression_materializes_summaries_without_deleting_source_episodes() {
    let engine = Engine::in_memory_with_admin("compression-test").unwrap();
    let procedure = double();
    engine.admin_insert_procedure(&procedure).unwrap();
    for value in 1..=4 {
        engine
            .execute_procedure(procedure.id, inputs(value), Some(Value::Int(value * 2)))
            .unwrap();
    }
    let before = engine.episodes().list_recent(32).unwrap().len();
    assert_eq!(engine.list_verified_answers(32).unwrap().len(), before);
    let result = engine.compress_episode_history(32).unwrap();
    assert!(!result.plan.summarize.is_empty());
    assert_eq!(
        result.archived_episode_ids.len(),
        result.plan.summarize.len()
    );
    assert_eq!(engine.episodes().list_recent(32).unwrap().len(), before);
    assert_eq!(
        engine.list_episode_compression_records(32).unwrap().len(),
        result.archived_episode_ids.len()
    );
    assert_eq!(
        result.plan.forgotten_as_known_gap, result.plan.summarize,
        "summarized episodes must remain explicit known gaps"
    );
    let record = engine
        .list_episode_compression_records(32)
        .unwrap()
        .pop()
        .unwrap();
    let gaps = record.summary["knownGaps"].as_array().unwrap();
    assert!(
        gaps.iter()
            .any(|gap| gap.as_str().unwrap().contains("execution trace"))
    );
    assert!(
        gaps.iter()
            .any(|gap| gap.as_str().unwrap().contains("behavioral subsumption"))
    );
    assert!(!record.archived_episode["execution_trace"].is_null());
}
