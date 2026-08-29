use spoon_core::{
    DialogueAct, DialogueMove, EvidenceReference, GroundedClaim, IntentDisposition, IntentFrame,
    IntentFrameProposal, IntentFrameSet, IntentScope, IntentSlot, IntentSlotProposal,
    InterpretationProposal, LanguageLimits, PlannedClaim, RenderVariant, ResponsePlan,
    ResponseRenderer, ResponseTone, SourceKind, TextSpan, TokenRange, Uncertainty, Value, tokenize,
    tokenize_with_limits,
};

fn evidence(id: &str) -> EvidenceReference {
    EvidenceReference {
        id: id.into(),
        source_kind: SourceKind::SelfVerified,
        linked_episode: None,
    }
}

fn claim(id: &str, text: &str) -> PlannedClaim {
    PlannedClaim::Grounded(GroundedClaim {
        id: id.into(),
        text: text.into(),
        evidence: vec![evidence("check-1")],
        provenance: vec!["procedure:letter-count-v1".into()],
        act: None,
    })
}

#[test]
fn tokenizer_preserves_unicode_byte_offsets_and_roundtrips_source() {
    let stream = tokenize("hé 🌍!").expect("valid small Unicode input");

    assert_eq!(stream.tokens.len(), 4);
    assert_eq!(stream.tokens[0].span, TextSpan::new(0, 3));
    assert_eq!(stream.tokens[1].span, TextSpan::new(3, 4));
    assert_eq!(stream.tokens[2].span, TextSpan::new(4, 8));
    assert_eq!(stream.tokens[3].span, TextSpan::new(8, 9));
    assert_eq!(stream.slice(&stream.tokens[0].span), Some("hé"));
    assert_eq!(stream.slice(&stream.tokens[2].span), Some("🌍"));
    stream.validate(&LanguageLimits::default()).unwrap();
}

#[test]
fn renderer_is_deterministic_and_can_vary_only_format_not_claims() {
    let plan = ResponsePlan {
        dialogue_move: DialogueMove::new(DialogueAct::Inform),
        claims: vec![claim("answer", "There are 3 r characters in strawberry.")],
        uncertainty: Uncertainty::certain(),
        tone: ResponseTone::Warm,
        variant: RenderVariant::Plain,
    };

    let renderer = ResponseRenderer;
    let first = renderer.render(&plan).unwrap();
    let second = renderer.render(&plan).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.text, "There are 3 r characters in strawberry.");
    assert_eq!(first.included_claim_ids, vec!["answer"]);

    let mut bulleted = plan.clone();
    bulleted.variant = RenderVariant::Bulleted;
    let varied = renderer.render(&bulleted).unwrap();
    assert_ne!(varied.text, first.text);
    assert!(
        varied
            .text
            .contains("There are 3 r characters in strawberry.")
    );
    assert_eq!(varied.included_claim_ids, first.included_claim_ids);
    assert_eq!(varied.tone, ResponseTone::Warm);
}

#[test]
fn renderer_omits_unsupported_and_rejects_ungrounded_claims() {
    let plan = ResponsePlan {
        dialogue_move: DialogueMove::new(DialogueAct::Inform),
        claims: vec![
            claim("verified", "The file contains 12 lines."),
            PlannedClaim::Unsupported {
                id: "guess".into(),
                reason: "No file observation was supplied.".into(),
            },
        ],
        uncertainty: Uncertainty::certain(),
        tone: ResponseTone::Neutral,
        variant: RenderVariant::Plain,
    };

    let rendered = ResponseRenderer.render(&plan).unwrap();
    assert_eq!(rendered.text, "The file contains 12 lines.");
    assert_eq!(rendered.omitted_claim_ids, vec!["guess"]);
    assert!(!rendered.text.contains("No file observation"));

    let mut ungrounded = plan;
    ungrounded.claims = vec![PlannedClaim::Grounded(GroundedClaim {
        id: "not-supported".into(),
        text: "An authority says this is true.".into(),
        evidence: vec![],
        provenance: vec![],
        act: None,
    })];
    assert!(ResponseRenderer.render(&ungrounded).is_err());
}

#[test]
fn language_values_roundtrip_and_enforce_bounds() {
    let stream = tokenize("one two").unwrap();
    let encoded = serde_json::to_string(&stream).unwrap();
    let decoded = serde_json::from_str(&encoded).unwrap();
    assert_eq!(stream, decoded);
    decoded.validate(&LanguageLimits::default()).unwrap();

    let limits = LanguageLimits {
        max_input_bytes: 4,
        ..LanguageLimits::default()
    };
    assert!(tokenize_with_limits("hello", &limits).is_err());
    assert!(stream.validate(&limits).is_err());

    let plan = ResponsePlan {
        dialogue_move: DialogueMove::new(DialogueAct::Inform),
        claims: vec![claim("only", "A grounded claim.")],
        uncertainty: Uncertainty::certain(),
        tone: ResponseTone::Neutral,
        variant: RenderVariant::Plain,
    };
    let no_claims_allowed = LanguageLimits {
        max_claims: 0,
        ..LanguageLimits::default()
    };
    assert!(plan.validate(&no_claims_allowed).is_err());
}

#[test]
fn intent_frame_is_typed_serializable_and_validates_slot_bounds() {
    let frame = IntentFrame {
        name: "count_occurrences".into(),
        confidence: 0.8,
        scope: IntentScope::CurrentTurn,
        source_spans: vec![TextSpan::new(0, 5)],
        slots: vec![IntentSlot {
            name: "needle".into(),
            value: Value::from("r"),
            source_spans: vec![TextSpan::new(0, 1)],
            confidence: 1.0,
        }],
        ambiguities: vec!["literal characters versus normalized graphemes".into()],
    };
    let encoded = serde_json::to_string(&frame).unwrap();
    let decoded: IntentFrame = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, frame);
    decoded.validate(&LanguageLimits::default()).unwrap();

    let no_slots_allowed = LanguageLimits {
        max_slots: 0,
        ..LanguageLimits::default()
    };
    assert!(decoded.validate(&no_slots_allowed).is_err());
}

#[test]
fn intent_frame_set_validates_grounded_execute_candidate() {
    let source = "count r in strawberry";
    let stream = tokenize(source).unwrap();
    let frames = IntentFrameSet {
        candidates: vec![count_intent_frame(source, Vec::new())],
        selected: Some(0),
        disposition: IntentDisposition::Execute,
    };

    frames
        .validate_for(&stream, &LanguageLimits::default())
        .unwrap();
}

#[test]
fn intent_frame_rejects_slot_span_outside_its_document() {
    let source = "count r";
    let stream = tokenize(source).unwrap();
    let mut frame = count_intent_frame(source, Vec::new());
    frame.slots[0].source_spans = vec![TextSpan::new(source.len(), source.len() + 1)];

    assert!(
        frame
            .validate_for(&stream, &LanguageLimits::default())
            .is_err()
    );
}

#[test]
fn intent_frame_rejects_slot_span_inside_multibyte_character() {
    let source = "count é";
    let stream = tokenize(source).unwrap();
    let mut frame = count_intent_frame(source, Vec::new());
    let character = source.find('é').unwrap();
    frame.slots[0].source_spans = vec![TextSpan::new(character + 1, character + 2)];

    assert!(
        frame
            .validate_for(&stream, &LanguageLimits::default())
            .is_err()
    );
}

#[test]
fn intent_frame_set_rejects_out_of_range_selection() {
    let source = "count r in strawberry";
    let stream = tokenize(source).unwrap();
    let frames = IntentFrameSet {
        candidates: vec![count_intent_frame(source, Vec::new())],
        selected: Some(1),
        disposition: IntentDisposition::Execute,
    };

    assert!(
        frames
            .validate_for(&stream, &LanguageLimits::default())
            .is_err()
    );
}

#[test]
fn intent_frame_set_rejects_executing_unresolved_ambiguity() {
    let source = "count letters in café";
    let stream = tokenize(source).unwrap();
    let frames = IntentFrameSet {
        candidates: vec![count_intent_frame(
            source,
            vec!["graphemes versus Unicode scalar values".into()],
        )],
        selected: Some(0),
        disposition: IntentDisposition::Execute,
    };

    assert!(
        frames
            .validate_for(&stream, &LanguageLimits::default())
            .is_err()
    );
}

#[test]
fn intent_frame_set_preserves_competing_frames_for_clarification() {
    let source = "count letters in café";
    let stream = tokenize(source).unwrap();
    let frames = IntentFrameSet {
        candidates: vec![
            count_intent_frame(source, vec!["use grapheme clusters".into()]),
            count_intent_frame(source, vec!["use Unicode scalar values".into()]),
        ],
        selected: None,
        disposition: IntentDisposition::Clarify,
    };

    frames
        .validate_for(&stream, &LanguageLimits::default())
        .unwrap();
    let encoded = serde_json::to_string(&frames).unwrap();
    let decoded: IntentFrameSet = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, frames);
}

#[test]
fn intent_frame_set_allows_empty_abstention() {
    let stream = tokenize("do something unknowable").unwrap();
    let frames = IntentFrameSet {
        candidates: Vec::new(),
        selected: None,
        disposition: IntentDisposition::Abstain,
    };

    frames
        .validate_for(&stream, &LanguageLimits::default())
        .unwrap();
}

#[test]
fn intent_frame_set_enforces_candidate_limit() {
    let source = "count r in strawberry";
    let stream = tokenize(source).unwrap();
    let frames = IntentFrameSet {
        candidates: vec![count_intent_frame(source, Vec::new())],
        selected: Some(0),
        disposition: IntentDisposition::Execute,
    };
    let no_candidates_allowed = LanguageLimits {
        max_intent_candidates: 0,
        ..LanguageLimits::default()
    };

    assert!(
        frames
            .validate_for(&stream, &no_candidates_allowed)
            .is_err()
    );
}

#[test]
fn interpretation_proposal_grounds_token_ranges_into_exact_byte_spans() {
    let stream = tokenize("please count é in café").unwrap();
    let proposal = InterpretationProposal {
        candidates: vec![IntentFrameProposal {
            name: "text.count_occurrences".into(),
            confidence: 0.94,
            scope: IntentScope::CurrentTurn,
            source_tokens: vec![TokenRange::new(2, 9)],
            slots: vec![
                IntentSlotProposal {
                    name: "target".into(),
                    confidence: 0.99,
                    source_tokens: vec![TokenRange::new(4, 5)],
                    inferred_value: None,
                },
                IntentSlotProposal {
                    name: "text".into(),
                    confidence: 0.99,
                    source_tokens: vec![TokenRange::new(8, 9)],
                    inferred_value: None,
                },
            ],
            ambiguities: Vec::new(),
        }],
        selected: Some(0),
        disposition: IntentDisposition::Execute,
    };

    let grounded = proposal
        .ground_for(&stream, &LanguageLimits::default())
        .unwrap();
    assert_eq!(
        grounded.candidates[0].source_spans,
        vec![TextSpan::new(7, 24)]
    );
    assert_eq!(
        grounded.candidates[0].slots[0].value,
        Value::Text("é".into())
    );
    assert_eq!(
        grounded.candidates[0].slots[0].source_spans,
        vec![TextSpan::new(13, 15)]
    );
    assert_eq!(
        grounded.candidates[0].slots[1].value,
        Value::Text("café".into())
    );
}

#[test]
fn interpretation_proposal_rejects_invalid_token_ranges() {
    let stream = tokenize("double 7").unwrap();
    let proposal = InterpretationProposal {
        candidates: vec![IntentFrameProposal {
            name: "number.double".into(),
            confidence: 1.0,
            scope: IntentScope::CurrentTurn,
            source_tokens: vec![TokenRange::new(0, 99)],
            slots: Vec::new(),
            ambiguities: Vec::new(),
        }],
        selected: Some(0),
        disposition: IntentDisposition::Execute,
    };

    assert!(
        proposal
            .ground_for(&stream, &LanguageLimits::default())
            .is_err()
    );
}

#[test]
fn interpretation_proposal_derives_literals_and_marks_inferred_values() {
    let stream = tokenize("double 7").unwrap();
    let proposal = InterpretationProposal {
        candidates: vec![IntentFrameProposal {
            name: "number.double".into(),
            confidence: 1.0,
            scope: IntentScope::CurrentTurn,
            source_tokens: vec![TokenRange::new(0, 3)],
            slots: vec![
                IntentSlotProposal {
                    name: "x".into(),
                    confidence: 1.0,
                    source_tokens: vec![TokenRange::new(2, 3)],
                    inferred_value: None,
                },
                IntentSlotProposal {
                    name: "format".into(),
                    confidence: 0.7,
                    source_tokens: Vec::new(),
                    inferred_value: Some(Value::Text("decimal".into())),
                },
            ],
            ambiguities: Vec::new(),
        }],
        selected: Some(0),
        disposition: IntentDisposition::Execute,
    };

    let grounded = proposal
        .ground_for(&stream, &LanguageLimits::default())
        .unwrap();
    assert_eq!(grounded.candidates[0].slots[0].value, Value::Int(7));
    assert_eq!(
        grounded.candidates[0].slots[1].value,
        Value::Text("decimal".into())
    );
    assert!(grounded.candidates[0].slots[1].source_spans.is_empty());
}

#[test]
fn interpretation_proposal_rejects_slot_with_both_or_neither_value_source() {
    let stream = tokenize("double 7").unwrap();
    for slot in [
        IntentSlotProposal {
            name: "x".into(),
            confidence: 1.0,
            source_tokens: vec![TokenRange::new(2, 3)],
            inferred_value: Some(Value::Int(7)),
        },
        IntentSlotProposal {
            name: "x".into(),
            confidence: 1.0,
            source_tokens: Vec::new(),
            inferred_value: None,
        },
    ] {
        let proposal = InterpretationProposal {
            candidates: vec![IntentFrameProposal {
                name: "number.double".into(),
                confidence: 1.0,
                scope: IntentScope::CurrentTurn,
                source_tokens: vec![TokenRange::new(0, 3)],
                slots: vec![slot],
                ambiguities: Vec::new(),
            }],
            selected: Some(0),
            disposition: IntentDisposition::Execute,
        };
        assert!(
            proposal
                .ground_for(&stream, &LanguageLimits::default())
                .is_err()
        );
    }
}

fn count_intent_frame(source: &str, ambiguities: Vec<String>) -> IntentFrame {
    let (target, target_start) = source
        .find(" r ")
        .map(|start| ("r", start + 1))
        .or_else(|| source.find("letters").map(|start| ("letters", start)))
        .unwrap_or((source, 0));
    IntentFrame {
        name: "text.count_occurrences".into(),
        confidence: 0.9,
        scope: IntentScope::CurrentTurn,
        source_spans: vec![TextSpan::new(0, source.len())],
        slots: vec![IntentSlot {
            name: "target".into(),
            value: Value::from(target),
            source_spans: vec![TextSpan::new(target_start, target_start + target.len())],
            confidence: 0.95,
        }],
        ambiguities,
    }
}
