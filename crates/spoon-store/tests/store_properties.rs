//! Orchestrator review: the store properties the rest of the system relies on.
//!
//! Deliberately not a re-run of the implementer's tests. These assert the
//! architectural guarantees: ground concepts stay free until they earn a row,
//! history is never destroyed, and a brain reloads as exactly what was written.

use chrono::{Duration, TimeZone, Utc};
use spoon_concept::{
    Activation, Concept, ConceptMeta, Effect, NativeId, Provenance, Realization, RealizationSpec,
    SymbolId, Tier,
};
use spoon_store::Store;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

fn meta_for(c: &Concept, forms: &[&str]) -> ConceptMeta {
    ConceptMeta::new(c.clone(), Provenance::Bootstrap, Tier::Kernel, now())
        .with_surface_forms(forms.iter().copied())
}

// ---------------------------------------------------------------------------
// The core design property: ground concepts are free until asserted about
// ---------------------------------------------------------------------------

#[test]
fn arithmetic_over_ground_values_writes_no_rows_for_them() {
    // Add<42, 1> is the shape every computation produces. If each literal in it
    // earned a database row, a few minutes of arithmetic would bury the store
    // in integers. Ground identity is self-describing precisely so this is free.
    let store = Store::open_in_memory().unwrap();
    let expr = Concept::call("math-add", [Concept::int(42), Concept::int(1)]);
    store.put_concept(&expr).unwrap();

    // The compound and its named head are worth storing. The two integers are
    // not, and must not have rows of their own.
    assert!(!store.has_concept(Concept::int(42).content_id()).unwrap());
    assert!(!store.has_concept(Concept::int(1).content_id()).unwrap());
    assert!(store.has_concept(expr.content_id()).unwrap());
}

#[test]
fn a_ground_value_materializes_once_something_is_said_about_it() {
    // This is how #160 becomes a real pull request concept without every
    // integer in every computation becoming one too.
    let store = Store::open_in_memory().unwrap();
    let forty_two = Concept::int(42);

    assert!(!store.has_concept(forty_two.content_id()).unwrap());

    store
        .put_meta(&meta_for(&forty_two, &["forty-two", "the answer"]))
        .unwrap();

    assert!(store.has_concept(forty_two.content_id()).unwrap());
    let found = store.surface_lookup("forty-two").unwrap();
    assert_eq!(found, vec![forty_two.clone()]);
    // Lookup is case-insensitive: the ears see whatever casing the user typed.
    assert_eq!(store.surface_lookup("Forty-Two").unwrap(), vec![forty_two]);
}

#[test]
fn asserting_about_a_ground_value_also_materializes_it() {
    let store = Store::open_in_memory().unwrap();
    let pr = Concept::int(160);
    assert!(!store.has_concept(pr.content_id()).unwrap());
    store
        .assert_concept(
            &Concept::call("pull-request", [pr.clone()]),
            Provenance::Bootstrap,
            None,
            None,
        )
        .unwrap();
    // The compound was asserted, so it exists. 160 appears inside it, which is
    // enough to find it, but is not itself the subject of a claim yet.
    let containing = store.concepts_containing(pr.content_id(), 10).unwrap();
    assert_eq!(
        containing.len(),
        1,
        "160 should be findable through its container"
    );

    store
        .assert_concept(&pr, Provenance::Bootstrap, None, None)
        .unwrap();
    assert!(
        store.has_concept(pr.content_id()).unwrap(),
        "asserting about 160 should give it a row"
    );
}

#[test]
fn ground_participants_are_indexed_even_without_rows_of_their_own() {
    let store = Store::open_in_memory().unwrap();
    let a = Concept::call("score", [Concept::named("probe-a"), Concept::int(7)]);
    let b = Concept::call("score", [Concept::named("probe-b"), Concept::int(7)]);
    store.put_concept(&a).unwrap();
    store.put_concept(&b).unwrap();

    let sevens = store
        .concepts_containing(Concept::int(7).content_id(), 10)
        .unwrap();
    assert_eq!(sevens.len(), 2, "both scores mention 7");
    assert!(!store.has_concept(Concept::int(7).content_id()).unwrap());
}

// ---------------------------------------------------------------------------
// Indexes answer the two questions the design doc names
// ---------------------------------------------------------------------------

#[test]
fn find_all_friendships_and_find_everything_about_greg() {
    let store = Store::open_in_memory().unwrap();
    let greg = Concept::named("greg");
    let pairs = [("greg", "keal"), ("greg", "syd"), ("emmy", "syd")];
    for (a, b) in pairs {
        store
            .put_concept(&Concept::call(
                "friend-with",
                [Concept::named(a), Concept::named(b)],
            ))
            .unwrap();
    }
    store
        .put_concept(&Concept::call(
            "employment",
            [greg.clone(), Concept::named("workiva")],
        ))
        .unwrap();

    let friendships = store
        .concepts_by_head(SymbolId::of("friend-with"), 100)
        .unwrap();
    assert_eq!(friendships.len(), 3);

    // Everything about Greg spans relationship types: two friendships and a job.
    let about_greg = store.concepts_containing(greg.content_id(), 100).unwrap();
    assert_eq!(about_greg.len(), 3, "got {about_greg:?}");
}

#[test]
fn nested_participation_is_found_at_any_depth() {
    // Stated<Keal, IsSad<Greg>>: Greg is two levels down, and a question about
    // Greg still has to surface it.
    let store = Store::open_in_memory().unwrap();
    let nested = Concept::call(
        "stated",
        [
            Concept::named("keal"),
            Concept::call("is-sad", [Concept::named("greg")]),
        ],
    );
    store.put_concept(&nested).unwrap();

    let about_greg = store
        .concepts_containing(Concept::named("greg").content_id(), 10)
        .unwrap();
    assert!(
        about_greg
            .iter()
            .any(|c| c.content_id() == nested.content_id()),
        "nested mention was not indexed: {about_greg:?}"
    );
    let back = store.get_concept(nested.content_id()).unwrap().unwrap();
    assert_eq!(back, nested);
}

// ---------------------------------------------------------------------------
// History is never destroyed
// ---------------------------------------------------------------------------

#[test]
fn retraction_hides_a_claim_without_erasing_that_it_was_believed() {
    // Something that was true does not become false merely because it is no
    // longer true now. Retraction has to be invisible to "what holds today" and
    // fully visible to "what did you think last Tuesday".
    let store = Store::open_in_memory().unwrap();
    let claim = Concept::call(
        "employment",
        [Concept::named("greg"), Concept::named("workiva")],
    );

    let id = store
        .assert_concept(&claim, Provenance::User { episode: Some(1) }, Some(1), None)
        .unwrap();
    assert!(store.holds(&claim).unwrap());
    let before = Utc::now();

    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(store.retract(id, Utc::now()).unwrap());

    assert!(!store.holds(&claim).unwrap(), "retracted claim still holds");
    assert!(store.live_assertions(&claim).unwrap().is_empty());
    assert_eq!(
        store.assertions_at(&claim, before).unwrap().len(),
        1,
        "the past should still show it as believed"
    );
}

#[test]
fn retracting_twice_is_not_an_error_and_changes_nothing() {
    let store = Store::open_in_memory().unwrap();
    let c = Concept::call("raining", []);
    let id = store
        .assert_concept(&c, Provenance::Bootstrap, None, None)
        .unwrap();
    assert!(store.retract(id, Utc::now()).unwrap());
    let second = store.retract(id, Utc::now()).unwrap();
    assert!(
        !second,
        "a second retraction should report that nothing changed"
    );
    assert!(!store.holds(&c).unwrap());
}

#[test]
fn competing_assertions_coexist_with_their_own_provenance() {
    // The user and the Teacher can both claim the same thing. Storing that as
    // one row would lose who said it, which is what credit assignment needs.
    let store = Store::open_in_memory().unwrap();
    let c = Concept::call("is-sad", [Concept::named("greg")]);
    store
        .assert_concept(
            &c,
            Provenance::User { episode: Some(7) },
            Some(7),
            Some(0.9),
        )
        .unwrap();
    store
        .assert_concept(&c, Provenance::Teacher { episode: None }, None, Some(0.4))
        .unwrap();

    let live = store.live_assertions(&c).unwrap();
    assert_eq!(live.len(), 2);
    assert!(
        live.iter()
            .any(|a| matches!(a.provenance, Provenance::User { .. }))
    );
    assert!(
        live.iter()
            .any(|a| matches!(a.provenance, Provenance::Teacher { .. }))
    );
}

// ---------------------------------------------------------------------------
// A reloaded brain is the brain that was written
// ---------------------------------------------------------------------------

#[test]
fn every_artifact_kind_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain.db");

    let concept = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    let realization = Realization {
        target: Concept::named("list-sort"),
        name: "native-quicksort".into(),
        spec: RealizationSpec::Native {
            native: NativeId::new("sort.quick"),
        },
        effect: Effect::Pure,
        activation: Activation::new(now()),
        provenance: Provenance::Bootstrap,
        tier: Tier::Kernel,
    };

    let assertion_id = {
        let store = Store::open(&path).unwrap();
        store.register_symbol("friend-with").unwrap();
        store.register_symbol("Greg").unwrap();
        store.put_concept(&concept).unwrap();
        store
            .put_meta(&meta_for(&concept, &["friendship"]))
            .unwrap();
        store.put_realization(&realization).unwrap();
        store.record_use(&concept, true, now()).unwrap();
        store
            .assert_concept(&concept, Provenance::Bootstrap, None, Some(0.8))
            .unwrap()
    };

    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.get_concept(concept.content_id()).unwrap(),
        Some(concept.clone())
    );
    assert_eq!(
        store.surface_lookup("friendship").unwrap(),
        vec![concept.clone()]
    );
    assert!(store.holds(&concept).unwrap());

    let reloaded = store
        .realizations_for(&Concept::named("list-sort"))
        .unwrap();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].spec, realization.spec);
    assert_eq!(reloaded[0].effect, Effect::Pure);

    // Evidence has to survive too, or every restart resets what Spoon learned
    // about which realizations work.
    let meta = store.get_meta(&concept).unwrap().unwrap();
    assert_eq!(meta.activation.uses, 1);
    assert_eq!(meta.activation.successes, 1);

    // Names survive so ids still print as words.
    let table = store.load_symbol_table().unwrap();
    assert_eq!(table.display(SymbolId::of("greg")), "Greg");

    let live = store.live_assertions(&concept).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, assertion_id);
}

// ---------------------------------------------------------------------------
// Seeds are deterministic and idempotent
// ---------------------------------------------------------------------------

fn populated_store() -> Store {
    let store = Store::open_in_memory().unwrap();
    for name in ["friend-with", "Greg", "Keal", "list-sort"] {
        store.register_symbol(name).unwrap();
    }
    for (a, b) in [("greg", "keal"), ("keal", "syd")] {
        let c = Concept::call("friend-with", [Concept::named(a), Concept::named(b)]);
        store.put_concept(&c).unwrap();
        store
            .assert_concept(&c, Provenance::Bootstrap, None, None)
            .unwrap();
    }
    store
        .put_meta(&meta_for(
            &Concept::named("greg"),
            &["Greg", "Greg Littlefield"],
        ))
        .unwrap();
    store
        .put_meta(&meta_for(&Concept::int(42), &["forty-two"]))
        .unwrap();
    store
        .put_realization(&Realization {
            target: Concept::named("list-sort"),
            name: "native-mergesort".into(),
            spec: RealizationSpec::Composed {
                body: Concept::call("merge", [Concept::hole(0)]),
            },
            effect: Effect::Pure,
            activation: Activation::new(now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .unwrap();
    store
}

#[test]
fn a_seed_round_trips_byte_identically() {
    // A seed is the distributable baseline and lives in git. If export is not
    // deterministic the diff is noise and nobody can review a brain change.
    let original = populated_store();
    let first = original.export_seed("baseline").unwrap();
    let first_json = serde_json::to_string_pretty(&first).unwrap();

    let fresh = Store::open_in_memory().unwrap();
    fresh.import_seed(&first).unwrap();
    let second_json =
        serde_json::to_string_pretty(&fresh.export_seed("baseline").unwrap()).unwrap();

    assert_eq!(
        first_json, second_json,
        "export/import/export was not stable"
    );
}

#[test]
fn exporting_the_same_store_twice_gives_the_same_bytes() {
    let store = populated_store();
    let a = serde_json::to_string(&store.export_seed("baseline").unwrap()).unwrap();
    let b = serde_json::to_string(&store.export_seed("baseline").unwrap()).unwrap();
    assert_eq!(a, b, "export is not deterministic");
}

#[test]
fn importing_twice_does_not_duplicate() {
    let seed = populated_store().export_seed("baseline").unwrap();
    let store = Store::open_in_memory().unwrap();
    store.import_seed(&seed).unwrap();
    let after_first = store.count_concepts().unwrap();
    store.import_seed(&seed).unwrap();
    assert_eq!(
        store.count_concepts().unwrap(),
        after_first,
        "import was not idempotent"
    );
    assert_eq!(
        store
            .realizations_for(&Concept::named("list-sort"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn storing_the_same_concept_twice_does_not_duplicate() {
    let store = Store::open_in_memory().unwrap();
    let c = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    store.put_concept(&c).unwrap();
    let after_first = store.count_concepts().unwrap();
    store.put_concept(&c).unwrap();
    assert_eq!(store.count_concepts().unwrap(), after_first);
}

#[test]
fn a_seed_carries_surface_forms_and_evidence_across() {
    let seed = populated_store().export_seed("baseline").unwrap();
    let store = Store::open_in_memory().unwrap();
    store.import_seed(&seed).unwrap();

    assert_eq!(
        store.surface_lookup("forty-two").unwrap(),
        vec![Concept::int(42)]
    );
    let greg = store.get_meta(&Concept::named("greg")).unwrap().unwrap();
    assert_eq!(greg.surface_forms.len(), 2);
    assert_eq!(
        greg.primary_surface(),
        Some("Greg"),
        "preference order was lost"
    );
}

// ---------------------------------------------------------------------------
// Evidence accumulation
// ---------------------------------------------------------------------------

#[test]
fn recording_uses_accumulates_rather_than_overwriting() {
    let store = Store::open_in_memory().unwrap();
    let c = Concept::named("list-sort");
    store.put_meta(&meta_for(&c, &["list-sort"])).unwrap();

    for i in 0..5 {
        store
            .record_use(&c, i % 2 == 0, now() + Duration::seconds(i))
            .unwrap();
    }
    let meta = store.get_meta(&c).unwrap().unwrap();
    assert_eq!(meta.activation.uses, 5);
    assert_eq!(meta.activation.successes, 3);
    assert_eq!(meta.activation.failures, 2);
    assert!(
        meta.activation
            .base_level(now() + Duration::seconds(10))
            .is_some()
    );
}

#[test]
fn realization_evidence_shifts_with_outcomes() {
    // Selection is evidence-weighted, so the store has to actually move the
    // numbers or every realization stays tied forever.
    let store = Store::open_in_memory().unwrap();
    let target = Concept::named("retrieve-resource");
    for (name, ok) in [("native-http", true), ("browser", false)] {
        store
            .put_realization(&Realization {
                target: target.clone(),
                name: name.into(),
                spec: RealizationSpec::Native {
                    native: NativeId::new(name),
                },
                effect: Effect::Network,
                activation: Activation::new(now()),
                provenance: Provenance::Bootstrap,
                tier: Tier::Provisional,
            })
            .unwrap();
        for i in 0..4 {
            store
                .record_realization_use(name, ok, now() + Duration::seconds(i))
                .unwrap();
        }
    }

    let all = store.realizations_for(&target).unwrap();
    let http = all.iter().find(|r| &*r.name == "native-http").unwrap();
    let browser = all.iter().find(|r| &*r.name == "browser").unwrap();
    assert!(
        http.activation.success_rate() > browser.activation.success_rate(),
        "evidence did not separate a working realization from a failing one"
    );
}
