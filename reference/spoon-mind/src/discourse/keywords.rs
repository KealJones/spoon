//! Keyword extraction for episode FTS indexing.
//!
//! Collects nouns, proper names, verbs, and adjectives from grounded clauses
//! plus the raw text, lowercased and deduplicated minus stopwords.

use spoon_core::types::*;

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
    "have", "has", "had", "do", "does", "did", "will", "would", "shall",
    "should", "may", "might", "must", "can", "could", "not", "no", "nor",
    "and", "or", "but", "if", "then", "that", "this", "these", "those",
    "it", "its", "of", "to", "in", "on", "at", "by", "for", "with",
    "about", "from", "there", "every", "some", "user", "assistant",
    "does", "who", "what", "which", "where", "when", "how", "many",
];

fn is_stopword(w: &str) -> bool {
    STOPWORDS.contains(&w)
}

/// Extract keywords from clauses and the raw utterance. Returns sorted,
/// deduplicated, lowercase tokens suitable for FTS indexing.
pub fn extract(clauses: &[Clause], raw: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();

    // From raw text: tokenize on non-alphabetic chars (keep hyphens).
    for token in raw.split(|c: char| !c.is_alphabetic() && c != '-') {
        let w = token.trim_matches('-').to_lowercase();
        if w.len() > 1 && !is_stopword(&w) {
            words.push(w);
        }
    }

    // From clauses: nouns, names, verbs, adjectives.
    for clause in clauses {
        for referent in clause
            .referents
            .iter()
            .chain(clause.then_referents.iter())
        {
            if let Some(noun) = &referent.noun {
                let n = noun.to_lowercase();
                if n.len() > 1 && !is_stopword(&n) {
                    words.push(n);
                }
            }
            if let Quant::Named(name) = &referent.quant {
                let n = name.to_lowercase();
                if n.len() > 1 && !is_stopword(&n) {
                    words.push(n);
                }
            }
            for m in &referent.mods {
                let m_lower = m.to_lowercase();
                if m_lower.len() > 1 && !is_stopword(&m_lower) && !m_lower.starts_with("count=") {
                    words.push(m_lower);
                }
            }
        }

        for pred in clause.conditions.iter().chain(clause.then.iter()) {
            let raw_verb = pred.pred.to_lowercase();
            let v = raw_verb.strip_prefix("rel.").unwrap_or(&raw_verb);
            if v.len() > 1 && !is_stopword(v) && v != "be" && v != "is_a" && v != "is" {
                words.push(v.to_string());
            }
            if let Some(attr) = &pred.attr {
                let a = attr.to_lowercase();
                if a.len() > 1 && !is_stopword(&a) {
                    words.push(a);
                }
            }
        }
    }

    words.sort();
    words.dedup();
    words.retain(|w| !w.is_empty());
    words
}
