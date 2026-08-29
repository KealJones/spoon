//! Per-part dispatch: independent speech acts finishing at different times.

use std::collections::BTreeSet;

use spoon_core::language::{
    DialogueAct, IntentDisposition, IntentFrameProposal, IntentScope, InterpretationProposal,
    PlannedClaim, ResponseRenderer, ResponseTone, TextSpan, TokenRange, TokenStream, Uncertainty,
    UncertaintyLevel, tokenize,
};
use spoon_core::utterance::{
    MentionKind, MentionProposal, MentionResolutionProposal, PartId, PartProposal, PartRefRole,
    UtteranceAnalysis, UtteranceAnalysisProposal, UtteranceLimits,
};
use spoon_core::{EpisodeId, Value};
use spoon_engine::parts::{EvidenceOrigin, PartOutcome, PartState, PartsRun, claim_id};

/// "hey whats 2+2 and then double that" as three parts. Token indexes are
/// computed from the real tokenizer so the fixture cannot drift.
fn worked_analysis() -> (TokenStream, UtteranceAnalysis) {
    let text = "hey whats 2+2 and then double that";
    let stream = tokenize(text).expect("tokenizes");
    let total = stream.tokens.len();

    // p0 = "hey", p1 = through "2+2", p2 = the rest. Split points are found by
    // byte offset so a tokenizer change surfaces as a test failure, not a
    // silently wrong fixture.
    let after_hey = token_ending_at(&stream, 3);
    let after_sum = token_ending_at(&stream, text.find(" and").expect("has and"));

    let proposal = UtteranceAnalysisProposal {
        cleaned: text.to_string(),
        alignment: Vec::new(),
        parts: vec![
            part(
                "p0",
                TokenRange::new(0, after_hey),
                DialogueAct::Acknowledge,
            ),
            part(
                "p1",
                TokenRange::new(after_hey, after_sum),
                DialogueAct::Inform,
            ),
            {
                let mut consumer =
                    part("p2", TokenRange::new(after_sum, total), DialogueAct::Inform);
                consumer.mentions.push(MentionProposal {
                    key: "x0".to_string(),
                    kind: MentionKind::Result,
                    source_tokens: Vec::new(),
                    inferred: true,
                    resolved: MentionResolutionProposal::PartRef {
                        part: "p1".to_string(),
                        role: PartRefRole::Result,
                    },
                });
                consumer
            },
        ],
        language_writes: Vec::new(),
    };

    let analysis = proposal
        .ground_for(&stream, &BTreeSet::new(), &UtteranceLimits::default())
        .expect("fixture grounds");
    (stream, analysis)
}

fn token_ending_at(stream: &TokenStream, byte: usize) -> usize {
    stream
        .tokens
        .iter()
        .position(|token| token.span.end_byte == byte)
        .map(|index| index + 1)
        .expect("byte offset lands on a token boundary")
}

fn part(id: &str, tokens: TokenRange, act: DialogueAct) -> PartProposal {
    PartProposal {
        id: id.to_string(),
        source_tokens: vec![tokens],
        template: format!("{id} template"),
        act,
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

fn run() -> PartsRun {
    let (_, analysis) = worked_analysis();
    PartsRun::new(analysis, EpisodeId::new()).expect("acyclic")
}

fn id(raw: &str) -> PartId {
    PartId::parse(raw).expect("valid part id")
}

fn procedure(name: &str) -> EvidenceOrigin {
    EvidenceOrigin::Procedure {
        id: name.to_string(),
        version: 1,
    }
}

// ---------------------------------------------------------------------------
// Dispatch order and readiness
// ---------------------------------------------------------------------------

#[test]
fn a_consumer_is_not_ready_until_its_producer_has_run() {
    let mut run = run();

    // p2 consumes p1, so it cannot be first even though p0 and p1 are done in
    // source order.
    assert_eq!(run.next_ready(), Some(&id("p0")));
    run.record(PartOutcome::spoken(id("p0"), "Hey.", TextSpan::new(0, 3)));

    assert_eq!(run.next_ready(), Some(&id("p1")));
    run.record(PartOutcome::executed(
        id("p1"),
        Value::Int(4),
        "2 + 2 is 4.",
        procedure("add"),
    ));

    assert_eq!(run.next_ready(), Some(&id("p2")));
    run.record(PartOutcome::executed(
        id("p2"),
        Value::Int(8),
        "Double that is 8.",
        procedure("double"),
    ));

    assert_eq!(run.next_ready(), None);
    assert!(run.is_complete());
    assert_eq!(run.executed_count(), 3);
}

#[test]
fn a_producer_value_is_available_to_bind_into_its_consumer() {
    let mut run = run();
    run.record(PartOutcome::executed(
        id("p1"),
        Value::Int(4),
        "2 + 2 is 4.",
        procedure("add"),
    ));

    // The analysis is never rewritten to contain 4. The value lives on the
    // outcome and is bound at dispatch time.
    assert_eq!(run.resolved_value(&id("p1")), Some(&Value::Int(4)));
    assert_eq!(run.resolved_value(&id("p2")), None);
}

// ---------------------------------------------------------------------------
// Blocking and independence
// ---------------------------------------------------------------------------

#[test]
fn a_failed_producer_blocks_its_consumer_but_not_an_independent_sibling() {
    let mut run = run();
    run.record(PartOutcome::spoken(id("p0"), "Hey.", TextSpan::new(0, 3)));
    run.record(PartOutcome::abstained(id("p1"), "no procedure was taught"));

    // p2 consumed p1, so it cannot run.
    assert_eq!(run.outcomes[&id("p2")].state, PartState::Blocked);
    // p0 was independent and still completed.
    assert_eq!(run.outcomes[&id("p0")].state, PartState::Executed);
    assert!(run.is_complete());
    assert_eq!(run.executed_count(), 1);
}

#[test]
fn blocking_propagates_transitively() {
    let text = "a b c";
    let stream = tokenize(text).expect("tokenizes");
    // p1 consumes p0, p2 consumes p1.
    let mut parts = vec![
        part("p0", TokenRange::new(0, 1), DialogueAct::Inform),
        part("p1", TokenRange::new(2, 3), DialogueAct::Inform),
        part("p2", TokenRange::new(4, 5), DialogueAct::Inform),
    ];
    for (index, producer) in [(1usize, "p0"), (2, "p1")] {
        parts[index].mentions.push(MentionProposal {
            key: "x0".to_string(),
            kind: MentionKind::Result,
            source_tokens: Vec::new(),
            inferred: true,
            resolved: MentionResolutionProposal::PartRef {
                part: producer.to_string(),
                role: PartRefRole::Result,
            },
        });
    }
    let analysis = UtteranceAnalysisProposal {
        cleaned: text.to_string(),
        alignment: Vec::new(),
        parts,
        language_writes: Vec::new(),
    }
    .ground_for(&stream, &BTreeSet::new(), &UtteranceLimits::default())
    .expect("grounds");

    let mut run = PartsRun::new(analysis, EpisodeId::new()).expect("acyclic");
    run.record(PartOutcome::abstained(id("p0"), "untaught"));

    assert_eq!(run.outcomes[&id("p1")].state, PartState::Blocked);
    assert_eq!(run.outcomes[&id("p2")].state, PartState::Blocked);
}

#[test]
fn budget_exhaustion_abstains_what_is_left_without_discarding_finished_work() {
    let mut run = run();
    run.record(PartOutcome::spoken(id("p0"), "Hey.", TextSpan::new(0, 3)));
    run.record(PartOutcome::executed(
        id("p1"),
        Value::Int(4),
        "2 + 2 is 4.",
        procedure("add"),
    ));

    run.abstain_remaining("the teacher budget is exhausted");

    assert_eq!(run.outcomes[&id("p2")].state, PartState::Abstained);
    // The two completed parts are untouched.
    assert_eq!(run.executed_count(), 2);

    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    let rendered = ResponseRenderer.render(&plan).expect("renders");
    assert_eq!(rendered.text, "Hey. 2 + 2 is 4.");
    assert_eq!(rendered.omitted_claim_ids, vec![claim_id(&id("p2"))]);
}

// ---------------------------------------------------------------------------
// Non-executable parts
// ---------------------------------------------------------------------------

#[test]
fn a_clarify_part_is_seeded_with_its_ambiguity_and_blocks_its_consumer() {
    let (stream, _) = worked_analysis();
    let text = "hey whats 2+2 and then double that";
    let after_hey = token_ending_at(&stream, 3);
    let after_sum = token_ending_at(&stream, text.find(" and").expect("has and"));

    let mut proposal = UtteranceAnalysisProposal {
        cleaned: text.to_string(),
        alignment: Vec::new(),
        parts: vec![
            part(
                "p0",
                TokenRange::new(0, after_hey),
                DialogueAct::Acknowledge,
            ),
            part(
                "p1",
                TokenRange::new(after_hey, after_sum),
                DialogueAct::Inform,
            ),
            part(
                "p2",
                TokenRange::new(after_sum, stream.tokens.len()),
                DialogueAct::Inform,
            ),
        ],
        language_writes: Vec::new(),
    };
    // p1 becomes a clarification, and p2 consumes it.
    proposal.parts[1].mentions.push(MentionProposal {
        key: "e0".to_string(),
        kind: MentionKind::Entity,
        source_tokens: vec![TokenRange::new(after_hey, after_hey + 1)],
        inferred: false,
        resolved: MentionResolutionProposal::Unresolved {
            ambiguity: "which sum did you mean".to_string(),
        },
    });
    proposal.parts[2].mentions.push(MentionProposal {
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
        .ground_for(&stream, &BTreeSet::new(), &UtteranceLimits::default())
        .expect("grounds");
    let mut run = PartsRun::new(analysis, EpisodeId::new()).expect("acyclic");
    run.seed_non_executable();

    assert_eq!(run.outcomes[&id("p1")].state, PartState::Clarified);
    assert_eq!(
        run.outcomes[&id("p1")].claim_text.as_deref(),
        Some("which sum did you mean")
    );
    assert_eq!(run.outcomes[&id("p2")].state, PartState::Blocked);
    assert!(run.needs_clarification());

    // The independent greeting is still available to run.
    assert_eq!(run.next_ready(), Some(&id("p0")));
}

// ---------------------------------------------------------------------------
// Response plan
// ---------------------------------------------------------------------------

#[test]
fn claims_render_in_source_order_not_dispatch_order() {
    let mut run = run();
    // Deliberately record out of source order.
    run.record(PartOutcome::executed(
        id("p1"),
        Value::Int(4),
        "2 + 2 is 4.",
        procedure("add"),
    ));
    run.record(PartOutcome::executed(
        id("p2"),
        Value::Int(8),
        "Double that is 8.",
        procedure("double"),
    ));
    run.record(PartOutcome::spoken(id("p0"), "Hey.", TextSpan::new(0, 3)));

    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    let rendered = ResponseRenderer.render(&plan).expect("renders");
    assert_eq!(rendered.text, "Hey. 2 + 2 is 4. Double that is 8.");
}

#[test]
fn every_claim_carries_evidence_including_the_greeting() {
    let mut run = run();
    run.record(PartOutcome::spoken(id("p0"), "Hey.", TextSpan::new(0, 3)));
    run.record(PartOutcome::executed(
        id("p1"),
        Value::Int(4),
        "2 + 2 is 4.",
        procedure("add"),
    ));
    run.record(PartOutcome::executed(
        id("p2"),
        Value::Int(8),
        "Double that is 8.",
        procedure("double"),
    ));

    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    for claim in &plan.claims {
        let PlannedClaim::Grounded(claim) = claim else {
            panic!("expected every claim grounded");
        };
        assert!(!claim.evidence.is_empty(), "{} has no evidence", claim.id);
    }

    let PlannedClaim::Grounded(greeting) = &plan.claims[0] else {
        panic!("greeting is grounded");
    };
    // The greeting is grounded in the observable fact that the user greeted.
    assert!(
        greeting.evidence[0].id.contains(":utterance:0-3"),
        "{}",
        greeting.evidence[0].id
    );
    assert_eq!(
        greeting.evidence[0].source_kind,
        spoon_core::SourceKind::Observed
    );

    let PlannedClaim::Grounded(answer) = &plan.claims[1] else {
        panic!("answer is grounded");
    };
    assert!(answer.evidence[0].id.contains(":part:p1"));
    assert_eq!(
        answer.evidence[0].source_kind,
        spoon_core::SourceKind::SelfVerified
    );
    assert_eq!(answer.provenance, vec!["procedure:add@1"]);
}

#[test]
fn the_plan_act_reflects_what_the_turn_asks_of_the_user() {
    let mut run = run();
    run.record(PartOutcome::spoken(id("p0"), "Hey.", TextSpan::new(0, 3)));
    run.record(PartOutcome::executed(
        id("p1"),
        Value::Int(4),
        "2 + 2 is 4.",
        procedure("add"),
    ));
    run.record(PartOutcome::clarified(
        id("p2"),
        "Which value should I double?",
        TextSpan::new(0, 3),
    ));

    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    // A pending question dominates two completed answers.
    assert_eq!(plan.dialogue_move.act, DialogueAct::Clarify);
}

#[test]
fn an_outcome_renders_exactly_once_across_turns() {
    let mut run = run();
    run.record(PartOutcome::spoken(id("p0"), "Hey.", TextSpan::new(0, 3)));
    run.record(PartOutcome::clarified(
        id("p1"),
        "Which sum?",
        TextSpan::new(0, 3),
    ));

    let first = run.response_plan("turn-1", ResponseTone::Neutral);
    let first_text = ResponseRenderer.render(&first).expect("renders").text;
    assert!(first_text.contains("Hey."));
    assert!(first_text.contains("Which sum?"));

    // The clarification arrives and p1 now really runs.
    run.outcomes.remove(&id("p1"));
    run.outcomes.remove(&id("p2"));
    run.record(PartOutcome::executed(
        id("p1"),
        Value::Int(4),
        "2 + 2 is 4.",
        procedure("add"),
    ));
    run.record(PartOutcome::executed(
        id("p2"),
        Value::Int(8),
        "Double that is 8.",
        procedure("double"),
    ));

    let second = run.response_plan("turn-2", ResponseTone::Neutral);
    let second_text = ResponseRenderer.render(&second).expect("renders").text;

    // The greeting already rendered in turn one and must not repeat.
    assert!(!second_text.contains("Hey."), "{second_text}");
    assert_eq!(second_text, "2 + 2 is 4. Double that is 8.");
    assert_eq!(
        run.outcomes[&id("p0")].rendered_in_turn.as_deref(),
        Some("turn-1")
    );
    assert_eq!(
        run.outcomes[&id("p1")].rendered_in_turn.as_deref(),
        Some("turn-2")
    );
}

#[test]
fn uncertainty_from_any_part_reaches_the_plan() {
    let mut run = run();
    run.record(PartOutcome::spoken(id("p0"), "Hey.", TextSpan::new(0, 3)));
    let mut qualified =
        PartOutcome::executed(id("p1"), Value::Int(4), "2 + 2 is 4.", procedure("add"));
    qualified.uncertainty = Some(Uncertainty {
        level: UncertaintyLevel::Qualified,
        disclosure: Some("Rounded.".to_string()),
    });
    run.record(qualified);
    run.abstain_remaining("out of budget");

    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    assert_eq!(plan.uncertainty.level, UncertaintyLevel::Qualified);
    assert_eq!(plan.uncertainty.disclosure.as_deref(), Some("Rounded."));
}

#[test]
fn claim_dependencies_carry_part_refs_through_to_the_realizer() {
    let mut run = run();
    run.record(PartOutcome::spoken(id("p0"), "Hey.", TextSpan::new(0, 3)));
    run.record(PartOutcome::executed(
        id("p1"),
        Value::Int(4),
        "2 + 2 is 4.",
        procedure("add"),
    ));
    run.record(PartOutcome::executed(
        id("p2"),
        Value::Int(8),
        "Double that is 8.",
        procedure("double"),
    ));

    let dependencies = run.claim_dependencies();
    assert!(
        dependencies[&claim_id(&id("p2"))].contains(&claim_id(&id("p1"))),
        "{dependencies:?}"
    );
    assert!(!dependencies.contains_key(&claim_id(&id("p0"))));
}

#[test]
fn a_blocked_part_becomes_an_unsupported_claim_rather_than_a_silent_omission() {
    let mut run = run();
    run.record(PartOutcome::spoken(id("p0"), "Hey.", TextSpan::new(0, 3)));
    run.record(PartOutcome::abstained(id("p1"), "untaught"));

    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    let unsupported: Vec<&str> = plan
        .claims
        .iter()
        .filter_map(|claim| match claim {
            PlannedClaim::Unsupported { id, reason } => Some((id.as_str(), reason.as_str())),
            _ => None,
        })
        .map(|(id, _)| id)
        .collect();

    // Both the untaught part and the part it blocked are retained in the plan
    // for audit, and neither renders as a fact.
    assert!(unsupported.contains(&claim_id(&id("p1")).as_str()));
    assert!(unsupported.contains(&claim_id(&id("p2")).as_str()));

    let rendered = ResponseRenderer.render(&plan).expect("renders");
    assert_eq!(rendered.text, "Hey.");
    assert_eq!(rendered.omitted_claim_ids.len(), 2);
}
