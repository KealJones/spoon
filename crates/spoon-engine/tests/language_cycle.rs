//! Packet projection and multi-part suspend/resume.

use std::collections::BTreeSet;

use spoon_core::language::{
    DialogueAct, IntentDisposition, IntentFrameProposal, IntentScope, InterpretationProposal,
    ResponseRenderer, ResponseTone, TextSpan, TokenRange, tokenize,
};
use spoon_core::packet::{PacketLimits, TurnRole};
use spoon_core::utterance::{
    MentionKind, MentionProposal, MentionResolutionProposal, PartId, PartProposal, PartRefRole,
    UtteranceAnalysis, UtteranceAnalysisProposal, UtteranceLimits,
};
use spoon_core::{EpisodeId, Lifecycle, Value};
use spoon_engine::intent_catalog::{IntentCatalogEntry, IntentCatalogPattern, IntentSlotSchema};
use spoon_engine::language_cycle::{
    PacketSources, PendingPartsCycle, SuspendReason, TurnSource, build_packet,
};
use spoon_engine::parts::{EvidenceOrigin, PartOutcome, PartState, PartsRun};

fn slot(name: &str) -> IntentSlotSchema {
    IntentSlotSchema {
        name: name.to_string(),
        required: true,
        value_kind: "int".to_string(),
    }
}

fn entry(key: &str, bound: bool) -> IntentCatalogEntry {
    IntentCatalogEntry {
        key: key.to_string(),
        slots: vec![slot("v0"), slot("v1")],
        concept_id: bound.then(|| "concept".to_string()),
        procedure_id: bound.then(|| "procedure".to_string()),
        procedure_version: bound.then_some(1),
        lifecycle: Lifecycle::Active,
        created_at: 0,
    }
}

fn pattern(key: &str, text: &str, support: u32) -> IntentCatalogPattern {
    IntentCatalogPattern {
        key: key.to_string(),
        skeleton: text.to_string(),
        pattern: text.to_string(),
        support,
        contradictions: 0,
        lifecycle: Lifecycle::Active,
        first_episode: "e0".to_string(),
        last_episode: "e1".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Packet projection
// ---------------------------------------------------------------------------

#[test]
fn the_packet_exposes_engine_state_only_through_aliases() {
    let sources = PacketSources {
        catalog: vec![(
            entry("arithmetic.multiply", true),
            vec![pattern("arithmetic.multiply", "what is {v0} times {v1}", 3)],
        )],
        turns: vec![TurnSource {
            role: TurnRole::User,
            summary: "asked about arithmetic".to_string(),
            facts: vec![("dog.name".to_string(), Value::Text("Pierre".into()))],
        }],
        terminology: vec![("times".to_string(), "arithmetic.multiply".to_string())],
        environment: vec![("clock".to_string(), Value::Int(8))],
    };

    let packet = build_packet(
        tokenize("what is 2 times 3").expect("tokenizes"),
        &sources,
        &PacketLimits::default(),
    )
    .expect("packet builds");

    assert_eq!(packet.catalog[0].alias, "c0");
    assert_eq!(packet.catalog[0].key, "arithmetic.multiply");
    assert!(packet.catalog[0].bound);
    assert_eq!(packet.turns[0].alias, "t0");
    assert_eq!(packet.turns[0].facts[0].alias, "f0");
    assert_eq!(packet.environment[0].alias, "e0");
    // Terminology points at the catalog alias, never at the key's storage.
    assert_eq!(packet.terminology[0].refers_to, "c0");

    // The observation is citable as fact provenance; the capability is not.
    let facts = packet.fact_aliases();
    assert!(facts.contains("f0"));
    assert!(facts.contains("e0"));
    assert!(!facts.contains("c0"));
}

#[test]
fn an_unbound_catalog_entry_is_marked_unbound() {
    let sources = PacketSources {
        catalog: vec![(entry("arithmetic.multiply", false), Vec::new())],
        ..PacketSources::default()
    };

    let packet = build_packet(
        tokenize("multiply").expect("tokenizes"),
        &sources,
        &PacketLimits::default(),
    )
    .expect("packet builds");

    assert!(!packet.catalog[0].bound);
}

#[test]
fn patterns_are_offered_highest_support_first() {
    let sources = PacketSources {
        catalog: vec![(
            entry("arithmetic.multiply", true),
            vec![
                pattern("arithmetic.multiply", "weak {v0}", 1),
                pattern("arithmetic.multiply", "strong {v0}", 9),
                pattern("arithmetic.multiply", "middle {v0}", 5),
            ],
        )],
        ..PacketSources::default()
    };

    let packet = build_packet(
        tokenize("multiply").expect("tokenizes"),
        &sources,
        &PacketLimits::default(),
    )
    .expect("packet builds");

    assert_eq!(
        packet.catalog[0].patterns,
        vec!["strong {v0}", "middle {v0}", "weak {v0}"]
    );
}

#[test]
fn a_term_pointing_at_a_key_the_packet_does_not_carry_is_dropped() {
    let sources = PacketSources {
        catalog: vec![(entry("arithmetic.multiply", true), Vec::new())],
        // "divide" is not in the packet, so this alias would dangle.
        terminology: vec![("over".to_string(), "arithmetic.divide".to_string())],
        ..PacketSources::default()
    };

    let packet = build_packet(
        tokenize("multiply").expect("tokenizes"),
        &sources,
        &PacketLimits::default(),
    )
    .expect("packet builds");

    assert!(packet.terminology.is_empty());
}

#[test]
fn projection_flags_what_a_bound_removed() {
    let sources = PacketSources {
        turns: (0..12)
            .map(|index| TurnSource {
                role: TurnRole::User,
                summary: format!("turn {index}"),
                facts: Vec::new(),
            })
            .collect(),
        ..PacketSources::default()
    };

    let packet = build_packet(
        tokenize("hello").expect("tokenizes"),
        &sources,
        &PacketLimits::default(),
    )
    .expect("packet builds");

    assert_eq!(packet.turns.len(), 8);
    assert!(
        packet
            .truncation
            .iter()
            .any(|flag| flag.group == "turns" && flag.dropped == 4)
    );
}

// ---------------------------------------------------------------------------
// Suspend and resume
// ---------------------------------------------------------------------------

fn two_part_analysis() -> UtteranceAnalysis {
    let text = "hi 2";
    let stream = tokenize(text).expect("tokenizes");
    let mut consumer = part("p1", TokenRange::new(2, 3));
    consumer.mentions.push(MentionProposal {
        key: "x0".to_string(),
        kind: MentionKind::Result,
        source_tokens: Vec::new(),
        inferred: true,
        resolved: MentionResolutionProposal::PartRef {
            part: "p0".to_string(),
            role: PartRefRole::Result,
        },
    });

    UtteranceAnalysisProposal {
        cleaned: text.to_string(),
        alignment: Vec::new(),
        parts: vec![part("p0", TokenRange::new(0, 1)), consumer],
        language_writes: Vec::new(),
    }
    .ground_for(&stream, &BTreeSet::new(), &UtteranceLimits::default())
    .expect("grounds")
}

fn part(id: &str, tokens: TokenRange) -> PartProposal {
    PartProposal {
        id: id.to_string(),
        source_tokens: vec![tokens],
        template: format!("{id} template"),
        act: DialogueAct::Inform,
        mentions: Vec::new(),
        context_bindings: Vec::new(),
        intent: InterpretationProposal {
            candidates: vec![IntentFrameProposal {
                name: "do".to_string(),
                confidence: 1.0,
                scope: IntentScope::CurrentTurn,
                source_tokens: Vec::new(),
                slots: Vec::new(),
                ambiguities: Vec::new(),
            }],
            selected: Some(0),
            disposition: IntentDisposition::Execute,
        },
        residual: Vec::new(),
    }
}

fn id(raw: &str) -> PartId {
    PartId::parse(raw).expect("valid")
}

#[test]
fn a_suspend_keeps_the_work_already_done() {
    let run = PartsRun::new(two_part_analysis(), EpisodeId::new()).expect("acyclic");
    let mut pending = PendingPartsCycle::new(run, id("p1"), SuspendReason::Teacher);

    pending.run.record(PartOutcome::executed(
        id("p0"),
        Value::Int(4),
        "2 + 2 is 4.",
        EvidenceOrigin::Procedure {
            id: "add".to_string(),
            version: 1,
        },
    ));

    // Suspend and restore exactly as the Engine persists it.
    let encoded = serde_json::to_string(&pending).expect("serializes");
    let restored: PendingPartsCycle = serde_json::from_str(&encoded).expect("deserializes");

    assert_eq!(restored.blocked_on, id("p1"));
    assert_eq!(restored.reason, SuspendReason::Teacher);
    assert_eq!(restored.run.outcomes[&id("p0")].state, PartState::Executed);
    assert_eq!(restored.run.resolved_value(&id("p0")), Some(&Value::Int(4)));
}

#[test]
fn resuming_does_not_re_derive_the_analysis_or_the_order() {
    let run = PartsRun::new(two_part_analysis(), EpisodeId::new()).expect("acyclic");
    let mut pending = PendingPartsCycle::new(run, id("p1"), SuspendReason::Teacher);
    let before = pending.frozen_digest();

    pending.run.record(PartOutcome::executed(
        id("p0"),
        Value::Int(4),
        "2 + 2 is 4.",
        EvidenceOrigin::Procedure {
            id: "add".to_string(),
            version: 1,
        },
    ));

    let encoded = serde_json::to_string(&pending).expect("serializes");
    let mut restored: PendingPartsCycle = serde_json::from_str(&encoded).expect("deserializes");

    // The lesson arrives and the blocked part finally runs.
    restored.run.record(PartOutcome::executed(
        id("p1"),
        Value::Int(8),
        "Double that is 8.",
        EvidenceOrigin::Procedure {
            id: "double".to_string(),
            version: 1,
        },
    ));

    // Segmentation is identical across the suspend. A model asked to segment
    // the same utterance twice could answer differently, which would orphan
    // p0's outcome.
    assert_eq!(restored.frozen_digest(), before);
    assert!(restored.run.is_complete());
    assert_eq!(restored.run.executed_count(), 2);
}

#[test]
fn each_turn_renders_only_what_that_turn_completed() {
    let run = PartsRun::new(two_part_analysis(), EpisodeId::new()).expect("acyclic");
    let mut pending = PendingPartsCycle::new(run, id("p1"), SuspendReason::Clarification);

    pending.run.record(PartOutcome::executed(
        id("p0"),
        Value::Int(4),
        "2 + 2 is 4.",
        EvidenceOrigin::Procedure {
            id: "add".to_string(),
            version: 1,
        },
    ));
    pending.run.record(PartOutcome::clarified(
        id("p1"),
        "Which value should I double?",
        TextSpan::new(0, 2),
    ));

    let first = pending
        .run
        .response_plan(&pending.turn_label(), ResponseTone::Neutral);
    let first_text = ResponseRenderer.render(&first).expect("renders").text;
    assert_eq!(first_text, "2 + 2 is 4. Which value should I double?");

    // The user replies. Only the newly unblocked part is answered.
    let label = pending.next_turn();
    pending.run.outcomes.remove(&id("p1"));
    pending.run.record(PartOutcome::executed(
        id("p1"),
        Value::Int(8),
        "Double that is 8.",
        EvidenceOrigin::Procedure {
            id: "double".to_string(),
            version: 1,
        },
    ));

    let second = pending.run.response_plan(&label, ResponseTone::Neutral);
    let second_text = ResponseRenderer.render(&second).expect("renders").text;
    assert_eq!(second_text, "Double that is 8.");
    // No double answer: the first part already rendered in turn one.
    assert!(!second_text.contains("2 + 2 is 4"));
    assert_eq!(
        pending.run.outcomes[&id("p0")].rendered_in_turn.as_deref(),
        Some("turn-1")
    );
    assert_eq!(
        pending.run.outcomes[&id("p1")].rendered_in_turn.as_deref(),
        Some("turn-2")
    );
}
