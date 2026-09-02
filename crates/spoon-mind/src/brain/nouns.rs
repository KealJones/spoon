//! Common nouns learned from context.
//!
//! `names.rs` does this for proper names. A new common noun ("movie", "hero")
//! needs the same treatment for a different reason: the SCE parser guesses it
//! from position, so the sentence parses, but nothing downstream knows the
//! word is vocabulary rather than noise. Teaching it to the gate and the ears
//! makes the next sentence about the same thing a plain parse, and the
//! provisional Concept `discourse` already mints gives it somewhere to live.
//!
//! Persisted under `seed.nouns`, so a restart still knows them.

use std::collections::BTreeSet;

use spoon_core::store::Store;
use spoon_core::types::*;

use spoon_lang::ears::Ears;

use super::Brain;

/// kv key under which learned nouns persist.
pub const NOUNS_KEY: &str = "seed.nouns";

/// Head nouns of the parse that the ears could not place. Modifiers are left
/// out: an unknown word in modifier position is an adjective as far as the
/// grammar cares, and the open class already handles it.
pub fn unknown_nouns_in(clauses: &[Clause], unknown: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for clause in clauses {
        for r in clause.referents.iter().chain(clause.then_referents.iter()) {
            let Some(noun) = &r.noun else { continue };
            let is_unknown = unknown.iter().any(|u| u.eq_ignore_ascii_case(noun));
            if is_unknown && !out.iter().any(|n| n.eq_ignore_ascii_case(noun)) {
                out.push(noun.clone());
            }
        }
    }
    out
}

/// Nouns persisted by earlier sessions.
pub fn load_nouns(store: &Store) -> Vec<String> {
    store
        .kv_get(NOUNS_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

impl Brain {
    /// Teach `nouns` to the ears and the gate and persist them.
    pub(super) fn learn_nouns(&self, nouns: &[String], ears: &mut Ears) -> anyhow::Result<()> {
        if nouns.is_empty() {
            return Ok(());
        }
        let refs: Vec<&str> = nouns.iter().map(String::as_str).collect();
        ears.lexicon_mut().add_nouns(&refs);
        {
            let mut gate = self.gate.lock();
            for n in nouns {
                gate.lex.add_noun(n);
            }
        }

        let store = self.store.lock();
        let mut known: BTreeSet<String> = load_nouns(&store).into_iter().collect();
        known.extend(nouns.iter().cloned());
        store.kv_set(NOUNS_KEY, &serde_json::to_value(&known)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn referent(var: &str, noun: Option<&str>) -> Referent {
        Referent {
            var: var.into(),
            noun: noun.map(str::to_string),
            quant: Quant::Indef,
            mods: vec![],
            owner: None,
            span: None,
        }
    }

    #[test]
    fn only_unplaced_head_nouns_count() {
        let clause = Clause {
            act: Act::Assert,
            referents: vec![referent("x1", Some("hero")), referent("x2", Some("dog")), referent("x3", None)],
            conditions: vec![],
            then: vec![],
            then_referents: vec![],
            sce: "A hero is a dog.".into(),
        };
        let unknown = vec!["hero".to_string(), "fictional".to_string()];
        assert_eq!(unknown_nouns_in(&[clause], &unknown), vec!["hero".to_string()]);
    }
}
