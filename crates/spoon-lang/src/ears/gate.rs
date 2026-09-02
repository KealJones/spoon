//! Real SCE gate: wraps the Earley parser from `crate::sce`.

use spoon_core::types::clause::Clause;
use spoon_core::Can;

use crate::ears::Gate;
use crate::sce;

/// Gate that delegates to the real SCE parser. Cheap to clone (a few word
/// sets), so callers can snapshot it for an async parse instead of holding a
/// lock across an await.
#[derive(Clone)]
pub struct SceGate {
    pub lex: sce::Lexicon,
}

impl SceGate {
    /// Gate with the default SCE vocabulary.
    pub fn with_defaults() -> Self {
        SceGate { lex: sce::Lexicon::with_defaults() }
    }

    /// Gate populated from a CAN index.
    /// Adds every action verb, concept noun, and entity name from the CAN plus
    /// the reserved names User, Assistant, and Spoon.
    pub fn from_can(can: &Can) -> Self {
        let mut lex = sce::Lexicon::with_defaults();
        for v in can.verbs() {
            lex.add_verb(v);
        }
        for n in can.nouns() {
            lex.add_noun(n);
        }
        for concept in can.concepts() {
            for noun in &concept.nouns {
                if noun.chars().next().is_some_and(|c| c.is_uppercase()) {
                    lex.add_name(noun);
                }
            }
        }
        for name in &["User", "Assistant", "Spoon"] {
            lex.add_name(name);
        }
        SceGate { lex }
    }

    /// Add additional proper names (useful in tests).
    pub fn add_names(&mut self, names: &[&str]) {
        for n in names {
            self.lex.add_name(n);
        }
    }
}

impl Gate for SceGate {
    fn parse(&self, sce: &str) -> Result<Vec<Clause>, String> {
        self.parse_reported(sce).map(|(clauses, _unknowns)| clauses)
    }

    fn parse_reported(&self, sce: &str) -> Result<(Vec<Clause>, Vec<String>), String> {
        sce::parse_text(sce, &self.lex).map_err(|(idx, e)| format!("sentence {idx}: {e}"))
    }
}
