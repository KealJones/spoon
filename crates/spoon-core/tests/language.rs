use spoon_core::{
    DialogueAct, DialogueMove, EvidenceReference, GroundedClaim, IntentFrame, IntentScope,
    IntentSlot, LanguageLimits, PlannedClaim, RenderVariant, ResponsePlan, ResponseRenderer,
    ResponseTone, SourceKind, TextSpan, Uncertainty, Value, tokenize, tokenize_with_limits,
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
