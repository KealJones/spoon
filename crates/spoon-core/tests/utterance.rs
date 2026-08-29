//! Utterance analysis: grounding, structural validation, and dispatch order.

use std::collections::{BTreeMap, BTreeSet};

use spoon_core::language::{
    DialogueAct, GroundedClaim, IntentDisposition, IntentFrameProposal, IntentScope,
    InterpretationProposal, PlannedClaim, RenderVariant, ResponsePlan, ResponseRenderer,
    ResponseTone, TokenRange, TokenStream, Uncertainty, UncertaintyLevel, tokenize,
};
use spoon_core::utterance::{
    MentionKind, MentionProposal, MentionResolutionProposal, PartId, PartProposal, PartRefRole,
    ResidualPolarity, ResidualProposal, ResidualProvenance, ResidualProvenanceProposal,
    UtteranceAnalysisProposal, UtteranceLimits,
};
use spoon_core::{DialogueMove, EvidenceReference, SourceKind, Value};

/// `"hi 2"` tokenizes to `hi`, whitespace, `2`. Two non-whitespace tokens at
/// known indexes keeps coverage and overlap assertions unambiguous.
fn fixture() -> TokenStream {
    tokenize("hi 2").expect("fixture tokenizes")
}

fn intent(name: &str, disposition: IntentDisposition) -> InterpretationProposal {
    InterpretationProposal {
        candidates: vec![IntentFrameProposal {
            name: name.to_string(),
            confidence: 1.0,
            scope: IntentScope::CurrentTurn,
            source_tokens: Vec::new(),
            slots: Vec::new(),
            ambiguities: Vec::new(),
        }],
        selected: match disposition {
            IntentDisposition::Execute => Some(0),
            _ => None,
        },
        disposition,
    }
}

fn part(id: &str, tokens: TokenRange, act: DialogueAct) -> PartProposal {
    PartProposal {
        id: id.to_string(),
        source_tokens: vec![tokens],
        template: format!("{id} template"),
        act,
        mentions: Vec::new(),
        context_bindings: Vec::new(),
        intent: intent("do", IntentDisposition::Execute),
        residual: Vec::new(),
    }
}

/// Two parts covering both non-whitespace tokens: the baseline valid shape.
fn two_parts() -> UtteranceAnalysisProposal {
    UtteranceAnalysisProposal {
        cleaned: "hi 2".to_string(),
        alignment: Vec::new(),
        parts: vec![
            part("p0", TokenRange::new(0, 1), DialogueAct::Acknowledge),
            part("p1", TokenRange::new(2, 3), DialogueAct::Inform),
        ],
        language_writes: Vec::new(),
    }
}

fn no_aliases() -> BTreeSet<String> {
    BTreeSet::new()
}

// ---------------------------------------------------------------------------
// Grounding succeeds
// ---------------------------------------------------------------------------

#[test]
fn grounds_a_two_part_utterance() {
    let stream = fixture();
    let analysis = two_parts()
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect("valid analysis grounds");

    assert_eq!(analysis.parts.len(), 2);
    assert_eq!(analysis.parts[0].id, PartId::parse("p0").unwrap());
    assert_eq!(analysis.parts[0].act, DialogueAct::Acknowledge);
    // Spans are byte offsets into the original, not token indexes.
    assert_eq!(analysis.parts[0].spans[0].start_byte, 0);
    assert_eq!(analysis.parts[0].spans[0].end_byte, 2);
    assert_eq!(analysis.parts[1].spans[0].start_byte, 3);
    assert_eq!(analysis.parts[1].spans[0].end_byte, 4);
}

#[test]
fn derives_dependencies_and_dispatch_order_from_part_refs() {
    let stream = fixture();
    let mut proposal = two_parts();
    // p0 consumes p1's not-yet-computed result, so p1 must dispatch first even
    // though p0 comes first in the source.
    proposal.parts[0].mentions.push(MentionProposal {
        key: "x0".to_string(),
        kind: MentionKind::Result,
        source_tokens: Vec::new(),
        inferred: true,
        resolved: MentionResolutionProposal::PartRef {
            part: "p1".to_string(),
            role: PartRefRole::Result,
        },
    });

    let analysis = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect("valid analysis grounds");

    let order = analysis.dispatch_order().expect("acyclic");
    assert_eq!(
        order,
        vec![PartId::parse("p1").unwrap(), PartId::parse("p0").unwrap()]
    );
    // Source order is unchanged by execution order. The reply concatenates in
    // source order regardless.
    assert_eq!(
        analysis.source_order(),
        vec![PartId::parse("p0").unwrap(), PartId::parse("p1").unwrap()]
    );

    let dependencies = analysis.depends_on();
    assert!(dependencies[&PartId::parse("p0").unwrap()].contains(&PartId::parse("p1").unwrap()));
    assert!(dependencies[&PartId::parse("p1").unwrap()].is_empty());
}

#[test]
fn independent_parts_keep_source_order_as_the_tie_break() {
    let stream = fixture();
    let analysis = two_parts()
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect("valid analysis grounds");

    assert_eq!(analysis.dispatch_order().unwrap(), analysis.source_order());
}

#[test]
fn unresolved_mention_coerces_an_execute_part_to_clarify() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[1].mentions.push(MentionProposal {
        key: "e0".to_string(),
        kind: MentionKind::Entity,
        source_tokens: vec![TokenRange::new(2, 3)],
        inferred: false,
        resolved: MentionResolutionProposal::Unresolved {
            ambiguity: "which file".to_string(),
        },
    });

    let analysis = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect("grounds, with the part coerced");

    assert_eq!(
        analysis.parts[1].intent.disposition,
        IntentDisposition::Clarify
    );
    assert_eq!(analysis.parts[1].intent.selected, None);
    assert!(!analysis.parts[1].is_executable());
    // The sibling is untouched and still runs.
    assert!(analysis.parts[0].is_executable());
}

// ---------------------------------------------------------------------------
// Structural rejection
// ---------------------------------------------------------------------------

#[test]
fn rejects_overlapping_part_spans() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[0].source_tokens = vec![TokenRange::new(0, 2)];
    proposal.parts[1].source_tokens = vec![TokenRange::new(1, 3)];

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("overlap is rejected");
    assert!(error.to_string().contains("must not overlap"), "{error}");
}

#[test]
fn rejects_a_coverage_gap_over_non_whitespace_tokens() {
    let stream = fixture();
    let mut proposal = two_parts();
    // Drop the part covering `2`. Half the utterance would vanish silently.
    proposal.parts.truncate(1);

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("a coverage gap is rejected");
    assert!(
        error
            .to_string()
            .contains("cover every non-whitespace token"),
        "{error}"
    );
}

#[test]
fn allows_uncovered_whitespace_tokens() {
    let stream = fixture();
    // The whitespace token at index 1 is deliberately owned by no part.
    let analysis = two_parts()
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect("whitespace needs no owner");
    assert_eq!(analysis.parts.len(), 2);
}

#[test]
fn rejects_a_part_ref_to_a_nonexistent_part() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[0].mentions.push(MentionProposal {
        key: "x0".to_string(),
        kind: MentionKind::Result,
        source_tokens: Vec::new(),
        inferred: true,
        resolved: MentionResolutionProposal::PartRef {
            part: "p7".to_string(),
            role: PartRefRole::Result,
        },
    });

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("dangling part reference is rejected");
    assert!(error.to_string().contains("unknown part"), "{error}");
}

#[test]
fn rejects_a_self_referential_part_ref() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[0].mentions.push(MentionProposal {
        key: "x0".to_string(),
        kind: MentionKind::Result,
        source_tokens: Vec::new(),
        inferred: true,
        resolved: MentionResolutionProposal::PartRef {
            part: "p0".to_string(),
            role: PartRefRole::Result,
        },
    });

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("self reference is rejected");
    assert!(error.to_string().contains("refers to itself"), "{error}");
}

#[test]
fn rejects_a_dependency_cycle() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[0].mentions.push(MentionProposal {
        key: "x0".to_string(),
        kind: MentionKind::Result,
        source_tokens: Vec::new(),
        inferred: true,
        resolved: MentionResolutionProposal::PartRef {
            part: "p1".to_string(),
            role: PartRefRole::Result,
        },
    });
    proposal.parts[1].mentions.push(MentionProposal {
        key: "x0".to_string(),
        kind: MentionKind::Result,
        source_tokens: Vec::new(),
        inferred: true,
        resolved: MentionResolutionProposal::PartRef {
            part: "p0".to_string(),
            role: PartRefRole::Result,
        },
    });

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("a cycle is rejected");
    assert!(error.to_string().contains("cycle"), "{error}");
}

#[test]
fn rejects_a_durable_identifier_anywhere_in_the_proposal() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[0].template = "use 550e8400-e29b-41d4-a716-446655440000".to_string();

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("durable identifiers are rejected");
    assert!(error.to_string().contains("durable identifier"), "{error}");
}

#[test]
fn rejects_a_context_ref_to_an_alias_absent_from_the_packet() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[0].mentions.push(MentionProposal {
        key: "e0".to_string(),
        kind: MentionKind::Entity,
        source_tokens: vec![TokenRange::new(0, 1)],
        inferred: false,
        resolved: MentionResolutionProposal::ContextRef {
            alias: "c9".to_string(),
        },
    });

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("unknown alias is rejected");
    assert!(
        error
            .to_string()
            .contains("not supplied in the context packet"),
        "{error}"
    );

    // The same proposal grounds once the alias really was in the packet.
    let aliases = BTreeSet::from(["c9".to_string()]);
    proposal
        .ground_for(&stream, &aliases, &UtteranceLimits::default())
        .expect("supplied alias resolves");
}

#[test]
fn rejects_more_parts_than_the_limit() {
    let text = "a b c d e f g h i";
    let stream = tokenize(text).unwrap();
    let parts: Vec<PartProposal> = (0..9)
        .map(|index| {
            // Words sit at even token indexes, whitespace at odd ones.
            let token = index * 2;
            part(
                &format!("p{index}"),
                TokenRange::new(token, token + 1),
                DialogueAct::Inform,
            )
        })
        .collect();
    let proposal = UtteranceAnalysisProposal {
        cleaned: text.to_string(),
        alignment: Vec::new(),
        parts,
        language_writes: Vec::new(),
    };

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("nine parts exceeds the bound");
    assert!(error.to_string().contains("utterance parts"), "{error}");
}

#[test]
fn rejects_a_non_inferred_mention_with_no_source_span() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[0].mentions.push(MentionProposal {
        key: "e0".to_string(),
        kind: MentionKind::Entity,
        source_tokens: Vec::new(),
        inferred: false,
        resolved: MentionResolutionProposal::Literal {
            value: Value::Text("hi".into()),
        },
    });

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("ungrounded non-inferred mention is rejected");
    assert!(
        error.to_string().contains("must carry a source span"),
        "{error}"
    );
}

#[test]
fn rejects_a_duplicate_part_id() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[1].id = "p0".to_string();

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("duplicate ids are rejected");
    assert!(error.to_string().contains("duplicate part id"), "{error}");
}

#[test]
fn rejects_a_malformed_part_id() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[1].id = "part_1".to_string();

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("malformed ids are rejected");
    assert!(error.to_string().contains("must look like p0"), "{error}");
}

// ---------------------------------------------------------------------------
// Residual facts
// ---------------------------------------------------------------------------

#[test]
fn grounds_a_residual_claim_backed_by_the_utterance() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[0].residual.push(ResidualProposal {
        id: "r0".to_string(),
        predicate: "greeting".to_string(),
        value: Value::Text("hi".into()),
        scope: BTreeMap::new(),
        polarity: ResidualPolarity::Assert,
        provenance: ResidualProvenanceProposal::UtteranceTokens(TokenRange::new(0, 1)),
    });

    let analysis = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect("span-backed residual grounds");

    match &analysis.parts[0].residual[0].provenance {
        ResidualProvenance::Utterance { span } => {
            // Provenance points at what the user actually said.
            assert_eq!(stream.slice(span), Some("hi"));
        }
        other => panic!("expected utterance provenance, got {other:?}"),
    }
}

#[test]
fn rejects_a_residual_citing_an_alias_absent_from_the_packet() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.parts[0].residual.push(ResidualProposal {
        id: "r0".to_string(),
        predicate: "owner".to_string(),
        value: Value::Text("pierre".into()),
        scope: BTreeMap::new(),
        polarity: ResidualPolarity::Assert,
        provenance: ResidualProvenanceProposal::ContextAlias("f4".to_string()),
    });

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("an uncited alias is rejected");
    assert!(
        error
            .to_string()
            .contains("not supplied in the context packet"),
        "{error}"
    );
}

#[test]
fn rejects_more_residuals_than_a_part_may_carry() {
    let stream = fixture();
    let mut proposal = two_parts();
    for index in 0..9 {
        proposal.parts[0].residual.push(ResidualProposal {
            id: format!("r{index}"),
            predicate: format!("p{index}"),
            value: Value::Int(index),
            scope: BTreeMap::new(),
            polarity: ResidualPolarity::Assert,
            provenance: ResidualProvenanceProposal::UtteranceTokens(TokenRange::new(0, 1)),
        });
    }

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("residual bound is enforced");
    assert!(error.to_string().contains("residual claims"), "{error}");
}

// ---------------------------------------------------------------------------
// Alignment
// ---------------------------------------------------------------------------

#[test]
fn alignment_maps_cleaned_ranges_back_onto_complete_original_tokens() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.cleaned = "hi 2".to_string();
    proposal.alignment = vec![
        spoon_core::utterance::AlignmentProposal {
            cleaned_start: 0,
            cleaned_end: 2,
            source_tokens: TokenRange::new(0, 1),
        },
        spoon_core::utterance::AlignmentProposal {
            cleaned_start: 3,
            cleaned_end: 4,
            source_tokens: TokenRange::new(2, 3),
        },
    ];

    let analysis = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect("aligned document grounds");

    let original = analysis
        .cleaned
        .original_span_for(spoon_core::language::TextSpan::new(0, 2))
        .expect("aligned range resolves");
    assert_eq!(stream.slice(&original), Some("hi"));

    // Introduced text has no counterpart, and says so rather than guessing.
    assert_eq!(
        analysis
            .cleaned
            .original_span_for(spoon_core::language::TextSpan::new(2, 3)),
        None
    );
}

#[test]
fn rejects_out_of_order_alignment_entries() {
    let stream = fixture();
    let mut proposal = two_parts();
    proposal.alignment = vec![
        spoon_core::utterance::AlignmentProposal {
            cleaned_start: 3,
            cleaned_end: 4,
            source_tokens: TokenRange::new(2, 3),
        },
        spoon_core::utterance::AlignmentProposal {
            cleaned_start: 0,
            cleaned_end: 2,
            source_tokens: TokenRange::new(0, 1),
        },
    ];

    let error = proposal
        .ground_for(&stream, &no_aliases(), &UtteranceLimits::default())
        .expect_err("unordered alignment is rejected");
    assert!(
        error.to_string().contains("ordered and non-overlapping"),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// Response plan changes
// ---------------------------------------------------------------------------

fn claim(id: &str, text: &str, act: Option<DialogueAct>) -> PlannedClaim {
    PlannedClaim::Grounded(GroundedClaim {
        id: id.to_string(),
        text: text.to_string(),
        evidence: vec![EvidenceReference {
            id: format!("episode:{id}"),
            source_kind: SourceKind::SelfVerified,
            linked_episode: None,
        }],
        provenance: Vec::new(),
        act,
    })
}

#[test]
fn joined_variant_renders_a_multi_part_reply_on_one_line() {
    let plan = ResponsePlan {
        dialogue_move: DialogueMove::new(DialogueAct::Inform),
        claims: vec![
            claim("c0", "Hey.", Some(DialogueAct::Acknowledge)),
            claim("c1", "2 + 2 is 4.", Some(DialogueAct::Inform)),
            claim("c2", "Double that is 8.", Some(DialogueAct::Inform)),
        ],
        uncertainty: Uncertainty::certain(),
        tone: ResponseTone::Neutral,
        variant: RenderVariant::Joined,
    };

    let rendered = ResponseRenderer.render(&plan).expect("plan renders");
    assert_eq!(rendered.text, "Hey. 2 + 2 is 4. Double that is 8.");
    assert_eq!(rendered.included_claim_ids, vec!["c0", "c1", "c2"]);
}

#[test]
fn plain_variant_keeps_its_newline_join_for_existing_callers() {
    let plan = ResponsePlan {
        dialogue_move: DialogueMove::new(DialogueAct::Inform),
        claims: vec![claim("c0", "one", None), claim("c1", "two", None)],
        uncertainty: Uncertainty::certain(),
        tone: ResponseTone::Neutral,
        variant: RenderVariant::Plain,
    };

    assert_eq!(ResponseRenderer.render(&plan).unwrap().text, "one\ntwo");
}

#[test]
fn a_claim_without_an_act_inherits_the_plan_act() {
    let inheriting = GroundedClaim {
        id: "c0".to_string(),
        text: "text".to_string(),
        evidence: Vec::new(),
        provenance: Vec::new(),
        act: None,
    };
    assert_eq!(
        inheriting.effective_act(DialogueAct::Inform),
        DialogueAct::Inform
    );

    let explicit = GroundedClaim {
        act: Some(DialogueAct::Acknowledge),
        ..inheriting
    };
    assert_eq!(
        explicit.effective_act(DialogueAct::Inform),
        DialogueAct::Acknowledge
    );
}

#[test]
fn a_plan_serialized_before_per_claim_acts_still_deserializes() {
    // Exactly the shape stored before `act` existed. Adding the field must not
    // invalidate previously persisted plans.
    let legacy = r#"{
        "id": "c0",
        "text": "2 + 2 is 4.",
        "evidence": [],
        "provenance": []
    }"#;
    let claim: GroundedClaim = serde_json::from_str(legacy).expect("legacy claim deserializes");
    assert_eq!(claim.act, None);

    // And it round-trips without introducing a null field.
    let encoded = serde_json::to_string(&claim).unwrap();
    assert!(!encoded.contains("act"), "{encoded}");
}

#[test]
fn plan_act_precedence_puts_a_pending_question_above_an_answer() {
    use DialogueAct::*;

    // A clarification dominates, because the turn expects a reply.
    assert_eq!(DialogueAct::plan_act(&[Inform, Clarify], 1), Clarify);
    assert_eq!(DialogueAct::plan_act(&[Acknowledge, Ask, Inform], 2), Ask);
    assert_eq!(DialogueAct::plan_act(&[Inform, Refuse], 1), Refuse);
    // A greeting plus two answers informs.
    assert_eq!(
        DialogueAct::plan_act(&[Acknowledge, Inform, Inform], 3),
        Inform
    );
    // A bare greeting only acknowledges.
    assert_eq!(DialogueAct::plan_act(&[Acknowledge], 1), Acknowledge);
    // Nothing rendered means nothing was answered.
    assert_eq!(DialogueAct::plan_act(&[Abstain, Abstain], 0), Abstain);
}

#[test]
fn uncertainty_merges_to_the_weakest_level_and_concatenates_disclosures() {
    let merged = Uncertainty::merge([
        Uncertainty::certain(),
        Uncertainty {
            level: UncertaintyLevel::Qualified,
            disclosure: Some("Rounded to two places.".to_string()),
        },
        Uncertainty {
            level: UncertaintyLevel::Unknown,
            disclosure: Some("The file was not readable.".to_string()),
        },
    ]);

    assert_eq!(merged.level, UncertaintyLevel::Unknown);
    assert_eq!(
        merged.disclosure.as_deref(),
        Some("Rounded to two places. The file was not readable.")
    );
}

#[test]
fn merging_no_uncertainty_stays_certain() {
    let merged = Uncertainty::merge(std::iter::empty());
    assert_eq!(merged.level, UncertaintyLevel::Certain);
    assert_eq!(merged.disclosure, None);
}
