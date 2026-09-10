//! Turning what the ears heard into something the interior can act on.
//!
//! The ears produce a flat sequence, because that is the shape speech has:
//! bind, correct, qualify, qualify. Deciding what the sequence *means* needs
//! context the ears do not have, so it happens here, in ordinary code, with the
//! store to hand.

use spoon_concept::{Concept, SymbolId};

/// What Spoon was asked to do with one concept.
#[derive(Debug, Clone, PartialEq)]
pub enum Move {
    /// Take this as true and remember it.
    Assert(Concept),
    /// Answer whether this holds, or fill its holes.
    Ask(Concept),
    /// Reduce this and report the result.
    Do(Concept),
    /// Say something back without touching the store.
    Chat(Concept),
}

impl Move {
    pub fn concept(&self) -> &Concept {
        match self {
            Move::Assert(c) | Move::Ask(c) | Move::Do(c) | Move::Chat(c) => c,
        }
    }
}

/// Resolve a heard sequence into the moves the interior should make.
///
/// Corrections are applied here rather than left for later. A user who says
/// "the weights, no wait the scores" has not asked two questions, and treating
/// the retraction as a separate turn would answer the wrong one first.
/// Words that put a whole sentence in the interrogative.
///
/// English marks a yes-or-no question by moving the verb to the front, which
/// is a syntactic fact and needs no model to see.
const INTERROGATIVE: &[&str] = &[
    "is", "are", "was", "were", "am", "do", "does", "did", "can", "could", "will", "would",
    "should", "has", "have", "had", "who", "what", "which", "where", "when", "why", "how", "whats",
    "what's", "whos", "who's",
];

/// Is this utterance asking rather than telling?
///
/// The ears read "is carol friends with frank" as a statement and Spoon
/// replied "noted", writing into the store a fact the speaker had asked about.
/// That is the expensive direction to get wrong: a wrong answer is visible and
/// a wrong assertion is not, and it poisons every later question.
pub fn is_question(text: &str) -> bool {
    let lowered = text.trim().to_lowercase();
    if lowered.ends_with('?') {
        return true;
    }
    let first = lowered
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '\'');
    INTERROGATIVE.contains(&first)
}

pub fn resolve(steps: &[Concept], question: bool) -> Vec<Move> {
    let assert = SymbolId::of("assert-that");
    let ask = SymbolId::of("ask");
    let do_ = SymbolId::of("do");
    let chat = SymbolId::of("chat");
    let correction = SymbolId::of("correction");

    let mut moves: Vec<Move> = Vec::new();
    for step in steps {
        let head = step.head_symbol();
        // A correction replaces the most recent move rather than adding one,
        // because the speaker is repairing what they just said, not adding to
        // it.
        if head == Some(correction) {
            let replacement = step.arg(0).cloned().unwrap_or_else(|| step.clone());
            moves.pop();
            moves.extend(resolve(std::slice::from_ref(&replacement), question));
            continue;
        }
        let inner = step.arg(0).cloned();
        let m = match (head, inner) {
            // The sentence was a question, so it is not a claim no matter what
            // the ears wrapped it in. Asking is safe either way: a goal that
            // derivation cannot reach still gets evaluated, so "can you
            // reverse banana" reaches the same place it did as a `do`.
            (Some(h), Some(c)) if question && (h == assert || h == do_) => Move::Ask(c),
            (Some(h), Some(c)) if h == assert => Move::Assert(c),
            (Some(h), Some(c)) if h == ask => Move::Ask(c),
            (Some(h), Some(c)) if h == do_ => Move::Do(c),
            (Some(h), Some(c)) if h == chat => Move::Chat(c),
            // An utterance the ears did not wrap is treated as something to
            // reduce. Guessing "store-assert" instead would let a misread question
            // silently write to the store, which is the more expensive mistake.
            _ if question => Move::Ask(step.clone()),
            _ => Move::Do(step.clone()),
        };
        moves.push(m);
    }
    moves
}
