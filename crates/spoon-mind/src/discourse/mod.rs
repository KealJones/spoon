//! Discourse layer: grounds clause referents, stores facts, answers questions,
//! manages rules, detects corrections, and extracts topic keywords.
//!
//! The grounding step turns DRS-style variables into concrete entity values
//! (or well-defined bindings) before the facts and rules layers act on them.
//! Everything here is pure deterministic logic - no LLM calls anywhere.

mod correction;
mod facts;
mod ground;
mod keywords;
mod realize;
mod rules;

pub use correction::{detect as detect_correction, detect_clause as detect_clause_correction, Correction};
pub use facts::{
    assert_grounded, answer_grounded, describe, supersede, Answer, AssertOutcome, FactWriter,
};
pub use ground::ground;
pub use keywords::extract as extract_keywords;
pub use realize::{concept_name, realize_fact, sub_clause_text};
pub use rules::{add_rule, forward_chain, universal_to_rule, Rule};

use std::collections::HashMap;

use spoon_core::can::Can;
use spoon_core::types::*;

/// A resolved discourse entity tracked across turns.
#[derive(Debug, Clone)]
pub struct Entity {
    /// Always Value::Name.
    pub id: Value,
    pub concept: Option<ConceptId>,
    pub noun: Option<String>,
    pub mods: Vec<String>,
    pub last_mentioned: i64,
    pub mentions: u32,
}

/// Running discourse context updated per turn.
#[derive(Debug, Default)]
pub struct DiscourseState {
    pub entities: Vec<Entity>,
    pub turn: i64,
    pub last_clauses: Vec<Clause>,
    pub last_result: Option<Value>,
    /// SCE text of an open question Spoon asked, if any.
    pub last_question: Option<String>,
    pub topic_keywords: Vec<String>,
    /// Running counter for minting new entity names (e.g. dog_1, cat_2).
    pub mint_counter: u32,
}

/// A clause with all discourse referents resolved.
#[derive(Debug, Clone)]
pub struct Grounded {
    pub clause: Clause,
    pub bindings: HashMap<String, Binding>,
    /// Entities created fresh by this grounding (Indef/Count/new-Def minting).
    pub new_entities: Vec<Entity>,
}

/// What a discourse referent resolved to.
#[derive(Debug, Clone)]
pub enum Binding {
    /// Resolved to a named entity (the Value is always Value::Name).
    Entity(Value),
    /// A literal value: number, string, path, etc.
    Literal(Value),
    /// Every/No/AtLeast/AtMost/Exactly - handled as a universal rule.
    Universal { noun: Option<String>, quant: Quant },
    /// Interrogative - the answer slot in a question.
    Query,
    /// Could not be resolved; left unbound.
    Unbound,
}

/// Increment `state.turn` once, then ground each clause in order so later
/// clauses in the batch see entities created by earlier ones.
pub fn ground_all(state: &mut DiscourseState, clauses: &[Clause], can: &Can) -> Vec<Grounded> {
    state.turn += 1;
    clauses.iter().map(|c| ground(state, c, can)).collect()
}
