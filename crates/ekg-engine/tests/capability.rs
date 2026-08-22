use ekg_engine::{
    CapabilityStatus, DiscoveredOperation, Engine, InterfaceDescription, LocalValidation,
    Permission,
};
use ekg_core::{Evaluation, Value, VerifiabilityTier};
use std::collections::BTreeMap;

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
            Value::Map(BTreeMap::from([(String::from("temperature"), Value::Int(72))])),
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
