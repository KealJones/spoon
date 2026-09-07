//! Everything survives a restart, and evidence accumulates across one.

use chrono::Utc;
use spoon_concept::{
    Activation, Concept, ConceptMeta, Effect, NativeId, Provenance, Realization, RealizationKind,
    RealizationSpec, RuleDirection, SymbolId, Tier,
};
use spoon_store::{Store, StoreError};

fn realization(name: &str, target: Concept, spec: RealizationSpec) -> Realization {
    Realization {
        target,
        name: name.into(),
        spec,
        effect: Effect::Pure,
        activation: Activation::new(Utc::now()),
        provenance: Provenance::Bootstrap,
        tier: Tier::Kernel,
    }
}

fn every_spec() -> Vec<(&'static str, RealizationSpec)> {
    vec![
        (
            "add/native",
            RealizationSpec::Native {
                native: NativeId::new("arith.add"),
            },
        ),
        (
            "double/composed",
            RealizationSpec::Composed {
                body: Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]),
            },
        ),
        (
            "symmetric/rule",
            RealizationSpec::Rule {
                pattern: Concept::apply(Concept::hole(0), vec![Concept::hole(1), Concept::hole(2)]),
                condition: Some(Concept::call("symmetric", [Concept::hole(0)])),
                produce: Concept::apply(Concept::hole(0), vec![Concept::hole(2), Concept::hole(1)]),
                direction: RuleDirection::Forward,
            },
        ),
        (
            "summarize/neural",
            RealizationSpec::Neural {
                prompt: Concept::call("template", [Concept::text("summarize {0}")]),
                parse: Concept::named("parse-summary"),
            },
        ),
        (
            "fetch/external",
            RealizationSpec::External {
                spec: Concept::call("http-get", [Concept::text("https://example.test")]),
            },
        ),
    ]
}

#[test]
fn every_realization_shape_round_trips() {
    let store = Store::open_in_memory().unwrap();
    let target = Concept::named("math-add");
    let written: Vec<Realization> = every_spec()
        .into_iter()
        .map(|(name, spec)| realization(name, target.clone(), spec))
        .collect();
    for r in &written {
        store.put_realization(r).unwrap();
    }

    let mut expected = written.clone();
    expected.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(store.all_realizations().unwrap(), expected);
    assert_eq!(store.realizations_for(&target).unwrap(), expected);

    for r in &written {
        let back = store.realization_by_name(&r.name).unwrap().unwrap();
        assert_eq!(&back, r);
        assert_eq!(back.spec.kind(), r.spec.kind());
    }
    assert!(store.realization_by_name("nope").unwrap().is_none());

    // The kind index agrees with the spec it was derived from.
    let rules = store.realizations_by_kind(RealizationKind::Rule).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].name.as_ref(), "symmetric/rule");
}

#[test]
fn recording_a_use_updates_activation_and_the_change_survives_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain.db");
    let greg = Concept::named("greg");
    let now = Utc::now();

    {
        let store = Store::open(&path).unwrap();
        store
            .put_meta(&ConceptMeta::new(
                greg.clone(),
                Provenance::User { episode: None },
                Tier::Provisional,
                now,
            ))
            .unwrap();
        store.record_use(&greg, true, now).unwrap();
        store.record_use(&greg, false, now).unwrap();

        let activation = store.activation(&greg).unwrap().unwrap();
        assert_eq!(activation.uses, 2);
        assert_eq!(activation.successes, 1);
        assert_eq!(activation.failures, 1);
    }

    let store = Store::open(&path).unwrap();
    let meta = store.get_meta(&greg).unwrap().unwrap();
    assert_eq!(meta.activation.uses, 2);
    assert_eq!(meta.activation.successes, 1);
    assert_eq!(meta.activation.accesses.len(), 2);
    assert!(meta.activation.last_used_at.is_some());
    // Provenance and tier are untouched by recording evidence.
    assert_eq!(meta.provenance, Provenance::User { episode: None });
    assert_eq!(meta.tier, Tier::Provisional);
}

#[test]
fn recording_a_use_of_an_undescribed_concept_starts_tracking_it() {
    let store = Store::open_in_memory().unwrap();
    let c = Concept::named("list-sort");
    store.record_use(&c, true, Utc::now()).unwrap();

    let meta = store.get_meta(&c).unwrap().unwrap();
    assert_eq!(meta.activation.uses, 1);
    assert_eq!(meta.provenance, Provenance::Inferred);
    assert_eq!(meta.tier, Tier::Provisional);
}

#[test]
fn recording_a_use_of_an_unknown_realization_is_an_error() {
    let store = Store::open_in_memory().unwrap();
    let err = store
        .record_realization_use("never-stored", true, Utc::now())
        .unwrap_err();
    assert!(matches!(err, StoreError::MissingRecord { .. }));
}

#[test]
fn surface_lookup_is_case_insensitive_and_ordered_by_preference() {
    let store = Store::open_in_memory().unwrap();
    let greg = Concept::named("greg");
    let gregory = Concept::named("gregory-house");
    store
        .put_meta(
            &ConceptMeta::new(
                greg.clone(),
                Provenance::Bootstrap,
                Tier::Kernel,
                Utc::now(),
            )
            .with_surface_forms(["Greg", "Gregory"]),
        )
        .unwrap();
    store
        .put_meta(
            &ConceptMeta::new(
                gregory.clone(),
                Provenance::Bootstrap,
                Tier::Kernel,
                Utc::now(),
            )
            .with_surface_forms(["Gregory", "House"]),
        )
        .unwrap();

    assert_eq!(store.surface_lookup("GREG").unwrap(), vec![greg.clone()]);
    assert_eq!(store.surface_lookup(" greg ").unwrap(), vec![greg.clone()]);
    // Two concepts share "gregory"; the one that prefers it most comes first.
    let ambiguous = store.surface_lookup("gregory").unwrap();
    assert_eq!(ambiguous, vec![gregory.clone(), greg.clone()]);
    assert!(store.surface_lookup("nobody").unwrap().is_empty());

    // Rewriting metadata replaces the index rather than adding to it.
    store
        .put_meta(
            &ConceptMeta::new(
                greg.clone(),
                Provenance::Bootstrap,
                Tier::Kernel,
                Utc::now(),
            )
            .with_surface_forms(["Greg"]),
        )
        .unwrap();
    assert_eq!(store.surface_lookup("gregory").unwrap(), vec![gregory]);
}

#[test]
fn a_whole_brain_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain.db");

    let friendship = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    let nested = Concept::call(
        "stated",
        [
            Concept::named("keal"),
            Concept::call("is-sad", [Concept::named("greg")]),
        ],
    );
    let answer = Concept::int(42);
    let now = Utc::now();
    let meta = ConceptMeta::new(
        answer.clone(),
        Provenance::User { episode: Some(3) },
        Tier::Consolidated,
        now,
    )
    .with_surface_forms(["forty-two", "the answer"])
    .with_note("materialized because something was said about it");
    let realization = realization(
        "add/native",
        Concept::named("math-add"),
        RealizationSpec::Native {
            native: NativeId::new("arith.add"),
        },
    );

    let assertion_id;
    let before_count;
    {
        let store = Store::open(&path).unwrap();
        store.put_concept(&friendship).unwrap();
        store.put_concept(&nested).unwrap();
        store.put_meta(&meta).unwrap();
        store.put_realization(&realization).unwrap();
        store.register_symbol("friend-with").unwrap();
        store.register_symbol("Greg").unwrap();
        assertion_id = store
            .assert_concept(
                &friendship,
                Provenance::User { episode: Some(3) },
                Some(3),
                Some(0.75),
            )
            .unwrap();
        before_count = store.count_concepts().unwrap();
    }

    let store = Store::open(&path).unwrap();
    assert_eq!(store.count_concepts().unwrap(), before_count);
    assert_eq!(
        store.get_concept(friendship.content_id()).unwrap().unwrap(),
        friendship
    );
    assert_eq!(
        store.get_concept(nested.content_id()).unwrap().unwrap(),
        nested
    );
    assert_eq!(store.get_meta(&answer).unwrap().unwrap(), meta);
    assert_eq!(
        store.realization_by_name("add/native").unwrap().unwrap(),
        realization
    );
    assert_eq!(
        store
            .concepts_by_head(SymbolId::of("friend-with"), 10)
            .unwrap(),
        vec![friendship.clone()]
    );
    assert!(
        store
            .concepts_containing(Concept::named("greg").content_id(), 10)
            .unwrap()
            .contains(&nested)
    );
    assert!(store.holds(&friendship).unwrap());
    assert_eq!(
        store.live_assertions(&friendship).unwrap()[0].id,
        assertion_id
    );
    assert_eq!(store.surface_lookup("The Answer").unwrap(), vec![answer]);

    // Names print again after a restart, with the spelling they were
    // registered under. Identity ignores casing, so the id still resolves no
    // matter how the caller spells it.
    let symbols = store.load_symbol_table().unwrap();
    assert_eq!(symbols.len(), 2);
    assert_eq!(symbols.display(SymbolId::of("friend-with")), "friend-with");
    assert_eq!(symbols.display(SymbolId::of("greg")), "Greg");
    assert_eq!(
        store.symbol_name(SymbolId::of("GREG")).unwrap().as_deref(),
        Some("Greg")
    );
    assert!(
        store
            .symbol_name(SymbolId::of("unknown"))
            .unwrap()
            .is_none()
    );
}
