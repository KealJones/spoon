//! Proper names learned from context.
//!
//! `Sandra moves to the garden.` parses as SCE, but the ears reject a pristine
//! sentence whose proper name they have never met, and the normalized
//! fallback lowercases the name into an unknown word. When that is the only
//! thing wrong with a sentence, the brain teaches the name to the ears and the
//! gate, persists it under `seed.names` (so it survives a restart and rides
//! along in `spoon export`), and hears the sentence again.

use std::collections::BTreeSet;

use spoon_core::store::Store;
use spoon_core::types::*;

use spoon_lang::ears::lexicon::Lexicon;
use spoon_lang::ears::{Ears, Gate};

use super::Brain;

/// kv key under which learned names persist.
pub const NAMES_KEY: &str = "seed.names";

/// The proper names that explain a failed hearing: capitalized words of a
/// pristine sentence that the SCE parser reads as `Quant::Named` referents
/// used as predicate arguments, and that the ears lexicon has never seen.
/// A sentence the parser rejects teaches nothing. Unknown common nouns
/// ("the ball") are not held against it: the ears' last resort already
/// accepts a parse that places every word but tolerates unknown nouns.
pub fn unknown_names_in(text: &str, gate: &dyn Gate, lex: &Lexicon) -> Vec<String> {
    let text = text.trim();
    if !text.starts_with(|c: char| c.is_uppercase()) || !text.ends_with(['.', '?', '!']) {
        return vec![];
    }
    let Ok((clauses, _)) = gate.parse_reported(text) else {
        return vec![];
    };
    let mut names: Vec<String> = vec![];
    for clause in &clauses {
        let used: BTreeSet<&str> = clause
            .conditions
            .iter()
            .chain(clause.then.iter())
            .flat_map(|p| p.args.iter().chain(p.adjuncts.iter().map(|(_, t)| t)))
            .filter_map(|t| match t {
                Term::Var { var } => Some(var.as_str()),
                _ => None,
            })
            .collect();
        for r in clause.referents.iter().chain(clause.then_referents.iter()) {
            let Quant::Named(name) = &r.quant else { continue };
            let unknown = !lex.is_name(name) && lex.canonical_name(name).is_none();
            if unknown && used.contains(r.var.as_str()) && looks_like_a_name(name) && !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    names
}

/// Capitalized, alphabetic, and not SCE syntax (variables like `X1`, minted
/// names like `Object-X`, the reserved names).
fn looks_like_a_name(word: &str) -> bool {
    let mut chars = word.chars();
    let first_upper = chars.next().is_some_and(|c| c.is_uppercase());
    let rest: Vec<char> = chars.collect();
    first_upper
        && !rest.is_empty()
        && rest.iter().all(|c| c.is_alphabetic())
        && !matches!(word, "User" | "Assistant" | "Spoon" | "It")
}

/// Names persisted by earlier sessions.
pub fn load_names(store: &Store) -> Vec<String> {
    store
        .kv_get(NAMES_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

impl Brain {
    /// Teach `names` to the ears and the gate and persist them.
    pub(super) fn learn_names(&self, names: &[String], ears: &mut Ears) -> anyhow::Result<()> {
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        ears.lexicon_mut().add_names(&refs);
        self.gate.lock().add_names(&refs);

        let store = self.store.lock();
        let mut known: BTreeSet<String> = load_names(&store).into_iter().collect();
        known.extend(names.iter().cloned());
        store.kv_set(NAMES_KEY, &serde_json::to_value(&known)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spoon_lang::ears::gate::SceGate;

    #[test]
    fn names_are_learned_only_when_they_explain_the_failure() {
        let gate = SceGate::with_defaults();
        let mut lex = Lexicon::new();
        lex.add_names(&["Mary"]);
        let names = |s: &str| unknown_names_in(s, &gate, &lex);
        assert_eq!(names("Sandra moves to the garden."), vec!["Sandra"]);
        assert_eq!(names("Sandra gives the apple to Maya."), vec!["Sandra", "Maya"]);
        // Known names are not relearned; an unknown noun does not block the name.
        assert_eq!(names("Sandra gives the ball to Mary."), vec!["Sandra"]);
        // Lowercase start, no terminator, no parse, or nothing new: nothing to learn.
        assert!(names("sandra moves to the garden.").is_empty());
        assert!(names("Whats the double of 100").is_empty());
        assert!(names("Sandra the garden.").is_empty());
        assert!(names("Mary moves to the garden.").is_empty());
    }
}
