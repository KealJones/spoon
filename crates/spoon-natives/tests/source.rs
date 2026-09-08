//! Source Concepts are ordinary persisted concepts with composed execution.

use spoon_concept::{Concept, Effect, Ground, Provenance, RealizationSpec, SymbolId};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome, PermissionMode};
use spoon_natives::{bootstrap, seed_bootstrap};
use spoon_store::Store;

fn brain() -> (Store, NativeRegistry) {
    let store = Store::open_in_memory().unwrap();
    let registry = bootstrap();
    seed_bootstrap(&store, &registry).unwrap();
    (store, registry)
}

fn run(store: &Store, registry: &NativeRegistry, concept: &Concept) -> Outcome {
    Evaluator::new(store, registry)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass)
        .evaluate(concept)
}

#[test]
fn source_concepts_and_composed_pipeline_are_seeded() {
    let (store, _) = brain();
    let wikidata = Concept::named("wikidata");
    let research = Concept::named("research-search");

    assert!(store.get_meta(&wikidata).unwrap().is_some());
    assert!(
        store
            .holds(&Concept::call("source", [wikidata.clone()]))
            .unwrap()
    );
    assert!(
        store
            .holds(&Concept::call(
                "supports",
                [wikidata.clone(), Concept::named("entity-search")],
            ))
            .unwrap()
    );

    let realizations = store.realizations_for(&research).unwrap();
    assert_eq!(realizations.len(), 1);
    let realization = &realizations[0];
    assert_eq!(realization.effect, Effect::Network);
    assert!(matches!(realization.spec, RealizationSpec::Composed { .. }));
}

#[test]
fn wikidata_query_url_is_a_real_concept_stage() {
    let (store, registry) = brain();
    let query = Concept::call(
        "source-query-url",
        [
            Concept::named("wikidata"),
            Concept::text("Ada Lovelace & friends"),
        ],
    );
    let outcome = run(&store, &registry, &query);
    let Outcome::Value(url) = outcome else {
        panic!("expected URL, got {outcome:?}");
    };
    let url = url.as_ground().and_then(Ground::as_str).unwrap();
    assert!(
        url.contains("search=Ada%20Lovelace%20%26%20friends"),
        "{url}"
    );
    assert!(url.contains("format=json"), "{url}");
}

#[test]
fn unsupported_source_is_rejected_before_network() {
    let (store, registry) = brain();
    let query = Concept::call(
        "source-query-url",
        [Concept::named("unknown-source"), Concept::text("anything")],
    );
    let mut evaluator = Evaluator::new(&store, &registry)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass);
    let outcome = evaluator.evaluate(&query);
    assert!(!outcome.is_value(), "unexpected {outcome:?}");
    assert!(
        evaluator
            .trace()
            .failures()
            .iter()
            .any(|(_, _, message)| message.contains("unsupported source"))
    );
}

#[test]
fn user_registered_source_is_persisted_with_an_env_credential_reference() {
    let (store, registry) = brain();
    let operation = Concept::call(
        "source-register",
        [
            Concept::text("weather-api"),
            Concept::text("https://example.test/weather?city={query}"),
            Concept::text("WEATHER_API_KEY"),
        ],
    );
    let outcome = run(&store, &registry, &operation);
    let Outcome::Value(source) = outcome else {
        panic!("expected source, got {outcome:?}");
    };
    assert_eq!(source, Concept::named("weather-api"));
    assert!(
        store
            .holds(&Concept::call("source", [source.clone()]))
            .unwrap()
    );
    assert!(
        store
            .holds(&Concept::call(
                "source-endpoint",
                [
                    source.clone(),
                    Concept::text("https://example.test/weather?city={query}")
                ],
            ))
            .unwrap()
    );
    assert!(
        store
            .holds(&Concept::call(
                "credential-ref",
                [source.clone(), Concept::text("WEATHER_API_KEY")],
            ))
            .unwrap()
    );

    let url = run(
        &store,
        &registry,
        &Concept::call("source-query-url", [source, Concept::text("Phoenix, AZ")]),
    );
    let Outcome::Value(url) = url else {
        panic!("expected URL, got {url:?}");
    };
    assert_eq!(
        url.as_ground().and_then(Ground::as_str),
        Some("https://example.test/weather?city=Phoenix%2C%20AZ"),
    );
}

#[test]
fn evidence_stage_persists_external_payload_and_returns_result() {
    let (store, registry) = brain();
    let source = Concept::named("wikidata");
    let query = Concept::text("Ada Lovelace");
    let payload = Concept::json(serde_json::json!({"search": [{"id": "Q7259"}]}));
    let operation = Concept::call(
        "source-evidence",
        [source.clone(), query.clone(), payload.clone()],
    );

    let outcome = run(&store, &registry, &operation);
    let Outcome::Value(result) = outcome else {
        panic!("expected result, got {outcome:?}");
    };
    assert_eq!(result.head_symbol(), Some(SymbolId::of("research-result")));
    assert_eq!(result.arg(0), Some(&source));
    assert_eq!(result.arg(1), Some(&query));
    let evidence = result.arg(2).expect("evidence payload");
    assert_eq!(evidence.head_symbol(), Some(SymbolId::of("evidence")));
    assert!(store.holds(evidence).unwrap());
    let record = store.live_assertions(evidence).unwrap().pop().unwrap();
    assert_eq!(
        record.provenance,
        Provenance::External {
            source: "wikidata".into()
        }
    );
}

#[test]
fn source_metadata_and_evidence_survive_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("brain.db");
    {
        let store = Store::open(&path).unwrap();
        let registry = bootstrap();
        seed_bootstrap(&store, &registry).unwrap();
        let evidence = Concept::call(
            "evidence",
            [
                Concept::named("wikidata"),
                Concept::text("Ada Lovelace"),
                Concept::json(serde_json::json!({"ok": true})),
            ],
        );
        store
            .assert_concept(
                &evidence,
                Provenance::External {
                    source: "wikidata".into(),
                },
                None,
                Some(1.0),
            )
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert!(
        store
            .get_meta(&Concept::named("wikidata"))
            .unwrap()
            .is_some()
    );
    let evidence = Concept::call(
        "evidence",
        [
            Concept::named("wikidata"),
            Concept::text("Ada Lovelace"),
            Concept::json(serde_json::json!({"ok": true})),
        ],
    );
    assert!(store.holds(&evidence).unwrap());
    assert_eq!(store.live_assertions(&evidence).unwrap().len(), 1);
}
