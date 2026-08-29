//! The real-model gate.
//!
//! The fixture in `tests/fixtures/` is verbatim output from a local Ollama run
//! of `qwen3.8:27b` against the worked example, captured with temperature 0.
//! Checking it in means the schema is proven against something a real model
//! actually emitted rather than against a hand-authored proposal that happens
//! to satisfy the validator.
//!
//! What this does NOT prove is that a small model can do it. See
//! `tasks/front-language-progress.md` for the `qwen2.5:1.5b` result, which
//! failed outright. The spec's weaning argument assumes a small front model, so
//! that gap is real and recorded rather than papered over.

use std::collections::BTreeSet;

use spoon_core::Value;
use spoon_core::language::{DialogueAct, IntentDisposition, tokenize};
use spoon_core::utterance::{
    MentionResolution, PartId, PartRefRole, UtteranceAnalysisProposal, UtteranceLimits,
};

const UTTERANCE: &str = "hey whats 2+2 and then double that";
const FIXTURE: &str = include_str!("fixtures/utterance-qwen3.8-27b.json");

#[test]
fn real_model_output_deserializes_into_the_proposal_type() {
    // deny_unknown_fields is on, so this also proves the model emitted no
    // field the trusted boundary does not know about.
    let _: UtteranceAnalysisProposal =
        serde_json::from_str(FIXTURE).expect("real model output matches the proposal schema");
}

#[test]
fn real_model_output_grounds_into_a_valid_analysis() {
    let stream = tokenize(UTTERANCE).expect("tokenizes");
    let proposal: UtteranceAnalysisProposal = serde_json::from_str(FIXTURE).expect("deserializes");

    let analysis = proposal
        .ground_for(&stream, &BTreeSet::new(), &UtteranceLimits::default())
        .expect("real model output passes structural validation");

    assert_eq!(analysis.parts.len(), 3);
    // The greeting, the sum, and the part that consumes the sum.
    assert_eq!(analysis.parts[0].act, DialogueAct::Acknowledge);
    assert_eq!(analysis.parts[1].act, DialogueAct::Ask);
    for part in &analysis.parts {
        assert_eq!(part.intent.disposition, IntentDisposition::Execute);
    }
}

#[test]
fn the_model_bound_the_uncomputed_value_instead_of_guessing_it() {
    let stream = tokenize(UTTERANCE).expect("tokenizes");
    let analysis = serde_json::from_str::<UtteranceAnalysisProposal>(FIXTURE)
        .expect("deserializes")
        .ground_for(&stream, &BTreeSet::new(), &UtteranceLimits::default())
        .expect("grounds");

    let consumer = analysis
        .part(&PartId::parse("p2").unwrap())
        .expect("p2 exists");
    let reference = consumer
        .mentions
        .iter()
        .find_map(|mention| match &mention.resolved {
            MentionResolution::PartRef { part, role } => Some((part.clone(), *role)),
            _ => None,
        })
        .expect("p2 references another part's result");

    assert_eq!(reference.0, PartId::parse("p1").unwrap());
    assert_eq!(reference.1, PartRefRole::Result);

    // The critical property: the model did not compute 4 and inline it. The
    // cleaned text still carries no answer.
    assert!(!analysis.cleaned.text.contains('4'));

    // And the dependency the Engine derives puts the producer first.
    let order = analysis.dispatch_order().expect("acyclic");
    let producer = order
        .iter()
        .position(|id| id == &PartId::parse("p1").unwrap())
        .unwrap();
    let consumer_position = order
        .iter()
        .position(|id| id == &PartId::parse("p2").unwrap())
        .unwrap();
    assert!(producer < consumer_position);
}

#[test]
fn the_model_grounded_the_literals_it_did_supply() {
    let stream = tokenize(UTTERANCE).expect("tokenizes");
    let analysis = serde_json::from_str::<UtteranceAnalysisProposal>(FIXTURE)
        .expect("deserializes")
        .ground_for(&stream, &BTreeSet::new(), &UtteranceLimits::default())
        .expect("grounds");

    let sum = analysis
        .part(&PartId::parse("p1").unwrap())
        .expect("p1 exists");
    let literals: Vec<&Value> = sum
        .mentions
        .iter()
        .filter_map(|mention| match &mention.resolved {
            MentionResolution::Literal { value } => Some(value),
            _ => None,
        })
        .collect();

    assert_eq!(literals, vec![&Value::Int(2), &Value::Int(2)]);
    // Every literal points at real source text rather than being asserted.
    for mention in &sum.mentions {
        if matches!(mention.resolved, MentionResolution::Literal { .. }) {
            assert!(!mention.inferred);
            let span = mention.surface.first().expect("a literal carries a span");
            assert_eq!(stream.slice(span), Some("2"));
        }
    }
}

#[test]
fn the_model_covered_the_whole_utterance() {
    // Coverage is enforced by ground_for, so reaching this point already
    // proves it. This asserts the specific thing that went wrong on the first
    // prompt revision: the connectives "and" and "then" were left orphaned.
    let stream = tokenize(UTTERANCE).expect("tokenizes");
    let analysis = serde_json::from_str::<UtteranceAnalysisProposal>(FIXTURE)
        .expect("deserializes")
        .ground_for(&stream, &BTreeSet::new(), &UtteranceLimits::default())
        .expect("grounds");

    let covered: usize = analysis
        .parts
        .iter()
        .flat_map(|part| part.spans.iter())
        .map(|span| span.end_byte - span.start_byte)
        .sum();
    let non_whitespace: usize = stream
        .tokens
        .iter()
        .filter(|token| token.kind != spoon_core::language::TokenKind::Whitespace)
        .map(|token| token.span.end_byte - token.span.start_byte)
        .sum();

    // Parts are contiguous ranges, so they also swallow the interior spaces.
    assert!(covered >= non_whitespace, "{covered} < {non_whitespace}");
}
