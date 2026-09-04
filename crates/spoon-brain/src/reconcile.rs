//! Stopping the ears from forking the vocabulary.
//!
//! The model names things freshly every time it speaks. Told "greg is friends
//! with keal" it writes `friends<greg, keal>`; told "friendship is symmetric"
//! it writes `symmetric<friendship>`. Both readings are reasonable and together
//! they are useless, because `Symmetric<Friendship>` says nothing about
//! `Friends`. The meta-rule is fine. The names simply disagree.
//!
//! So before a heard reading reaches the interior, every name in it is checked
//! against what Spoon already knows, and a near-miss is resolved to the
//! existing concept rather than minting a rival.
//!
//! The bias is deliberately conservative. Merging two concepts that are not the
//! same is worse than leaving a duplicate: a duplicate is visible and fixable,
//! while a wrong merge silently answers questions about one thing using facts
//! about another. Anything ambiguous is left alone.

use std::collections::HashMap;

use rust_stemmers::{Algorithm, Stemmer};
use spoon_concept::{Concept, ConceptId, Provenance, SymbolId, SymbolTable};
use spoon_store::Store;
use strsim::normalized_levenshtein;

/// Shortest shared opening that counts as evidence. Below this, agreement is
/// coincidence: "add" and "ask" share two letters and mean nothing alike.
const MIN_COMMON_PREFIX: usize = 4;

/// How close two stems must be.
///
/// Sits just under the "friendship" to "friends" distance, which measures
/// exactly 0.6, because that pair is the case this exists for and a threshold
/// resting on the boundary is a coin flip decided by floating point.
const MIN_SIMILARITY: f64 = 0.55;

/// Names shorter than this are never fuzzily matched. In a short word every
/// edit is a large fraction of it, so similarity stops being informative.
const MIN_LENGTH: usize = 5;

/// What reconciliation did, so the caller can record it and a reader can see
/// why an answer used a name they did not type.
#[derive(Debug, Clone, PartialEq)]
pub struct Reconciliation {
    pub steps: Vec<Concept>,
    /// `Synonym<written, canonical>` claims worth storing, so the next
    /// encounter is an exact hit rather than another fuzzy guess.
    pub synonyms: Vec<Concept>,
    /// Names rewritten, as (written, canonical).
    pub rewrites: Vec<(String, String)>,
}

/// Resolve the names in a heard reading against what Spoon already knows.
pub fn reconcile(steps: &[Concept], store: &Store, symbols: &SymbolTable) -> Reconciliation {
    let known = known_names(store);
    let stemmer = Stemmer::create(Algorithm::English);
    let mut decided: HashMap<SymbolId, Option<SymbolId>> = HashMap::new();
    let mut synonyms = Vec::new();
    let mut rewrites = Vec::new();

    for step in steps {
        for node in spoon_concept::pre_order(step) {
            let Some(id) = node.as_symbol() else { continue };
            if decided.contains_key(&id) {
                continue;
            }
            let written = symbols
                .resolve(id)
                .map(|n| n.to_string())
                .unwrap_or_default();
            let resolved = resolve_one(id, &written, &known, store, &stemmer);
            if let Some(target) = resolved
                && target != id
            {
                let canonical = known
                    .get(&target)
                    .cloned()
                    .unwrap_or_else(|| symbols.display(target));
                synonyms.push(Concept::call(
                    "synonym",
                    [Concept::text(&written), Concept::symbol(target)],
                ));
                rewrites.push((written.clone(), canonical));
            }
            decided.insert(id, resolved);
        }
    }

    let rewritten = steps.iter().map(|s| rewrite(s, &decided)).collect();
    Reconciliation {
        steps: rewritten,
        synonyms,
        rewrites,
    }
}

/// Store whatever reconciliation learned, so the same near-miss is an exact hit
/// next time rather than another fuzzy guess.
pub fn remember(reconciliation: &Reconciliation, store: &Store) {
    for claim in &reconciliation.synonyms {
        let _ = store.assert_concept(claim, Provenance::Inferred, None, None);
    }
}

fn resolve_one(
    id: SymbolId,
    written: &str,
    known: &HashMap<SymbolId, String>,
    store: &Store,
    stemmer: &Stemmer,
) -> Option<SymbolId> {
    // Already a concept Spoon knows. Nothing to reconcile.
    if known.contains_key(&id) {
        return None;
    }
    // An explicit synonym beats any amount of spelling similarity, because
    // somebody said so and nobody guessed.
    if let Some(target) = stored_synonym(written, store) {
        return Some(target);
    }
    if written.chars().count() < MIN_LENGTH {
        return None;
    }

    let stem = stemmer.stem(written).to_string();
    let mut best: Option<(SymbolId, f64)> = None;
    let mut contested = false;

    for (candidate_id, candidate_name) in known {
        if candidate_name.chars().count() < MIN_LENGTH {
            continue;
        }
        if common_prefix(written, candidate_name) < MIN_COMMON_PREFIX {
            continue;
        }
        let candidate_stem = stemmer.stem(candidate_name).to_string();
        let score = normalized_levenshtein(&stem, &candidate_stem);
        if score < MIN_SIMILARITY {
            continue;
        }
        match best {
            Some((_, previous)) if previous >= score => {
                // A second candidate scoring as well as the first means the
                // evidence does not single one out.
                if (previous - score).abs() < f64::EPSILON {
                    contested = true;
                }
            }
            _ => best = Some((*candidate_id, score)),
        }
    }

    if contested {
        None
    } else {
        best.map(|(id, _)| id)
    }
}

/// A `Synonym<"word", concept>` already asserted about this spelling.
fn stored_synonym(written: &str, store: &Store) -> Option<SymbolId> {
    let head = SymbolId::of("synonym");
    let claims = store.concepts_by_head(head, 512).ok()?;
    for claim in claims {
        let matches_word = claim
            .arg(0)
            .and_then(|a| a.as_ground())
            .and_then(|g| g.as_str())
            .is_some_and(|w| w.eq_ignore_ascii_case(written));
        if matches_word
            && let Some(target) = claim.arg(1).and_then(|a| a.as_symbol())
            && store.holds(&claim).unwrap_or(false)
        {
            return Some(target);
        }
    }
    None
}

/// Concepts Spoon has actually established, which is what a rewrite may aim at.
///
/// Only what the store has committed to counts. Deliberately NOT the session
/// symbol table: that holds every spelling the ears have uttered this run,
/// including the one being reconciled right now, so consulting it would make
/// every freshly invented name count as already known and reconcile nothing.
/// That is precisely the bug this pass exists to fix.
fn known_names(store: &Store) -> HashMap<SymbolId, String> {
    store
        .all_symbols()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(x, y)| x.eq_ignore_ascii_case(y))
        .count()
}

fn rewrite(concept: &Concept, decided: &HashMap<SymbolId, Option<SymbolId>>) -> Concept {
    match concept {
        Concept::Atomic(ConceptId::Named(id)) => match decided.get(id).copied().flatten() {
            Some(target) => Concept::symbol(target),
            None => concept.clone(),
        },
        Concept::Compound { head, args } => {
            let new_head = rewrite(head, decided);
            let new_args: Vec<Concept> = args.iter().map(|a| rewrite(a, decided)).collect();
            Concept::apply(new_head, new_args)
        }
        _ => concept.clone(),
    }
}
