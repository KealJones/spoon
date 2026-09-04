//! Contract tests for concept identity.
//!
//! These encode architectural invariants, not implementation details. If one
//! of these fails, the design is broken, not just the code.

use chrono::{TimeZone, Utc};
use spoon_concept::{
    Activation, Concept, ConceptId, ContentId, Effect, Ground, GroundKind, JsonBlob, SymbolId,
    SymbolTable,
};

// ---------------------------------------------------------------------------
// Symbol identity is stable and content-derived
// ---------------------------------------------------------------------------

#[test]
fn symbol_id_is_deterministic() {
    assert_eq!(SymbolId::of("friend-with"), SymbolId::of("friend-with"));
}

#[test]
fn symbol_id_ignores_case_and_surrounding_space() {
    // Casing is presentation, kept in surface forms. It must not fork identity,
    // or "Greg" and "greg" would become two different people.
    assert_eq!(SymbolId::of("Greg"), SymbolId::of("greg"));
    assert_eq!(SymbolId::of("  Greg  "), SymbolId::of("greg"));
    assert_eq!(SymbolId::of("FriendWith"), SymbolId::of("friendwith"));
}

#[test]
fn distinct_names_get_distinct_ids() {
    let names = [
        "greg",
        "keal",
        "math-add",
        "list-sort",
        "friend-with",
        "employment",
        "height",
    ];
    let mut seen = std::collections::HashSet::new();
    for n in names {
        assert!(seen.insert(SymbolId::of(n)), "collision on {n}");
    }
}

#[test]
fn symbol_table_is_display_only_not_identity() {
    // The id is computable without ever touching a table. The table only makes
    // it printable, which is why a fresh process can read a stored concept
    // without reconstructing any allocator state.
    let bare = SymbolId::of("employment");
    let table = SymbolTable::new();
    let interned = table.intern("Employment");
    assert_eq!(bare, interned);
    // Casing as written is kept for display; identity ignored it already.
    assert_eq!(table.display(bare), "Employment");
}

#[test]
fn unregistered_symbol_still_displays() {
    let table = SymbolTable::new();
    let id = SymbolId::of("never-registered");
    assert_eq!(table.display(id), format!("#{:016x}", id.as_u64()));
}

// ---------------------------------------------------------------------------
// Ground values: identity IS the value
// ---------------------------------------------------------------------------

#[test]
fn int_float_and_text_are_three_different_concepts() {
    // This is the whole reason Ground carries a discriminant into the digest.
    // If these collapsed, `Add<42, 1>` and `Concat<"42", "1">` could not be
    // told apart by the store.
    let as_int = Concept::int(42);
    let as_float = Concept::float(42.0);
    let as_text = Concept::text("42");

    assert_ne!(as_int.content_id(), as_float.content_id());
    assert_ne!(as_int.content_id(), as_text.content_id());
    assert_ne!(as_float.content_id(), as_text.content_id());
}

#[test]
fn bool_false_does_not_collide_with_int_zero() {
    assert_ne!(
        Concept::bool(false).content_id(),
        Concept::int(0).content_id()
    );
}

#[test]
fn nan_equals_itself() {
    // f64 says NaN != NaN, but a concept must equal itself or it could never be
    // looked up again. Ground canonicalizes NaN to one bit pattern.
    let a = Ground::Float(f64::NAN);
    let b = Ground::Float(f64::NAN);
    assert_eq!(a, b);
    assert_eq!(
        Concept::ground(a).content_id(),
        Concept::ground(b).content_id()
    );
}

#[test]
fn negative_zero_equals_positive_zero() {
    let pos = Concept::float(0.0);
    let neg = Concept::float(-0.0);
    assert_eq!(pos, neg);
    assert_eq!(pos.content_id(), neg.content_id());
}

#[test]
fn ground_hashing_agrees_with_equality() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(Ground::Float(f64::NAN));
    assert!(set.contains(&Ground::Float(f64::NAN)));
    set.insert(Ground::Float(0.0));
    assert!(set.contains(&Ground::Float(-0.0)));
    assert_eq!(set.len(), 2);
}

#[test]
fn ground_kinds_are_reported_correctly() {
    assert_eq!(Ground::Bool(true).kind(), GroundKind::Bool);
    assert_eq!(Ground::Int(1).kind(), GroundKind::Int);
    assert_eq!(Ground::Float(1.0).kind(), GroundKind::Float);
    assert_eq!(Ground::text("x").kind(), GroundKind::Text);
    assert_eq!(Ground::bytes([1u8, 2]).kind(), GroundKind::Bytes);
    assert_eq!(Ground::json(serde_json::json!({})).kind(), GroundKind::Json);
}

#[test]
fn json_blob_digest_survives_serde_round_trip() {
    // A skipped digest field would deserialize to zeros and silently break Eq
    // and Hash, so every JSON concept in a reloaded brain would stop matching
    // itself. Serialization goes through the bare Value and rebuilds the
    // digest on the way back in.
    let blob = JsonBlob::new(serde_json::json!({"b": 2, "a": [1, 2, 3]}));
    let encoded = serde_json::to_string(&blob).unwrap();
    let decoded: JsonBlob = serde_json::from_str(&encoded).unwrap();

    assert_eq!(blob.digest(), decoded.digest());
    assert_eq!(blob, decoded);

    let mut set = std::collections::HashSet::new();
    set.insert(blob);
    assert!(set.contains(&decoded));
}

#[test]
fn json_object_key_order_does_not_change_identity() {
    // serde_json backs objects with a BTreeMap, so the canonical form is
    // key-sorted. Two payloads that differ only in written key order are the
    // same concept.
    let a: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
    let b: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
    assert_eq!(JsonBlob::new(a), JsonBlob::new(b));
}

// ---------------------------------------------------------------------------
// Structural identity
// ---------------------------------------------------------------------------

#[test]
fn content_id_is_stable_across_construction_paths() {
    let a = Concept::call(
        "friend-with",
        [Concept::named("Greg"), Concept::named("Keal")],
    );
    let b = Concept::apply(
        Concept::named("Friend-With"),
        vec![Concept::named("greg"), Concept::named("keal")],
    );
    assert_eq!(a.content_id(), b.content_id());
}

#[test]
fn hyphenation_is_significant() {
    // Normalization lowercases and trims but deliberately does not touch
    // separators: collapsing them would merge "co-op" with "coop". The cost is
    // that "FriendWith" and "friend-with" are two different concepts, so code
    // and prose have to agree on one spelling. Kebab-case is the convention for
    // symbol names passed to `Concept::named` and `Concept::call`.
    assert_ne!(SymbolId::of("FriendWith"), SymbolId::of("friend-with"));
    assert_eq!(SymbolId::of("FriendWith"), SymbolId::of("friendwith"));
}

#[test]
fn argument_order_matters() {
    let a = Concept::call(
        "friend-with",
        [Concept::named("Greg"), Concept::named("Keal")],
    );
    let b = Concept::call(
        "friend-with",
        [Concept::named("Keal"), Concept::named("Greg")],
    );
    // These are different concepts. That they mean the same thing is something
    // Symmetric<FriendWith> has to establish by inference, not something the
    // representation may assume.
    assert_ne!(a.content_id(), b.content_id());
}

#[test]
fn nesting_is_not_flattening() {
    // f<a, g<b>> must not digest the same as f<a, g, b>. The arity is mixed
    // into the digest before the children precisely to prevent this.
    let nested = Concept::call(
        "f",
        [
            Concept::named("a"),
            Concept::call("g", [Concept::named("b")]),
        ],
    );
    let flat = Concept::call(
        "f",
        [
            Concept::named("a"),
            Concept::named("g"),
            Concept::named("b"),
        ],
    );
    assert_ne!(nested.content_id(), flat.content_id());
}

#[test]
fn zero_arg_compound_differs_from_bare_atomic() {
    // Raining<> asserts the concept in context. Raining alone just names it.
    let asserted = Concept::call("raining", []);
    let named = Concept::named("raining");
    assert_ne!(asserted.content_id(), named.content_id());
    assert!(asserted.is_compound());
    assert!(named.is_atomic());
}

#[test]
fn holes_are_distinguished_by_index() {
    assert_ne!(Concept::hole(0).content_id(), Concept::hole(1).content_id());
}

#[test]
fn hole_does_not_collide_with_named_or_ground() {
    let h = Concept::hole(0).content_id();
    assert_ne!(h, Concept::int(0).content_id());
    assert_ne!(h, Concept::named("0").content_id());
}

#[test]
fn content_id_hex_round_trips() {
    let c = Concept::call(
        "stated",
        [
            Concept::named("keal"),
            Concept::call("is-sad", [Concept::named("greg")]),
        ],
    );
    let id = c.content_id();
    let hex = id.to_hex();
    assert_eq!(hex.len(), 64);
    assert_eq!(ContentId::from_hex(&hex), Some(id));
    assert_eq!(id.short().len(), 12);
    assert!(ContentId::from_hex("nonsense").is_none());
}

// ---------------------------------------------------------------------------
// Shape accessors
// ---------------------------------------------------------------------------

#[test]
fn ground_identity_versus_named_identity() {
    let answer = Concept::int(42);
    assert!(answer.is_atomic());
    assert!(answer.is_ground());
    assert!(!answer.is_named());
    assert_eq!(answer.as_ground(), Some(&Ground::Int(42)));
    assert_eq!(answer.as_symbol(), None);

    let greg = Concept::named("Greg");
    assert!(greg.is_named());
    assert!(!greg.is_ground());
    assert_eq!(greg.as_symbol(), Some(SymbolId::of("greg")));
}

#[test]
fn compound_exposes_head_and_args() {
    let c = Concept::call(
        "employment",
        [Concept::named("Greg"), Concept::named("Workiva")],
    );
    assert_eq!(c.arity(), 2);
    assert_eq!(c.head_symbol(), Some(SymbolId::of("employment")));
    assert_eq!(c.arg(0), Some(&Concept::named("greg")));
    assert_eq!(c.arg(1), Some(&Concept::named("workiva")));
    assert_eq!(c.arg(2), None);
}

#[test]
fn head_symbol_is_none_for_partial_application() {
    // (Sort<Descending>)<Friends>: the head is itself a compound, so there is
    // no single symbol to file it under. The store has to fall back to the
    // head's content_id for these.
    let curried = Concept::apply(
        Concept::call("list-sort", [Concept::named("descending")]),
        vec![Concept::named("friends")],
    );
    assert_eq!(curried.head_symbol(), None);
    assert!(curried.head().unwrap().is_compound());
}

#[test]
fn size_and_depth_count_the_head() {
    let atom = Concept::named("greg");
    assert_eq!(atom.size(), 1);
    assert_eq!(atom.depth(), 1);

    // Compound node + head + 2 args
    let flat = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    assert_eq!(flat.size(), 4);
    assert_eq!(flat.depth(), 2);

    let nested = Concept::call(
        "math-sum",
        [Concept::call(
            "list-map",
            [Concept::named("friends"), Concept::named("height")],
        )],
    );
    assert_eq!(nested.depth(), 3);
    assert_eq!(nested.size(), 6);
}

#[test]
fn atomic_and_hole_have_no_args() {
    assert!(Concept::named("greg").args().is_empty());
    assert!(Concept::hole(3).args().is_empty());
    assert_eq!(Concept::int(1).arity(), 0);
}

// ---------------------------------------------------------------------------
// Serde round trip
// ---------------------------------------------------------------------------

#[test]
fn concepts_round_trip_through_json() {
    let corpus = vec![
        Concept::named("Greg"),
        Concept::int(-17),
        Concept::float(2.5),
        Concept::bool(true),
        Concept::text("hello \"world\"\n"),
        Concept::bytes([0u8, 1, 2, 255]),
        Concept::json(serde_json::json!({"probes": [{"score": 3}]})),
        Concept::datetime(Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()),
        Concept::hole(7),
        Concept::call("raining", []),
        Concept::call(
            "stated",
            [
                Concept::named("keal"),
                Concept::call("is-sad", [Concept::named("greg")]),
            ],
        ),
        Concept::apply(
            Concept::call("list-sort", [Concept::named("descending")]),
            vec![Concept::named("friends")],
        ),
    ];

    for c in corpus {
        let encoded = serde_json::to_string(&c).unwrap();
        let decoded: Concept = serde_json::from_str(&encoded).unwrap();
        assert_eq!(c, decoded, "value mismatch for {encoded}");
        assert_eq!(
            c.content_id(),
            decoded.content_id(),
            "content_id drifted for {encoded}"
        );
    }
}

#[test]
fn non_finite_floats_survive_json() {
    // JSON has no spelling for NaN or infinity and serde_json writes them as
    // null, which then refuses to read back as f64. A concept carrying one
    // would be writable and permanently unreadable, so Ground::Float encodes
    // the non-finite cases as strings.
    for f in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let c = Concept::call("f", [Concept::float(f)]);
        let encoded = serde_json::to_string(&c).unwrap();
        let decoded: Concept = serde_json::from_str(&encoded)
            .unwrap_or_else(|e| panic!("{f} encoded as {encoded} and failed to read back: {e}"));
        assert_eq!(c.content_id(), decoded.content_id(), "{encoded}");
    }
}

#[test]
fn finite_floats_stay_readable_in_seed_files() {
    // Only the non-finite cases become strings. Ordinary values stay JSON
    // numbers so an exported seed is still something a human can read.
    let encoded = serde_json::to_string(&Concept::float(2.5)).unwrap();
    assert!(encoded.contains(r#""Float":2.5"#), "got {encoded}");
    assert!(
        !encoded.contains(r#""Float":""#),
        "finite float was quoted: {encoded}"
    );
}

#[test]
fn concept_id_variants_round_trip() {
    for id in [
        ConceptId::named("greg"),
        ConceptId::Ground(Ground::Int(1)),
        ConceptId::Ground(Ground::text("x")),
    ] {
        let s = serde_json::to_string(&id).unwrap();
        let back: ConceptId = serde_json::from_str(&s).unwrap();
        assert_eq!(id, back);
    }
}

// ---------------------------------------------------------------------------
// Activation: the evidence that drives selection
// ---------------------------------------------------------------------------

#[test]
fn fresh_activation_has_no_base_level() {
    // No history means "unknown", not "bad". A brand new realization has no
    // evidence against it and must stay reachable.
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    let act = Activation::new(now);
    assert_eq!(act.base_level(now), None);
    assert_eq!(act.uses, 0);
}

#[test]
fn unused_realization_scores_neutral() {
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    // Laplace smoothing: (0 + 1) / (0 + 2)
    assert!((Activation::new(now).success_rate() - 0.5).abs() < 1e-9);
}

#[test]
fn recent_use_outranks_old_use() {
    let base = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    let now = base + chrono::Duration::hours(10);

    let mut recent = Activation::new(base);
    recent.record(now - chrono::Duration::minutes(1), true);

    let mut old = Activation::new(base);
    old.record(base, true);

    assert!(recent.base_level(now).unwrap() > old.base_level(now).unwrap());
}

#[test]
fn frequent_use_outranks_single_use() {
    let base = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    let now = base + chrono::Duration::hours(2);

    let mut frequent = Activation::new(base);
    for i in 0..10 {
        frequent.record(base + chrono::Duration::minutes(i), true);
    }
    let mut rare = Activation::new(base);
    rare.record(base, true);

    assert!(frequent.base_level(now).unwrap() > rare.base_level(now).unwrap());
}

#[test]
fn same_instant_access_does_not_blow_up() {
    // Age is clamped to 1 ms so a same-millisecond access cannot divide by zero
    // and hand back an infinite score.
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    let mut act = Activation::new(now);
    act.record(now, true);
    let level = act.base_level(now).unwrap();
    assert!(level.is_finite(), "base level was {level}");
}

#[test]
fn access_window_is_bounded() {
    let base = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    let mut act = Activation::new(base);
    for i in 0..(Activation::ACCESS_WINDOW as i64 * 3) {
        act.record(base + chrono::Duration::seconds(i), true);
    }
    assert_eq!(act.accesses.len(), Activation::ACCESS_WINDOW);
    assert_eq!(act.uses, Activation::ACCESS_WINDOW as u64 * 3);
    // The retained window must be the newest entries, not the oldest.
    let newest = base + chrono::Duration::seconds(Activation::ACCESS_WINDOW as i64 * 3 - 1);
    assert_eq!(*act.accesses.last().unwrap(), newest.timestamp_millis());
}

#[test]
fn failures_pull_success_rate_down() {
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    let mut good = Activation::new(now);
    let mut bad = Activation::new(now);
    for _ in 0..10 {
        good.record(now, true);
        bad.record(now, false);
    }
    assert!(good.success_rate() > bad.success_rate());
    assert_eq!(good.failures, 0);
    assert_eq!(bad.successes, 0);
}

#[test]
fn one_early_failure_does_not_bury_a_realization() {
    // Smoothing exists so a single bad first run does not permanently sink an
    // otherwise good realization.
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    let mut act = Activation::new(now);
    act.record(now, false);
    assert!(act.success_rate() > 0.3, "rate was {}", act.success_rate());
}

// ---------------------------------------------------------------------------
// Effect authority
// ---------------------------------------------------------------------------

#[test]
fn effects_order_by_required_authority() {
    assert!(Effect::Pure.rank() < Effect::Read.rank());
    assert!(Effect::Read.rank() < Effect::Write.rank());
    assert!(Effect::Write.rank() < Effect::Network.rank());
    assert!(Effect::Network.rank() < Effect::Shell.rank());
}

#[test]
fn composed_effect_is_the_maximum_of_its_parts() {
    // A composed realization is as dangerous as its most dangerous step.
    assert_eq!(Effect::Pure.join(Effect::Network), Effect::Network);
    assert_eq!(Effect::Shell.join(Effect::Pure), Effect::Shell);
    assert_eq!(Effect::Read.join(Effect::Read), Effect::Read);
}
