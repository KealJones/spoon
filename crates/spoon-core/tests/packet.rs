//! The bounded context projection handed to a front language model.

use spoon_core::Value;
use spoon_core::language::{TokenRange, tokenize};
use spoon_core::packet::{
    LanguageContextPacket, MAX_TURN_WINDOW, PacketAlias, PacketCatalogEntry, PacketEnvFact,
    PacketFact, PacketLimits, PacketSlot, PacketTurn, SupplementalBudget, SupplementalRequest,
    TurnRole,
};

fn packet() -> LanguageContextPacket {
    LanguageContextPacket::new(tokenize("what is 2 times 3").expect("tokenizes"))
}

fn turn(index: usize, summary: &str) -> PacketTurn {
    PacketTurn {
        alias: format!("t{index}"),
        role: TurnRole::User,
        summary: summary.to_string(),
        facts: Vec::new(),
    }
}

fn entry(index: usize, key: &str, patterns: Vec<&str>) -> PacketCatalogEntry {
    PacketCatalogEntry {
        alias: format!("c{index}"),
        key: key.to_string(),
        slots: vec![PacketSlot {
            name: "v0".to_string(),
            required: true,
            value_kind: "int".to_string(),
        }],
        patterns: patterns.into_iter().map(str::to_string).collect(),
        bound: true,
    }
}

// ---------------------------------------------------------------------------
// Bounds and truncation
// ---------------------------------------------------------------------------

#[test]
fn trimming_a_group_records_what_it_dropped() {
    let mut packet = packet();
    packet.turns = (0..12).map(|index| turn(index, "summary")).collect();

    packet
        .enforce(&PacketLimits::default())
        .expect("packet fits after trimming");

    assert_eq!(packet.turns.len(), 8);
    let flag = packet
        .truncation
        .iter()
        .find(|flag| flag.group == "turns")
        .expect("a dropped group is flagged");
    assert_eq!(flag.dropped, 4);
}

#[test]
fn a_packet_within_every_bound_records_no_truncation() {
    let mut packet = packet();
    packet.turns = vec![turn(0, "asked about arithmetic")];
    packet.catalog = vec![entry(
        0,
        "arithmetic.multiply",
        vec!["what is {v0} times {v1}"],
    )];

    packet.enforce(&PacketLimits::default()).expect("fits");

    assert!(packet.truncation.is_empty(), "{:?}", packet.truncation);
}

#[test]
fn per_entry_pattern_bound_is_enforced_and_flagged() {
    let mut packet = packet();
    packet.catalog = vec![entry(
        0,
        "arithmetic.multiply",
        vec!["a {v0}", "b {v0}", "c {v0}", "d {v0}", "e {v0}", "f {v0}"],
    )];

    packet.enforce(&PacketLimits::default()).expect("fits");

    assert_eq!(packet.catalog[0].patterns.len(), 4);
    let flag = packet
        .truncation
        .iter()
        .find(|flag| flag.group == "catalogPatterns")
        .expect("dropped patterns are flagged");
    assert_eq!(flag.dropped, 2);
}

#[test]
fn an_over_long_summary_is_clipped_on_a_character_boundary() {
    let mut packet = packet();
    // Multi-byte characters make a naive byte truncation produce invalid UTF-8.
    packet.turns = vec![turn(0, &"é".repeat(400))];

    packet.enforce(&PacketLimits::default()).expect("fits");

    let summary = &packet.turns[0].summary;
    assert!(summary.len() <= 512);
    // Still valid UTF-8, and still whole characters.
    assert!(summary.chars().all(|character| character == 'é'));
    assert!(
        packet
            .truncation
            .iter()
            .any(|flag| flag.group == "turnSummaries")
    );
}

#[test]
fn a_packet_over_the_size_bound_is_an_error_not_a_silent_trim() {
    let mut packet = packet();
    // Each summary sits under the per-summary bound, so only the whole-packet
    // bound can catch this.
    packet.turns = (0..8).map(|index| turn(index, &"x".repeat(500))).collect();
    packet.environment = (0..32)
        .map(|index| PacketEnvFact {
            alias: format!("e{index}"),
            predicate: "k".repeat(200),
            value: Value::Text("v".repeat(200)),
        })
        .collect();
    packet.terminology = (0..64)
        .map(|index| PacketAlias {
            alias: format!("a{index}"),
            surface: "s".repeat(200),
            refers_to: "c0".to_string(),
        })
        .collect();

    let error = packet
        .enforce(&PacketLimits::default())
        .expect_err("an oversized packet is refused");
    assert!(
        error.to_string().contains("context packet bytes"),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// Aliases
// ---------------------------------------------------------------------------

#[test]
fn aliases_cover_every_group_the_packet_exposes() {
    let mut packet = packet();
    packet.turns = vec![PacketTurn {
        alias: "t0".to_string(),
        role: TurnRole::Spoon,
        summary: "answered".to_string(),
        facts: vec![PacketFact {
            alias: "f0".to_string(),
            predicate: "dog.name".to_string(),
            value: Value::Text("Pierre".into()),
        }],
    }];
    packet.catalog = vec![entry(0, "arithmetic.multiply", vec![])];
    packet.terminology = vec![PacketAlias {
        alias: "a0".to_string(),
        surface: "times".to_string(),
        refers_to: "c0".to_string(),
    }];
    packet.environment = vec![PacketEnvFact {
        alias: "e0".to_string(),
        predicate: "clock".to_string(),
        value: Value::Int(8),
    }];

    let aliases = packet.aliases();
    for expected in ["t0", "f0", "c0", "a0", "e0"] {
        assert!(aliases.contains(expected), "missing {expected}");
    }
}

#[test]
fn only_observation_backed_aliases_count_as_fact_provenance() {
    let mut packet = packet();
    packet.turns = vec![PacketTurn {
        alias: "t0".to_string(),
        role: TurnRole::User,
        summary: "said something".to_string(),
        facts: vec![PacketFact {
            alias: "f0".to_string(),
            predicate: "dog.name".to_string(),
            value: Value::Text("Pierre".into()),
        }],
    }];
    packet.catalog = vec![entry(0, "arithmetic.multiply", vec![])];
    packet.environment = vec![PacketEnvFact {
        alias: "e0".to_string(),
        predicate: "clock".to_string(),
        value: Value::Int(8),
    }];

    let facts = packet.fact_aliases();
    assert!(facts.contains("f0"));
    assert!(facts.contains("e0"));
    // A catalog entry is a capability, not an observation. Citing it as the
    // source of a fact would be citing the ability to compute, not evidence.
    assert!(!facts.contains("c0"));
    assert!(!facts.contains("t0"));
}

#[test]
fn a_packet_carrying_a_durable_identifier_is_refused() {
    let mut packet = packet();
    packet.catalog = vec![PacketCatalogEntry {
        alias: "c0".to_string(),
        key: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        slots: Vec::new(),
        patterns: Vec::new(),
        bound: true,
    }];

    let error = packet
        .validate_redaction()
        .expect_err("a leaked identifier is an Engine bug worth catching here");
    assert!(error.to_string().contains("durable identifier"), "{error}");
}

#[test]
fn a_redacted_packet_passes_validation() {
    let mut packet = packet();
    packet.catalog = vec![entry(0, "arithmetic.multiply", vec!["{v0} times {v1}"])];
    packet.validate_redaction().expect("aliases only");
}

// ---------------------------------------------------------------------------
// Supplemental round
// ---------------------------------------------------------------------------

#[test]
fn catalog_detail_must_name_an_alias_the_packet_supplied() {
    let mut packet = packet();
    packet.catalog = vec![entry(0, "arithmetic.multiply", vec![])];

    SupplementalRequest::CatalogDetail {
        alias: "c0".to_string(),
    }
    .validate_for(&packet)
    .expect("a supplied alias resolves");

    let error = SupplementalRequest::CatalogDetail {
        alias: "c7".to_string(),
    }
    .validate_for(&packet)
    .expect_err("an invented alias is refused");
    assert!(
        error.to_string().contains("was not in the packet"),
        "{error}"
    );
}

#[test]
fn a_turn_window_is_bounded_at_both_ends() {
    let packet = packet();

    SupplementalRequest::TurnWindow { count: 1 }
        .validate_for(&packet)
        .expect("one turn is allowed");
    SupplementalRequest::TurnWindow {
        count: MAX_TURN_WINDOW,
    }
    .validate_for(&packet)
    .expect("the maximum is allowed");

    assert!(
        SupplementalRequest::TurnWindow { count: 0 }
            .validate_for(&packet)
            .is_err()
    );
    assert!(
        SupplementalRequest::TurnWindow {
            count: MAX_TURN_WINDOW + 1
        }
        .validate_for(&packet)
        .is_err()
    );
}

#[test]
fn terminology_must_ground_in_the_current_utterance() {
    let packet = packet();

    SupplementalRequest::Terminology {
        source_tokens: TokenRange::new(0, 1),
    }
    .validate_for(&packet)
    .expect("a real token range grounds");

    let error = SupplementalRequest::Terminology {
        source_tokens: TokenRange::new(0, 900),
    }
    .validate_for(&packet)
    .expect_err("a range outside the stream is refused");
    assert!(error.to_string().contains("token range"), "{error}");
}

#[test]
fn the_supplemental_round_is_spent_exactly_once() {
    let mut budget = SupplementalBudget::default();
    assert!(!budget.exhausted());

    budget.consume().expect("the first round is allowed");
    assert!(budget.exhausted());

    let error = budget
        .consume()
        .expect_err("a second round is refused rather than answered");
    assert!(error.to_string().contains("already used"), "{error}");
}

#[test]
fn a_supplemental_request_rejects_unknown_fields() {
    // The wire shape is closed, so a provider cannot smuggle a path or a query
    // alongside a legitimate variant.
    let smuggled = r#"{"turnWindow":{"count":2,"path":"/etc/passwd"}}"#;
    assert!(serde_json::from_str::<SupplementalRequest>(smuggled).is_err());

    let clean = r#"{"turnWindow":{"count":2}}"#;
    let parsed: SupplementalRequest = serde_json::from_str(clean).expect("clean request parses");
    assert_eq!(parsed, SupplementalRequest::TurnWindow { count: 2 });
}
