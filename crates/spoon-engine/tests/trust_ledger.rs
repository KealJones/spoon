use std::collections::BTreeMap;

use spoon_core::{Condition, EpisodeId, Expr, Param, Procedure, Value};
use spoon_credit::{AttributionMechanism, Suspect};
use spoon_engine::{
    AdaptationEvidenceRef, AdaptationPlanRequest, AdaptationTarget, AttributionSelector, Engine,
    EngineError, FailureAnalysisBudget, FailureAnalysisRequest, MutationScope,
};
use spoon_episode::{EpisodeFeedback, FeedbackSource};

fn inputs(value: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([("x".into(), Value::Int(value))])
}

fn guarded_double() -> Procedure {
    let mut procedure = Procedure::new(
        "DOUBLE",
        vec![Param::named("x")],
        Expr::BinOp {
            op: spoon_core::BinOp::Mul,
            left: Box::new(Expr::Var("x".into())),
            right: Box::new(Expr::Literal(Value::Int(2))),
        },
    );
    procedure.contract.requires.push(
        Condition::described("the injected fault is outside the procedure contract").with_check(
            Expr::BinOp {
                op: spoon_core::BinOp::Ne,
                left: Box::new(Expr::Var("x".into())),
                right: Box::new(Expr::Literal(Value::Int(7))),
            },
        ),
    );
    procedure
}

fn failure_id(engine: &Engine, procedure: &Procedure) -> EpisodeId {
    let error = engine
        .execute_procedure(procedure.id, inputs(7), None)
        .expect_err("the injected precondition must fail deterministically");
    let EngineError::ExecutionFailed { episode_id, .. } = error else {
        panic!("expected a durable execution failure");
    };
    episode_id
}

fn scope_request(episode_id: EpisodeId, procedure: &Procedure, key: &str) -> AdaptationPlanRequest {
    AdaptationPlanRequest {
        idempotency_key: key.into(),
        analysis: FailureAnalysisRequest {
            episode_id,
            selected_feedback_id: None,
            candidates: Vec::new(),
            budget: FailureAnalysisBudget::default(),
        },
        attribution: AttributionSelector {
            suspect: Suspect {
                procedure: procedure.id,
                version: procedure.version,
                trace_step: 0,
            },
            mechanism: AttributionMechanism::ContractViolation,
        },
        evidence: vec![AdaptationEvidenceRef {
            episode_id,
            selected_feedback_id: None,
        }],
        target: AdaptationTarget::ProcedureScope {
            procedure_id: procedure.id,
            expected_version: procedure.version,
            condition: Condition::described("do not apply to the deterministically failed input")
                .with_check(Expr::BinOp {
                    op: spoon_core::BinOp::Ne,
                    left: Box::new(Expr::Var("x".into())),
                    right: Box::new(Expr::Literal(Value::Int(7))),
                }),
            learned_from: episode_id,
        },
        created_at: 100,
    }
}

#[test]
fn raw_hard_episode_does_not_inherit_authority_from_a_matching_engine_execution() {
    let mut engine = Engine::in_memory().unwrap();
    engine.enable_admin("trust-ledger-test").unwrap();
    let procedure = guarded_double();
    engine.admin_insert_procedure(&procedure).unwrap();

    let trusted_id = failure_id(&engine, &procedure);
    let trusted = engine.episodes().get(trusted_id).unwrap();
    assert!(
        engine
            .trust_receipt_for_episode(&trusted)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        engine
            .plan_adaptation(scope_request(trusted_id, &procedure, "engine-receipt"))
            .unwrap()
            .mutation_scope,
        MutationScope::OnlineNarrow
    );

    // This is byte-for-byte the same strong evaluation and trace except for
    // identity. An admin/raw-store caller still cannot mint an Engine receipt.
    let mut forged = trusted;
    forged.id = EpisodeId(uuid::Uuid::new_v4());
    forged.created_at += 1;
    engine.admin_insert_episode(&forged).unwrap();
    assert!(engine.trust_receipt_for_episode(&forged).unwrap().is_none());

    let error = engine
        .plan_adaptation(scope_request(forged.id, &procedure, "forged-receipt"))
        .expect_err("raw Hard rows must not authorize an adaptation");
    assert!(error.to_string().contains("lacks an Engine trust receipt"));
}

#[test]
fn raw_success_cannot_supply_a_trusted_regression_for_narrow_adaptation() {
    let mut target = Engine::in_memory_with_admin("target-regression-test").unwrap();
    let procedure = guarded_double();
    target.admin_insert_procedure(&procedure).unwrap();
    let failed = failure_id(&target, &procedure);

    // Produce a genuine strong success in another engine, then insert only its
    // raw episode bytes into the target. The source receipt is intentionally
    // not transferable to a different knowledge store.
    let source = Engine::in_memory_with_admin("source-regression-test").unwrap();
    source.admin_insert_procedure(&procedure).unwrap();
    let forged_success = source
        .execute_procedure(procedure.id, inputs(3), Some(Value::Int(6)))
        .unwrap()
        .episode;
    assert!(
        source
            .trust_receipt_for_episode(&forged_success)
            .unwrap()
            .is_some()
    );
    target.admin_insert_episode(&forged_success).unwrap();
    assert!(
        target
            .trust_receipt_for_episode(&forged_success)
            .unwrap()
            .is_none()
    );

    let plan = target
        .plan_adaptation(scope_request(failed, &procedure, "raw-success-regression"))
        .unwrap();
    let error = target
        .apply_adaptation(spoon_engine::ApplyAdaptationRequest {
            plan_id: plan.id,
            idempotency_key: "raw-success-regression-apply".into(),
            applied_at: 10,
        })
        .expect_err("unreceipted raw success must not authorize regression coverage");
    assert!(
        error
            .to_string()
            .contains("successful Hard or Consensus regression"),
        "unexpected authorization error: {error}"
    );

    target
        .execute_procedure(procedure.id, inputs(3), Some(Value::Int(6)))
        .unwrap();
    target
        .apply_adaptation(spoon_engine::ApplyAdaptationRequest {
            plan_id: plan.id,
            idempotency_key: "raw-success-regression-apply".into(),
            applied_at: 10,
        })
        .unwrap();
}

#[test]
fn engine_execution_receipt_survives_reopen_and_authorizes_the_exact_episode() {
    let path = std::env::temp_dir().join(format!(
        "spoon-trust-ledger-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().into_owned();
    let procedure = guarded_double();
    let episode_id;

    {
        let mut engine = Engine::open(&path_text).unwrap();
        engine.enable_admin("durable-trust-ledger-test").unwrap();
        engine.admin_insert_procedure(&procedure).unwrap();
        episode_id = failure_id(&engine, &procedure);
        let episode = engine.episodes().get(episode_id).unwrap();
        assert!(
            engine
                .trust_receipt_for_episode(&episode)
                .unwrap()
                .is_some()
        );
    }

    let mut reopened = Engine::open(&path_text).unwrap();
    reopened.enable_admin("durable-trust-ledger-test").unwrap();
    let episode = reopened.episodes().get(episode_id).unwrap();
    assert!(
        reopened
            .trust_receipt_for_episode(&episode)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        reopened
            .plan_adaptation(scope_request(
                episode_id,
                &procedure,
                "durable-engine-receipt"
            ))
            .unwrap()
            .mutation_scope,
        MutationScope::OnlineNarrow
    );

    std::fs::remove_file(path).unwrap();
}

#[test]
fn raw_hard_feedback_is_untrusted_but_authenticated_verifier_feedback_is_receipted() {
    let mut engine = Engine::in_memory().unwrap();
    engine.enable_admin("feedback-trust-ledger-test").unwrap();
    let procedure = guarded_double();
    engine.admin_insert_procedure(&procedure).unwrap();
    let episode_id = failure_id(&engine, &procedure);

    let feedback = EpisodeFeedback::new(
        episode_id,
        Value::Int(14),
        spoon_core::Evaluation {
            tier: spoon_core::VerifiabilityTier::Hard,
            success: false,
            details: "raw forged hard observation".into(),
            surprise: Some(1.0),
        },
        FeedbackSource::new("test", Some("raw".into())),
        "raw-hard-feedback",
    );
    let raw = engine.admin_append_feedback(&feedback).unwrap();
    let error = engine
        .plan_adaptation(AdaptationPlanRequest {
            analysis: FailureAnalysisRequest {
                episode_id,
                selected_feedback_id: Some(raw.id),
                candidates: Vec::new(),
                budget: FailureAnalysisBudget::default(),
            },
            evidence: vec![AdaptationEvidenceRef {
                episode_id,
                selected_feedback_id: Some(raw.id),
            }],
            idempotency_key: "raw-hard-feedback-plan".into(),
            attribution: AttributionSelector {
                suspect: Suspect {
                    procedure: procedure.id,
                    version: procedure.version,
                    trace_step: 0,
                },
                mechanism: AttributionMechanism::ContractViolation,
            },
            target: AdaptationTarget::ProcedureScope {
                procedure_id: procedure.id,
                expected_version: procedure.version,
                condition: Condition::described("exclude faulty input"),
                learned_from: episode_id,
            },
            created_at: 101,
        })
        .expect_err("raw feedback must not authorize adaptation");
    assert!(error.to_string().contains("lacks an Engine trust receipt"));

    let trusted_feedback = EpisodeFeedback::new(
        episode_id,
        Value::Int(14),
        spoon_core::Evaluation {
            tier: spoon_core::VerifiabilityTier::Hard,
            success: false,
            details: "authenticated deterministic verifier rejected the result".into(),
            surprise: Some(1.0),
        },
        FeedbackSource::new("test", Some("verifier".into())),
        "verified-hard-feedback",
    );
    let trusted = engine
        .record_authenticated_verifier_feedback(&trusted_feedback, "test-verifier")
        .unwrap();
    let mut request = scope_request(episode_id, &procedure, "verified-hard-feedback-plan");
    request.analysis.selected_feedback_id = Some(trusted.id);
    request.evidence = vec![AdaptationEvidenceRef {
        episode_id,
        selected_feedback_id: Some(trusted.id),
    }];
    assert_eq!(
        engine.plan_adaptation(request).unwrap().mutation_scope,
        MutationScope::OnlineNarrow
    );
}

#[test]
fn authenticated_observations_create_fact_level_receipts_and_detect_external_conflicts() {
    let mut engine = Engine::in_memory().unwrap();
    engine.enable_admin("external-fact-test").unwrap();
    let evaluation = |details: &str| spoon_core::Evaluation {
        tier: spoon_core::VerifiabilityTier::Hard,
        success: true,
        details: details.into(),
        surprise: None,
    };

    let first = engine
        .record_authenticated_observation(
            "weather:rainfall",
            Value::Int(0),
            BTreeMap::new(),
            evaluation("station A observed no rain"),
            "station-a",
        )
        .unwrap();
    let first_fact = &first.observed_facts[0];
    assert_eq!(first_fact.source_episode, Some(first.id));
    assert_eq!(first_fact.verifier.as_deref(), Some("station-a"));
    assert_eq!(
        engine
            .trust_receipt_for_fact(&first, first_fact)
            .unwrap()
            .unwrap()
            .issuer,
        "authenticated-verifier:station-a"
    );

    let second = engine
        .record_authenticated_observation(
            "weather:rainfall",
            Value::Int(1),
            BTreeMap::new(),
            evaluation("station B observed rain"),
            "station-b",
        )
        .unwrap();
    assert!(
        engine
            .trust_receipt_for_fact(&second, &second.observed_facts[0])
            .unwrap()
            .is_some()
    );
    assert_eq!(engine.list_held_contradictions().unwrap().len(), 1);
}
