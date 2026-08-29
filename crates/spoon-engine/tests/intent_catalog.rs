use spoon_core::Lifecycle;
use spoon_engine::{
    EngineError, IntentCatalogEntry, IntentCatalogStore, IntentSlotSchema, MAX_PATTERNS_PER_KEY,
    PatternAdmission, normalize_skeleton,
};

fn multiply_slots() -> Vec<IntentSlotSchema> {
    vec![
        IntentSlotSchema {
            name: "v0".into(),
            required: true,
            value_kind: "number".into(),
        },
        IntentSlotSchema {
            name: "v1".into(),
            required: true,
            value_kind: "number".into(),
        },
    ]
}

fn multiply_entry() -> IntentCatalogEntry {
    IntentCatalogEntry {
        key: "arithmetic.multiply".into(),
        slots: multiply_slots(),
        concept_id: None,
        procedure_id: None,
        procedure_version: None,
        lifecycle: Lifecycle::Active,
        created_at: 1_000,
    }
}

fn store_with_multiply_entry() -> IntentCatalogStore {
    let store = IntentCatalogStore::in_memory().unwrap();
    store.upsert_entry(&multiply_entry()).unwrap();
    store
}

// --- normalization -------------------------------------------------------

#[test]
fn normalization_makes_argument_order_a_real_distinction() {
    let slots = multiply_slots();
    let forward = normalize_skeleton("what is {v0} times {v1}", &slots).unwrap();
    let swapped = normalize_skeleton("{v1} times {v0}", &slots).unwrap();

    assert_eq!(forward, "what is {0} times {1}");
    assert_eq!(swapped, "{1} times {0}");
    assert_ne!(forward, swapped);
}

#[test]
fn normalization_applies_nfkc_lowercase_and_whitespace_collapse() {
    let slots = multiply_slots();
    // U+FF2D U+FF49 U+FF58 spell "Mix" in fullwidth forms; NFKC folds them to
    // ASCII, and the following lowercase step then folds the case.
    let fullwidth = "\u{FF2D}\u{FF49}\u{FF58}   {v0}    times   {v1}";
    let normalized = normalize_skeleton(fullwidth, &slots).unwrap();
    assert_eq!(normalized, "mix {0} times {1}");
}

#[test]
fn normalization_strips_leading_and_trailing_punctuation_but_keeps_interior() {
    let slots = multiply_slots();
    let normalized = normalize_skeleton("¿what is {v0} times {v1}, exactly?!", &slots).unwrap();
    assert_eq!(normalized, "what is {0} times {1}, exactly");
}

#[test]
fn normalization_keeps_placeholder_braces_at_the_string_edge() {
    let slots = multiply_slots();
    let normalized = normalize_skeleton("{v1} times {v0}", &slots).unwrap();
    assert!(normalized.starts_with('{'));
    assert!(normalized.ends_with('}'));
}

#[test]
fn normalization_rejects_a_placeholder_naming_an_unknown_slot() {
    let slots = multiply_slots();
    let error = normalize_skeleton("what is {v0} times {v9}", &slots).unwrap_err();
    assert!(matches!(error, EngineError::InvalidInput(_)));
}

// --- pattern lifecycle -----------------------------------------------------

#[test]
fn a_successful_pattern_admits_provisional_with_support_one() {
    let store = store_with_multiply_entry();
    let admission = store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-1")
        .unwrap();
    assert_eq!(admission, PatternAdmission::Admitted);

    let patterns = store.list_patterns("arithmetic.multiply").unwrap();
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0].support, 1);
    assert_eq!(patterns[0].contradictions, 0);
    assert_eq!(patterns[0].lifecycle, Lifecycle::Provisional);
    assert_eq!(patterns[0].first_episode, "ep-1");
    assert_eq!(patterns[0].last_episode, "ep-1");
}

#[test]
fn a_distinct_episode_repeating_the_skeleton_promotes_to_active() {
    let store = store_with_multiply_entry();
    store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-1")
        .unwrap();
    let admission = store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-2")
        .unwrap();
    assert_eq!(admission, PatternAdmission::Promoted { support: 2 });

    let patterns = store.list_patterns("arithmetic.multiply").unwrap();
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0].support, 2);
    assert_eq!(patterns[0].lifecycle, Lifecycle::Active);
    assert_eq!(patterns[0].last_episode, "ep-2");
}

#[test]
fn the_same_episode_does_not_double_count_support() {
    let store = store_with_multiply_entry();
    store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-1")
        .unwrap();
    let admission = store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-1")
        .unwrap();
    assert_eq!(admission, PatternAdmission::AlreadyCounted);

    let patterns = store.list_patterns("arithmetic.multiply").unwrap();
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0].support, 1);
}

#[test]
fn only_active_patterns_are_returned_by_matching_patterns() {
    let store = store_with_multiply_entry();
    store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-1")
        .unwrap();
    let skeleton = normalize_skeleton("what is {v0} times {v1}", &multiply_slots()).unwrap();

    // Only support 1 so far: still Provisional, must not drive local matching.
    assert!(store.matching_patterns(&skeleton).unwrap().is_empty());

    store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-2")
        .unwrap();
    let matches = store.matching_patterns(&skeleton).unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].lifecycle, Lifecycle::Active);
    assert_eq!(matches[0].skeleton, skeleton);
}

#[test]
fn a_contradiction_drops_an_active_pattern_to_under_review_then_retires_it() {
    let store = store_with_multiply_entry();
    store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-1")
        .unwrap();
    store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-2")
        .unwrap();
    let skeleton = normalize_skeleton("what is {v0} times {v1}", &multiply_slots()).unwrap();
    assert_eq!(store.matching_patterns(&skeleton).unwrap().len(), 1);

    store
        .record_contradiction("arithmetic.multiply", &skeleton)
        .unwrap();
    let patterns = store.list_patterns("arithmetic.multiply").unwrap();
    assert_eq!(patterns[0].lifecycle, Lifecycle::UnderReview);
    assert_eq!(patterns[0].contradictions, 1);
    assert!(store.matching_patterns(&skeleton).unwrap().is_empty());

    store
        .record_contradiction("arithmetic.multiply", &skeleton)
        .unwrap();
    let patterns = store.list_patterns("arithmetic.multiply").unwrap();
    assert_eq!(patterns[0].lifecycle, Lifecycle::Retired);
    assert_eq!(patterns[0].contradictions, 2);
    assert!(store.matching_patterns(&skeleton).unwrap().is_empty());
}

#[test]
fn cap_eviction_removes_the_lowest_support_provisional_and_never_an_active_one() {
    let store = store_with_multiply_entry();

    // One pattern earns real support and gets promoted to Active.
    store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-1")
        .unwrap();
    store
        .admit_pattern("arithmetic.multiply", "what is {v0} times {v1}", "ep-2")
        .unwrap();
    let active_skeleton = normalize_skeleton("what is {v0} times {v1}", &multiply_slots()).unwrap();

    // Fill the remaining 15 slots with distinct, single-support Provisional
    // patterns so the key sits at the 16-pattern cap.
    for i in 0..(MAX_PATTERNS_PER_KEY - 1) {
        let pattern = format!("phrasing variant {i} for {{v0}} and {{v1}}");
        let admission = store
            .admit_pattern("arithmetic.multiply", &pattern, &format!("ep-fill-{i}"))
            .unwrap();
        assert_eq!(admission, PatternAdmission::Admitted);
    }
    assert_eq!(
        store.list_patterns("arithmetic.multiply").unwrap().len(),
        MAX_PATTERNS_PER_KEY
    );

    // The very first fill pattern (i = 0) has the lowest support (1) and the
    // earliest first_episode among the Provisional set, so it is the
    // eviction candidate.
    let weakest_skeleton =
        normalize_skeleton("phrasing variant 0 for {v0} and {v1}", &multiply_slots()).unwrap();

    let admission = store
        .admit_pattern(
            "arithmetic.multiply",
            "yet another phrasing for {v0} and {v1}",
            "ep-new",
        )
        .unwrap();
    match admission {
        PatternAdmission::Evicted { evicted_skeleton } => {
            assert_eq!(evicted_skeleton, weakest_skeleton);
        }
        other => panic!("expected an eviction, got {other:?}"),
    }

    let patterns = store.list_patterns("arithmetic.multiply").unwrap();
    assert_eq!(patterns.len(), MAX_PATTERNS_PER_KEY);
    assert!(
        !patterns
            .iter()
            .any(|pattern| pattern.skeleton == weakest_skeleton),
        "evicted pattern must be gone"
    );
    assert!(
        patterns
            .iter()
            .any(|pattern| pattern.skeleton == active_skeleton
                && pattern.lifecycle == Lifecycle::Active),
        "the Active pattern must never be evicted"
    );
}

#[test]
fn cap_full_of_active_patterns_refuses_the_new_pattern() {
    let store = store_with_multiply_entry();

    // Promote MAX_PATTERNS_PER_KEY distinct skeletons to Active, filling the
    // cap entirely with patterns that must never be evicted.
    for i in 0..MAX_PATTERNS_PER_KEY {
        let pattern = format!("phrasing variant {i} for {{v0}} and {{v1}}");
        store
            .admit_pattern("arithmetic.multiply", &pattern, &format!("ep-{i}-a"))
            .unwrap();
        let admission = store
            .admit_pattern("arithmetic.multiply", &pattern, &format!("ep-{i}-b"))
            .unwrap();
        assert!(matches!(admission, PatternAdmission::Promoted { .. }));
    }

    let admission = store
        .admit_pattern(
            "arithmetic.multiply",
            "one phrasing too many for {v0} and {v1}",
            "ep-overflow",
        )
        .unwrap();
    match admission {
        PatternAdmission::Refused { reason } => {
            assert!(reason.contains("arithmetic.multiply"));
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(
        store.list_patterns("arithmetic.multiply").unwrap().len(),
        MAX_PATTERNS_PER_KEY
    );
}

// --- entries -----------------------------------------------------------

#[test]
fn entry_upsert_get_list_and_bind_procedure_round_trip() {
    let store = IntentCatalogStore::in_memory().unwrap();
    let entry = multiply_entry();
    store.upsert_entry(&entry).unwrap();

    let fetched = store.get_entry(&entry.key).unwrap().unwrap();
    assert_eq!(fetched, entry);

    let all = store.list_entries(10).unwrap();
    assert_eq!(all, vec![entry.clone()]);

    store
        .bind_procedure("arithmetic.multiply", "concept-1", "procedure-1", 3)
        .unwrap();
    let bound = store.get_entry("arithmetic.multiply").unwrap().unwrap();
    assert_eq!(bound.concept_id.as_deref(), Some("concept-1"));
    assert_eq!(bound.procedure_id.as_deref(), Some("procedure-1"));
    assert_eq!(bound.procedure_version, Some(3));
    // created_at must survive the later bind_procedure/upsert cycle unchanged.
    assert_eq!(bound.created_at, entry.created_at);
}

#[test]
fn an_unbound_entry_round_trips_with_none_procedure_fields() {
    let store = IntentCatalogStore::in_memory().unwrap();
    let entry = multiply_entry();
    store.upsert_entry(&entry).unwrap();

    let fetched = store.get_entry(&entry.key).unwrap().unwrap();
    assert_eq!(fetched.concept_id, None);
    assert_eq!(fetched.procedure_id, None);
    assert_eq!(fetched.procedure_version, None);
}

#[test]
fn bind_procedure_on_an_unknown_key_errors() {
    let store = IntentCatalogStore::in_memory().unwrap();
    let error = store
        .bind_procedure("no.such.key", "concept-1", "procedure-1", 1)
        .unwrap_err();
    assert!(matches!(error, EngineError::InvalidInput(_)));
}

#[test]
fn admit_pattern_on_an_unknown_key_errors() {
    let store = IntentCatalogStore::in_memory().unwrap();
    let error = store
        .admit_pattern("no.such.key", "what is {v0} times {v1}", "ep-1")
        .unwrap_err();
    assert!(matches!(error, EngineError::InvalidInput(_)));
}
