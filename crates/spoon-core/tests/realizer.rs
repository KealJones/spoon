//! The template realizer: a model picks a shape, the Engine writes every char.

use std::collections::{BTreeMap, BTreeSet};

use spoon_core::language::{
    DialogueAct, DialogueMove, GroundedClaim, PlannedClaim, RenderVariant, ResponsePlan,
    ResponseRenderer, ResponseTone, TokenStream, Uncertainty, tokenize,
};
use spoon_core::realizer::{ClaimDependencies, RealizationProposal, TEMPLATES, template};
use spoon_core::{EvidenceReference, SourceKind};

fn utterance() -> TokenStream {
    tokenize("hey whats 2+2 and then double that").expect("tokenizes")
}

fn claim(id: &str, text: &str, act: DialogueAct) -> PlannedClaim {
    PlannedClaim::Grounded(GroundedClaim {
        id: id.to_string(),
        text: text.to_string(),
        evidence: vec![EvidenceReference {
            id: format!("ep:{id}"),
            source_kind: SourceKind::SelfVerified,
            linked_episode: None,
        }],
        provenance: Vec::new(),
        act: Some(act),
    })
}

/// The worked example from the spec: a greeting and two answers, the second
/// consuming the first.
fn worked_plan() -> ResponsePlan {
    ResponsePlan {
        dialogue_move: DialogueMove::new(DialogueAct::Inform),
        claims: vec![
            claim("c0", "Hey.", DialogueAct::Acknowledge),
            claim("c1", "2 + 2 is 4.", DialogueAct::Inform),
            claim("c2", "Double that is 8.", DialogueAct::Inform),
        ],
        uncertainty: Uncertainty::certain(),
        tone: ResponseTone::Neutral,
        variant: RenderVariant::Joined,
    }
}

fn worked_dependencies() -> ClaimDependencies {
    BTreeMap::from([("c2".to_string(), BTreeSet::from(["c1".to_string()]))])
}

fn proposal(id: &str, order: &[&str]) -> RealizationProposal {
    RealizationProposal {
        template_id: id.to_string(),
        slot_order: order.iter().map(|id| id.to_string()).collect(),
        tone: ResponseTone::Neutral,
    }
}

// ---------------------------------------------------------------------------
// Realizing
// ---------------------------------------------------------------------------

#[test]
fn stitches_a_greeting_and_two_answers_into_one_reply() {
    let realized = proposal("join.ack.and", &["c0", "c1", "c2"])
        .realize(&worked_plan(), &worked_dependencies(), &utterance())
        .expect("valid realization");

    assert_eq!(realized.text, "Hey. 2 + 2 is 4, and double that is 8.");
    // Every claim's content survives verbatim; only sentence mechanics moved.
    assert!(realized.text.contains("2 + 2 is 4"));
    assert!(realized.text.contains("8."));
}

#[test]
fn the_deterministic_renderer_stays_the_fallback_for_the_same_plan() {
    let rendered = ResponseRenderer
        .render(&worked_plan())
        .expect("plan renders");
    assert_eq!(rendered.text, "Hey. 2 + 2 is 4. Double that is 8.");
}

#[test]
fn variadic_join_accepts_any_claim_count() {
    let realized = proposal("join.sentences", &["c0", "c1", "c2"])
        .realize(&worked_plan(), &worked_dependencies(), &utterance())
        .expect("variadic realization");
    assert_eq!(realized.text, "Hey. 2 + 2 is 4. Double that is 8.");
}

#[test]
fn tone_selects_an_engine_owned_wording_not_model_text() {
    let plan = ResponsePlan {
        claims: vec![
            claim("c0", "The file is open.", DialogueAct::Inform),
            claim("c1", "It has 4 lines.", DialogueAct::Inform),
        ],
        ..worked_plan()
    };
    let formal = RealizationProposal {
        template_id: "join.and".to_string(),
        slot_order: vec!["c0".to_string(), "c1".to_string()],
        tone: ResponseTone::Formal,
    };

    let realized = formal
        .realize(&plan, &ClaimDependencies::new(), &utterance())
        .expect("formal realization");
    assert_eq!(
        realized.text,
        "The file is open, and additionally It has 4 lines."
    );
}

#[test]
fn a_terminator_is_stripped_only_where_the_template_continues_the_sentence() {
    let plan = ResponsePlan {
        claims: vec![
            claim("c0", "Two is two.", DialogueAct::Inform),
            claim("c1", "Three is three.", DialogueAct::Inform),
        ],
        ..worked_plan()
    };

    let realized = proposal("join.and", &["c0", "c1"])
        .realize(&plan, &ClaimDependencies::new(), &utterance())
        .expect("realizes");
    // First claim loses its period, the last keeps it.
    assert_eq!(realized.text, "Two is two, and Three is three.");
}

#[test]
fn an_ellipsis_is_not_collapsed_into_a_period() {
    let plan = ResponsePlan {
        claims: vec![
            claim("c0", "It is still running...", DialogueAct::Inform),
            claim("c1", "Check back later.", DialogueAct::Inform),
        ],
        ..worked_plan()
    };

    let realized = proposal("join.and", &["c0", "c1"])
        .realize(&plan, &ClaimDependencies::new(), &utterance())
        .expect("realizes");
    // Trimming one dot would change what the claim said.
    assert!(
        realized.text.starts_with("It is still running..."),
        "{}",
        realized.text
    );
}

// ---------------------------------------------------------------------------
// Case evidence
// ---------------------------------------------------------------------------

#[test]
fn lowercases_a_mid_sentence_initial_only_with_evidence_from_the_utterance() {
    // The user typed "double" lowercase, so lowercasing it is grounded.
    let realized = proposal("join.ack.and", &["c0", "c1", "c2"])
        .realize(&worked_plan(), &worked_dependencies(), &utterance())
        .expect("realizes");
    assert!(
        realized.text.contains("and double that"),
        "{}",
        realized.text
    );
}

#[test]
fn a_proper_noun_is_never_decapitalized_to_make_a_sentence_flow() {
    let stream = tokenize("whats 2+2 and who owns it").expect("tokenizes");
    let plan = ResponsePlan {
        claims: vec![
            claim("c0", "2 + 2 is 4.", DialogueAct::Inform),
            claim("c1", "Pierre owns it.", DialogueAct::Inform),
        ],
        ..worked_plan()
    };

    let realized = proposal("join.and", &["c0", "c1"])
        .realize(&plan, &ClaimDependencies::new(), &stream)
        .expect("realizes");
    // "Pierre" never appeared lowercase, so it keeps its capital even though
    // the template would otherwise lowercase that slot.
    assert_eq!(realized.text, "2 + 2 is 4, and Pierre owns it.");
}

// ---------------------------------------------------------------------------
// Rejection
// ---------------------------------------------------------------------------

#[test]
fn rejects_a_template_outside_the_pinned_set() {
    let error = proposal("join.freestyle", &["c0", "c1", "c2"])
        .realize(&worked_plan(), &worked_dependencies(), &utterance())
        .expect_err("an unknown template is refused");
    assert!(
        error.to_string().contains("not in the pinned set"),
        "{error}"
    );
}

#[test]
fn rejects_a_slot_order_that_drops_a_claim() {
    let error = proposal("join.sentences", &["c0", "c1"])
        .realize(&worked_plan(), &worked_dependencies(), &utterance())
        .expect_err("dropping an answer is refused");
    assert!(error.to_string().contains("slot order covers"), "{error}");
}

#[test]
fn rejects_a_slot_order_that_repeats_a_claim() {
    let error = proposal("join.ack.and", &["c0", "c1", "c1"])
        .realize(&worked_plan(), &worked_dependencies(), &utterance())
        .expect_err("asserting a claim twice is refused");
    assert!(error.to_string().contains("repeats claim"), "{error}");
}

#[test]
fn rejects_a_slot_order_naming_an_unknown_claim() {
    let error = proposal("join.ack.and", &["c0", "c1", "c9"])
        .realize(&worked_plan(), &worked_dependencies(), &utterance())
        .expect_err("an invented claim id is refused");
    assert!(
        error.to_string().contains("not a grounded claim"),
        "{error}"
    );
}

#[test]
fn rejects_presenting_an_unsupported_claim_as_a_fact() {
    // Two grounded claims plus one that failed. An Unsupported claim is not
    // counted toward arity, so the order below is the right length and the
    // grounded-claim guard is what has to catch it.
    let mut plan = worked_plan();
    plan.claims[2] = PlannedClaim::Unsupported {
        id: "c2".to_string(),
        reason: "the procedure failed".to_string(),
    };

    let error = proposal("join.and", &["c0", "c2"])
        .realize(&plan, &worked_dependencies(), &utterance())
        .expect_err("an unsupported claim cannot be worded as a fact");
    assert!(
        error.to_string().contains("not a grounded claim"),
        "{error}"
    );
}

#[test]
fn an_unsupported_claim_does_not_count_toward_arity() {
    let mut plan = worked_plan();
    plan.claims[2] = PlannedClaim::Unsupported {
        id: "c2".to_string(),
        reason: "the procedure failed".to_string(),
    };

    // Three claims in the plan, but only two can be worded.
    let realized = proposal("join.and", &["c0", "c1"])
        .realize(&plan, &worked_dependencies(), &utterance())
        .expect("the two grounded claims still realize");
    assert_eq!(realized.text, "Hey, and 2 + 2 is 4.");
}

#[test]
fn rejects_wording_a_consumer_before_its_producer() {
    // "Double that is 8, and 2 + 2 is 4" reads as though the arithmetic
    // followed from the doubling.
    let error = proposal("join.ack.and", &["c0", "c2", "c1"])
        .realize(&worked_plan(), &worked_dependencies(), &utterance())
        .expect_err("a reversed dependency is refused");
    assert!(
        error.to_string().contains("cannot be worded before it"),
        "{error}"
    );
}

#[test]
fn rejects_an_act_constraint_violation() {
    let error = proposal("join.ack.and", &["c1", "c0", "c2"])
        .realize(&worked_plan(), &worked_dependencies(), &utterance())
        .expect_err("a non-greeting cannot lead an acknowledgement template");
    assert!(error.to_string().contains("requires"), "{error}");
}

#[test]
fn rejects_an_arity_mismatch() {
    let error = proposal("join.and", &["c0", "c1", "c2"])
        .realize(&worked_plan(), &worked_dependencies(), &utterance())
        .expect_err("arity is checked");
    assert!(error.to_string().contains("takes 2 claims"), "{error}");
}

#[test]
fn rejects_a_sequence_template_when_no_sequence_exists() {
    let plan = ResponsePlan {
        claims: vec![
            claim("c0", "The file is open.", DialogueAct::Inform),
            claim("c1", "The clock says 8.", DialogueAct::Inform),
        ],
        ..worked_plan()
    };

    let error = proposal("join.then", &["c0", "c1"])
        .realize(&plan, &ClaimDependencies::new(), &utterance())
        .expect_err("independent claims are not a sequence");
    assert!(error.to_string().contains("asserts a sequence"), "{error}");
}

#[test]
fn accepts_a_sequence_template_when_the_dependency_is_real() {
    let plan = ResponsePlan {
        claims: vec![
            claim("c1", "2 + 2 is 4.", DialogueAct::Inform),
            claim("c2", "Double that is 8.", DialogueAct::Inform),
        ],
        ..worked_plan()
    };

    let realized = proposal("join.then", &["c1", "c2"])
        .realize(&plan, &worked_dependencies(), &utterance())
        .expect("a real sequence realizes");
    assert_eq!(realized.text, "2 + 2 is 4. Then double that is 8.");
}

#[test]
fn rejects_a_plan_with_no_grounded_claims() {
    let plan = ResponsePlan {
        claims: vec![PlannedClaim::Unsupported {
            id: "c0".to_string(),
            reason: "abstained".to_string(),
        }],
        ..worked_plan()
    };

    let error = proposal("join.sentences", &[])
        .realize(&plan, &ClaimDependencies::new(), &utterance())
        .expect_err("nothing grounded means nothing to realize");
    assert!(
        error.to_string().contains("at least one grounded claim"),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// The template set itself
// ---------------------------------------------------------------------------

#[test]
fn every_pinned_template_is_internally_consistent() {
    for pinned in TEMPLATES {
        if let spoon_core::realizer::TemplateArity::Exact(arity) = pinned.arity {
            assert_eq!(
                pinned.slot_acts.len(),
                arity,
                "{} declares {} slot acts for arity {arity}",
                pinned.id,
                pinned.slot_acts.len()
            );
            assert_eq!(
                pinned.mechanics.len(),
                arity,
                "{} declares {} mechanics for arity {arity}",
                pinned.id,
                pinned.mechanics.len()
            );
        }
        assert!(template(pinned.id).is_some());
    }
}

#[test]
fn no_pinned_template_contributes_a_content_word() {
    // Everything a template supplies outside its placeholders must be
    // connective tissue. A template that introduced a claim word would be
    // authoring content the plan never held.
    const ALLOWED: &[&str] = &["and", "additionally", "then", "subsequently"];
    for pinned in TEMPLATES {
        let forms = match pinned.shape {
            spoon_core::realizer::TemplateShape::Fixed(forms) => forms,
            spoon_core::realizer::TemplateShape::Joined(forms) => forms,
        };
        for form in [forms.neutral, forms.direct, forms.warm, forms.formal] {
            let stripped: String = form
                .chars()
                .map(|character| {
                    if character.is_alphabetic() {
                        character
                    } else {
                        ' '
                    }
                })
                .collect();
            for word in stripped.split_whitespace() {
                assert!(
                    ALLOWED.contains(&word.to_lowercase().as_str()),
                    "template {} supplies content word {word:?}",
                    pinned.id
                );
            }
        }
    }
}
