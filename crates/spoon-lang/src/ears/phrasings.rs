//! Phrasing store: slotted utterance templates with BM25 retrieval.

use std::collections::HashMap;
use std::path::Path;

use crate::ears::values::{SlotKind, Spotted};

// ---- phrasing types ----

#[derive(Debug, Clone)]
pub enum Tok {
    Word(String),
    Slot { name: String, kind: SlotKind },
}

#[derive(Debug, Clone)]
pub struct Phrasing {
    /// Pattern tokens (words + typed slots).
    pub pattern: Vec<Tok>,
    /// SCE template string, with {slot_name} placeholders.
    pub sce: String,
    /// Origin: "seed", "learned", "induced".
    pub source: String,
    pub credit: i8,
}

// ---- BM25 index ----

struct Bm25 {
    k1: f32,
    b: f32,
    avgdl: f32,
    /// term -> document frequency
    df: HashMap<String, u32>,
    /// per-document term lists
    docs: Vec<Vec<String>>,
    total_docs: usize,
}

impl Bm25 {
    fn build(docs: Vec<Vec<String>>) -> Bm25 {
        let total_docs = docs.len();
        let avgdl = if total_docs == 0 {
            1.0
        } else {
            docs.iter().map(|d| d.len() as f32).sum::<f32>() / total_docs as f32
        };
        let mut df: HashMap<String, u32> = HashMap::new();
        for doc in &docs {
            let unique: std::collections::HashSet<&String> = doc.iter().collect();
            for term in unique {
                *df.entry(term.clone()).or_default() += 1;
            }
        }
        Bm25 { k1: 1.5, b: 0.75, avgdl, df, docs, total_docs }
    }

    fn score(&self, query: &[String], doc_idx: usize) -> f32 {
        if self.total_docs == 0 {
            return 0.0;
        }
        let doc = &self.docs[doc_idx];
        let dl = doc.len() as f32;
        let mut score = 0.0f32;
        for qt in query {
            let df = self.df.get(qt).copied().unwrap_or(0) as f32;
            if df == 0.0 {
                continue;
            }
            let idf = ((self.total_docs as f32 - df + 0.5) / (df + 0.5) + 1.0).ln();
            let tf = doc.iter().filter(|t| *t == qt).count() as f32;
            let tf_norm = tf * (self.k1 + 1.0)
                / (tf + self.k1 * (1.0 - self.b + self.b * dl / self.avgdl));
            score += idf * tf_norm;
        }
        score
    }

    fn retrieve(&self, query: &[String], k: usize) -> Vec<(usize, f32)> {
        let mut scores: Vec<(usize, f32)> = (0..self.docs.len())
            .map(|i| (i, self.score(query, i)))
            .filter(|(_, s)| *s > 0.0)
            .collect();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(k);
        scores
    }
}

// ---- PhrasingStore ----

pub struct PhrasingStore {
    phrasings: Vec<Phrasing>,
    /// pre-tokenized pattern words for BM25
    bm25: Bm25,
}

impl PhrasingStore {
    pub fn new() -> Self {
        PhrasingStore {
            phrasings: vec![],
            bm25: Bm25::build(vec![]),
        }
    }

    /// Load from `data/seed/dialog_phrasings.json`.
    /// `normalize_fn` converts the raw utterance text to a normalized token sequence.
    pub fn load<F>(path: &Path, normalize_fn: F) -> anyhow::Result<Self>
    where
        F: Fn(&str) -> Vec<String>,
    {
        #[derive(serde::Deserialize)]
        struct Entry {
            utterance: String,
            sce: String,
        }

        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        let entries: Vec<Entry> = serde_json::from_str(&text)?;

        let mut store = PhrasingStore::new();
        for e in entries {
            let tokens = normalize_fn(&e.utterance);
            if tokens.is_empty() { continue; }
            let pattern: Vec<Tok> = tokens.into_iter().map(|t| Tok::Word(t)).collect();
            store.phrasings.push(Phrasing {
                pattern,
                sce: e.sce,
                source: "seed".to_string(),
                credit: 1,
            });
        }
        store.rebuild_index();
        Ok(store)
    }

    /// Add a phrasing to the store. Phrasings with empty patterns are skipped.
    pub fn add(&mut self, phrasing: Phrasing) {
        if phrasing.pattern.is_empty() {
            return; // empty patterns match everything - useless noise
        }
        self.phrasings.push(phrasing);
        self.rebuild_index();
    }

    /// Add many phrasings at once and rebuild the index once. Empty patterns are skipped.
    pub fn add_many(&mut self, phrasings: Vec<Phrasing>) {
        let non_empty: Vec<_> = phrasings.into_iter().filter(|p| !p.pattern.is_empty()).collect();
        if non_empty.is_empty() { return; }
        self.phrasings.extend(non_empty);
        self.rebuild_index();
    }

    pub fn rebuild_index(&mut self) {
        let docs: Vec<Vec<String>> = self
            .phrasings
            .iter()
            .map(|p| pattern_words(&p.pattern))
            .collect();
        self.bm25 = Bm25::build(docs);
    }

    pub fn len(&self) -> usize {
        self.phrasings.len()
    }

    /// Exact token match: `query_tokens` (after normalization) vs stored pattern words.
    /// Returns the SCE string and the slot bindings if matched.
    pub fn exact_match(
        &self,
        query_tokens: &[String],
        spotted: &[Spotted],
    ) -> Option<String> {
        for p in &self.phrasings {
            if let Some(sce) = try_align(query_tokens, spotted, p) {
                return Some(sce);
            }
        }
        None
    }

    /// BM25 retrieval: return top-k (phrasing_index, score) pairs.
    pub fn retrieve(&self, query_tokens: &[String], k: usize) -> Vec<(usize, f32)> {
        self.bm25.retrieve(query_tokens, k)
    }

    /// Retrieve and try slot-alignment on the top-k candidates.
    /// Returns the first candidate that aligns successfully: (sce_string, score).
    pub fn retrieve_and_align(
        &self,
        query_tokens: &[String],
        spotted: &[Spotted],
        k: usize,
        threshold: f32,
    ) -> Option<(String, f32)> {
        for (idx, score) in self.retrieve(query_tokens, k) {
            if score < threshold {
                break;
            }
            let p = &self.phrasings[idx];
            // Try alignment (slots consume spotted values)
            if let Some(sce) = try_align(query_tokens, spotted, p) {
                return Some((sce, score));
            }
        }
        None
    }
}

impl Default for PhrasingStore {
    fn default() -> Self {
        PhrasingStore::new()
    }
}

// ---- alignment logic ----

/// Try to align `query_tokens` against `pattern` filling slots with `spotted` values.
/// Returns the filled SCE string if alignment succeeds.
fn try_align(query_tokens: &[String], spotted: &[Spotted], p: &Phrasing) -> Option<String> {
    let mut bindings: HashMap<String, String> = HashMap::new();

    // Build a flat word list from the pattern, tracking slot positions
    // We do token-by-token alignment.
    let mut qi = 0; // index into query_tokens
    let mut pi = 0; // index into pattern tokens

    while pi < p.pattern.len() && qi < query_tokens.len() {
        match &p.pattern[pi] {
            Tok::Word(w) => {
                // Must match the query token exactly
                if w != &query_tokens[qi] {
                    return None;
                }
                qi += 1;
                pi += 1;
            }
            Tok::Slot { name, kind } => {
                // Consume one query token that matches the slot kind
                let qt = &query_tokens[qi];
                // Check that this token was spotted as the right kind
                let is_match = spotted.iter().any(|s| {
                    s.text == *qt
                        && (s.kind == *kind
                            || (*kind == SlotKind::Number
                                && matches!(
                                    s.kind,
                                    SlotKind::Number | SlotKind::Arith
                                ))
                            || (*kind == SlotKind::Arith
                                && matches!(s.kind, SlotKind::Number | SlotKind::Arith)))
                });
                if !is_match {
                    // Try consuming multiple tokens for Arith / Name
                    if matches!(kind, SlotKind::Arith | SlotKind::Name | SlotKind::Number) {
                        // look ahead for a spotted span that covers the next N tokens
                        if let Some(arith_text) = find_spotted_span_starting(spotted, query_tokens, qi, kind) {
                            let n_tokens = arith_text.split_whitespace().count();
                            bindings.insert(name.clone(), arith_text);
                            qi += n_tokens;
                            pi += 1;
                            continue;
                        }
                    }
                    return None;
                }
                bindings.insert(name.clone(), qt.clone());
                qi += 1;
                pi += 1;
            }
        }
    }

    // Must consume entire pattern AND entire query for a valid match.
    if pi != p.pattern.len() || qi != query_tokens.len() {
        return None;
    }

    // Fill the SCE template
    Some(fill_template(&p.sce, &bindings))
}

/// Find a spotted span of the given kind whose text starts at `query_tokens[qi]`.
fn find_spotted_span_starting(
    spotted: &[Spotted],
    query_tokens: &[String],
    qi: usize,
    kind: &SlotKind,
) -> Option<String> {
    for s in spotted {
        if !matches_kind(s, kind) {
            continue;
        }
        // Check if the spotted text matches a sequence of query tokens starting at qi
        let s_tokens: Vec<&str> = s.text.split_whitespace().collect();
        let n = s_tokens.len();
        if qi + n > query_tokens.len() {
            continue;
        }
        let qt_slice = &query_tokens[qi..qi + n];
        let qt_joined = qt_slice.join(" ");
        if qt_joined == s.text || qt_joined.replace(' ', "") == s.text.replace(' ', "") {
            return Some(s.text.clone());
        }
    }
    None
}

fn matches_kind(s: &Spotted, kind: &SlotKind) -> bool {
    s.kind == *kind
        || (*kind == SlotKind::Number && matches!(s.kind, SlotKind::Number | SlotKind::Arith))
        || (*kind == SlotKind::Arith && matches!(s.kind, SlotKind::Number | SlotKind::Arith))
}

/// Substitute {name} placeholders in the SCE template with bound values.
pub fn fill_template(sce: &str, bindings: &HashMap<String, String>) -> String {
    let mut result = sce.to_string();
    for (k, v) in bindings {
        result = result.replace(&format!("{{{}}}", k), v);
    }
    result
}

fn pattern_words(pattern: &[Tok]) -> Vec<String> {
    pattern
        .iter()
        .filter_map(|t| match t {
            Tok::Word(w) => Some(w.clone()),
            Tok::Slot { .. } => None,
        })
        .collect()
}

/// Tokenize text for BM25 comparison (split on whitespace, strip punctuation).
pub fn tokenize_for_bm25(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':'))
        .map(|t| t.trim_matches(|c: char| matches!(c, '.' | '?' | '!' | '\'' | '"')).to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}
