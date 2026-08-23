use spoon_adapt::{Claim, Implication};
use spoon_core::{Concept, MutabilityClass, Value};
use spoon_engine::{AdaptationPlanId, ApplyAdaptationRequest, Engine, EngineError};

#[test]
fn ordinary_engine_handles_expose_reads_but_reject_raw_mutation() {
    let engine = Engine::in_memory().unwrap();
    let concept = Concept::new("untrusted seed", MutabilityClass::DefeasibleGeneral);

    let error = engine.admin_insert_concept(&concept).unwrap_err();
    assert!(matches!(error, EngineError::InvalidInput(_)));
    assert!(engine.graph().list_concepts().unwrap().is_empty());
    assert_eq!(engine.episodes().count().unwrap(), 0);

    let left = Claim::new(
        "left",
        "left",
        Implication::new("predicate", Value::Bool(true)),
        Vec::new(),
    );
    let right = Claim::new(
        "right",
        "right",
        Implication::new("predicate", Value::Bool(false)),
        Vec::new(),
    );
    assert!(engine.admin_record_contradiction(left, right, 1).is_err());
    assert!(engine.admin_add_claim_dependency("left", "right").is_err());

    let mut engine = engine;
    assert!(
        engine
            .issue_offline_capability(&ApplyAdaptationRequest {
                plan_id: AdaptationPlanId(uuid::Uuid::nil()),
                idempotency_key: "unauthorized-maintenance".into(),
                applied_at: 1,
            })
            .is_err()
    );
}

#[test]
fn durable_admin_authority_requires_the_exact_local_secret_after_reopen() {
    let path = std::env::temp_dir().join(format!(
        "spoon-admin-boundary-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let path_text = path.to_string_lossy().into_owned();
    let concept = Concept::new("authorized seed", MutabilityClass::DefeasibleGeneral);

    {
        let engine = Engine::open_with_admin(&path_text, "correct-local-secret").unwrap();
        engine.admin_insert_concept(&concept).unwrap();
    }

    let ordinary = Engine::open(&path_text).unwrap();
    let stored = ordinary.graph().list_concepts().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, concept.id);
    assert_eq!(stored[0].name, concept.name);
    assert!(ordinary.admin_delete_concept(concept.id).is_err());
    drop(ordinary);

    let mismatch = match Engine::open_with_admin(&path_text, "different-secret") {
        Ok(_) => panic!("a different secret must not inherit durable admin authority"),
        Err(error) => error,
    };
    assert!(matches!(mismatch, EngineError::InvalidInput(_)));

    let authorized = Engine::open_with_admin(&path_text, "correct-local-secret").unwrap();
    authorized.admin_delete_concept(concept.id).unwrap();
    assert!(authorized.graph().list_concepts().unwrap().is_empty());
    drop(authorized);
    std::fs::remove_file(path).unwrap();
}
