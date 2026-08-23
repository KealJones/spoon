use ekg_core::{Evaluation, Value, VerifiabilityTier};
use ekg_engine::{
    AdapterExecution, AuthorizedPrimitiveInvocation, CapabilityInvocationAdapter, CapabilityStatus,
    DiscoveredOperation, Effect, Engine, EngineError, InterfaceDescription, LocalValidation,
    Permission, PrimitivePolicy, ResourceUsage,
};
use std::collections::{BTreeMap, BTreeSet};

struct DeterministicFixtureAdapter {
    calls: usize,
    expected_content_id: String,
}

impl CapabilityInvocationAdapter for DeterministicFixtureAdapter {
    fn execute(
        &mut self,
        invocation: &AuthorizedPrimitiveInvocation,
    ) -> Result<AdapterExecution, ekg_engine::CapabilityError> {
        self.calls += 1;
        assert_eq!(invocation.effect, Effect::Network);
        assert_eq!(invocation.content_id, self.expected_content_id);
        assert_eq!(
            invocation.input,
            serde_json::json!({"query": "today", "apiSecret": "do-not-persist"})
        );
        Ok(AdapterExecution {
            effect: Effect::Network,
            output: serde_json::json!({"temperature": 72}),
            usage: ResourceUsage {
                bytes: 64,
                steps: 1,
                millis: 0,
            },
        })
    }
}

#[test]
fn engine_keeps_imported_capabilities_quarantined_until_local_validation_and_grant() {
    let engine = Engine::in_memory_with_admin("capability-admin").unwrap();
    let bundle = engine
        .discover_capability(&InterfaceDescription {
            source: "weather-api".into(),
            fingerprint: "weather-v1".into(),
            operations: vec![DiscoveredOperation {
                name: "forecast".into(),
                input_schema: serde_json::json!({"type":"object"}),
                output_schema: serde_json::json!({"type":"object"}),
                host: "api.example.test".into(),
                method: "GET".into(),
                response_fixture: serde_json::json!({"temperature":72}),
            }],
        })
        .unwrap();
    let bytes = ekg_engine::export_bundle(&bundle).unwrap();
    let imported = engine.import_capability_bundle(&bytes).unwrap();
    assert_eq!(imported.status, CapabilityStatus::Quarantined);
    assert!(
        engine
            .require_capability_permissions(&imported.content_id, &bundle.procedures[0].permissions)
            .is_err()
    );
    let validation_episode = engine
        .record_authenticated_observation(
            "weather.forecast",
            Value::Map(BTreeMap::from([(
                String::from("temperature"),
                Value::Int(72),
            )])),
            BTreeMap::new(),
            Evaluation {
                tier: VerifiabilityTier::Hard,
                success: true,
                details: "fixture matched".into(),
                surprise: Some(0.0),
            },
            "local-capability-test",
        )
        .unwrap();
    let validated = engine
        .revalidate_capability(
            &imported.content_id,
            &LocalValidation {
                passed: true,
                validation_episodes: vec![validation_episode.id.to_string()],
                environment_digest: "local".into(),
            },
        )
        .unwrap();
    assert_eq!(validated.status, CapabilityStatus::Provisional);
    let permission = Permission::NetworkHost {
        host: "api.example.test".into(),
    };
    engine
        .grant_capability_permission(&imported.content_id, &permission)
        .unwrap();
    engine
        .require_capability_permissions(&imported.content_id, &[permission])
        .unwrap();
    let authorized = engine
        .require_capability_procedure(&imported.content_id, &bundle.procedures[0].id)
        .unwrap();
    assert_eq!(authorized.id, bundle.procedures[0].id);
    assert_eq!(
        engine
            .export_capability_bundle(&imported.content_id)
            .unwrap(),
        bytes
    );
}

#[test]
fn engine_capability_invocation_is_explicit_redacted_and_revocation_is_immediate() {
    let engine = Engine::in_memory_with_admin("capability-admin").unwrap();
    let bundle = engine
        .discover_capability(&InterfaceDescription {
            source: "weather-api".into(),
            fingerprint: "weather-v1".into(),
            operations: vec![DiscoveredOperation {
                name: "forecast".into(),
                input_schema: serde_json::json!({"type":"object"}),
                output_schema: serde_json::json!({"type":"object"}),
                host: "api.example.test".into(),
                method: "GET".into(),
                response_fixture: serde_json::json!({"temperature":72}),
            }],
        })
        .unwrap();
    let imported = engine
        .import_capability_bundle(&ekg_engine::export_bundle(&bundle).unwrap())
        .unwrap();
    let validation_episode = engine
        .record_authenticated_observation(
            "weather.fixture",
            Value::Map(BTreeMap::from([(
                String::from("temperature"),
                Value::Int(72),
            )])),
            BTreeMap::new(),
            Evaluation {
                tier: VerifiabilityTier::Hard,
                success: true,
                details: "local fixture matched".into(),
                surprise: Some(0.0),
            },
            "local-capability-test",
        )
        .unwrap();
    let validated = engine
        .revalidate_capability(
            &imported.content_id,
            &LocalValidation {
                passed: true,
                validation_episodes: vec![validation_episode.id.to_string()],
                environment_digest: "sha256:local-fixture-environment".into(),
            },
        )
        .unwrap();
    assert_eq!(validated.status, CapabilityStatus::Provisional);

    let permission = Permission::NetworkHost {
        host: "api.example.test".into(),
    };
    engine
        .grant_capability_permission(&validated.content_id, &permission)
        .unwrap();
    let policy = PrimitivePolicy {
        network_hosts: BTreeSet::from(["api.example.test".into()]),
        bounds: bundle.procedures[0].bounds.clone(),
        ..PrimitivePolicy::default()
    };
    let mut adapter = DeterministicFixtureAdapter {
        calls: 0,
        expected_content_id: validated.content_id.clone(),
    };
    let input = serde_json::json!({"query": "today", "apiSecret": "do-not-persist"});
    let outcome = engine
        .invoke_capability(
            &validated.content_id,
            &bundle.procedures[0].id,
            &input,
            Some(&serde_json::json!({"temperature": 72})),
            &policy,
            &mut adapter,
        )
        .unwrap();
    assert_eq!(adapter.calls, 1);
    assert_eq!(
        outcome.invocation.output,
        serde_json::json!({"temperature": 72})
    );
    assert!(outcome.episode.succeeded());
    assert_eq!(
        outcome
            .episode
            .observed_result
            .as_ref()
            .unwrap()
            .as_map()
            .unwrap()["redacted"],
        Value::Bool(true)
    );
    let durable = engine.episodes().get(outcome.episode.id).unwrap();
    let durable_json = serde_json::to_string(&durable).unwrap();
    assert!(!durable_json.contains("temperature"));
    assert!(!durable_json.contains("do-not-persist"));
    assert!(durable_json.contains("capability_invocation"));
    assert!(durable_json.contains("network_host"));
    assert!(durable_json.contains("usage"));
    assert!(durable_json.contains("receipt"));
    assert!(input["apiSecret"].is_string());

    engine
        .revoke_capability_permission(&validated.content_id, &permission)
        .unwrap();
    let error = engine
        .invoke_capability(
            &validated.content_id,
            &bundle.procedures[0].id,
            &input,
            Some(&serde_json::json!({"temperature": 72})),
            &policy,
            &mut adapter,
        )
        .unwrap_err();
    let EngineError::CapabilityInvocationFailed { episode_id, .. } = error else {
        panic!("expected a persisted capability invocation failure");
    };
    assert_eq!(
        adapter.calls, 1,
        "revocation must reject before the adapter"
    );
    let failed = engine.episodes().get(episode_id).unwrap();
    assert!(failed.failed());
    assert!(failed.action.as_ref().unwrap().ends_with(":failed"));
    assert!(
        !serde_json::to_string(&failed)
            .unwrap()
            .contains("do-not-persist")
    );
    assert!(
        engine
            .require_capability_procedure(&validated.content_id, &bundle.procedures[0].id)
            .is_err()
    );
}

#[test]
fn capability_bundle_round_trips_into_a_clean_instance_before_invocation() {
    // Discovery and export belong to the source instance; no trust, grant, or
    // environment assumption is allowed to hitch a ride with the bundle.
    let source = Engine::in_memory_with_admin("capability-source").unwrap();
    let bundle = source
        .discover_capability(&InterfaceDescription {
            source: "clean-instance-api".into(),
            fingerprint: "clean-instance-v1".into(),
            operations: vec![DiscoveredOperation {
                name: "lookup".into(),
                input_schema: serde_json::json!({"type":"object"}),
                output_schema: serde_json::json!({"type":"object"}),
                host: "api.example.test".into(),
                method: "GET".into(),
                response_fixture: serde_json::json!({"temperature":72}),
            }],
        })
        .unwrap();
    let bytes = ekg_engine::export_bundle(&bundle).unwrap();

    let clean = Engine::in_memory_with_admin("capability-clean-instance").unwrap();
    let imported = clean.import_capability_bundle(&bytes).unwrap();
    assert_eq!(imported.status, CapabilityStatus::Quarantined);
    let validation_episode = clean
        .record_authenticated_observation(
            "clean.lookup",
            Value::Map(BTreeMap::from([(
                String::from("temperature"),
                Value::Int(72),
            )])),
            BTreeMap::new(),
            Evaluation {
                tier: VerifiabilityTier::Hard,
                success: true,
                details: "clean local fixture matched".into(),
                surprise: Some(0.0),
            },
            "clean-instance-validation",
        )
        .unwrap();
    let validated = clean
        .revalidate_capability(
            &imported.content_id,
            &LocalValidation {
                passed: true,
                validation_episodes: vec![validation_episode.id.to_string()],
                environment_digest: "clean-instance".into(),
            },
        )
        .unwrap();
    assert_eq!(validated.status, CapabilityStatus::Provisional);
    let permission = Permission::NetworkHost {
        host: "api.example.test".into(),
    };
    clean
        .grant_capability_permission(&validated.content_id, &permission)
        .unwrap();
    let policy = PrimitivePolicy {
        network_hosts: BTreeSet::from(["api.example.test".into()]),
        bounds: bundle.procedures[0].bounds.clone(),
        ..PrimitivePolicy::default()
    };
    let mut adapter = DeterministicFixtureAdapter {
        calls: 0,
        expected_content_id: validated.content_id.clone(),
    };
    let outcome = clean
        .invoke_capability(
            &validated.content_id,
            &bundle.procedures[0].id,
            &serde_json::json!({"query":"today","apiSecret":"do-not-persist"}),
            Some(&serde_json::json!({"temperature":72})),
            &policy,
            &mut adapter,
        )
        .unwrap();
    assert!(outcome.episode.succeeded());
    assert_eq!(adapter.calls, 1);
    assert!(
        outcome
            .episode
            .action
            .as_deref()
            .is_some_and(|action| action.ends_with(":invoked"))
    );
}
