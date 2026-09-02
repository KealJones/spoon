//! Lexicon: known words, names, slang, typo-repair candidates.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use spoon_core::Can;

// ---- seed JSON shapes ----

#[derive(Deserialize)]
struct SeedNoun {
    lemma: String,
    plural: Option<String>,
}

#[derive(Deserialize)]
struct SeedVerb {
    lemma: String,
    third: Option<String>,
    past: Option<String>,
}

#[derive(Deserialize)]
struct SeedAdj {
    lemma: String,
}

#[derive(Deserialize)]
struct LexiconSeed {
    nouns: Vec<SeedNoun>,
    verbs: Vec<SeedVerb>,
    adjectives: Vec<SeedAdj>,
    names: Vec<String>,
    function_words: Vec<String>,
}

#[derive(Deserialize)]
struct SlangSeed {
    replacements: HashMap<String, String>,
    fillers: Vec<String>,
    number_words: HashMap<String, serde_json::Value>,
}

// ---- internal word entry ----

#[derive(Clone)]
struct WordEntry {
    freq: u32,
    kind: WordKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WordKind {
    Noun,
    Verb,
    Adjective,
    Function,
    Name,
}

// ---- public Lexicon ----

pub struct Lexicon {
    /// canonical lowercase -> entry
    words: HashMap<String, WordEntry>,
    /// proper names in their canonical capitalized form
    names: HashSet<String>,
    /// slang/contraction -> expansion, sorted by key length descending for application
    pub replacements: Vec<(String, String)>,
    /// filler phrases to strip at sentence starts
    pub fillers: Vec<String>,
    /// number words "one" -> 1
    pub number_words: HashMap<String, f64>,
}

impl Lexicon {
    pub fn new() -> Self {
        Lexicon {
            words: HashMap::new(),
            names: HashSet::new(),
            replacements: vec![],
            fillers: vec![],
            number_words: HashMap::new(),
        }
    }

    /// Load from `data/seed/` directory (lexicon.json + slang.json + common_words.txt).
    pub fn load_seed_dir(path: &Path) -> anyhow::Result<Self> {
        let mut lex = Lexicon::new();

        let lex_path = path.join("lexicon.json");
        let text = std::fs::read_to_string(&lex_path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", lex_path.display()))?;
        let seed: LexiconSeed = serde_json::from_str(&text)?;

        for n in &seed.nouns {
            lex.insert_word(&n.lemma, WordKind::Noun, 1);
            if let Some(p) = &n.plural {
                lex.insert_word(p, WordKind::Noun, 1);
            }
        }
        for v in &seed.verbs {
            lex.insert_word(&v.lemma, WordKind::Verb, 1);
            if let Some(t) = &v.third {
                lex.insert_word(t, WordKind::Verb, 1);
            }
            if let Some(p) = &v.past {
                lex.insert_word(p, WordKind::Verb, 1);
            }
        }
        for a in &seed.adjectives {
            lex.insert_word(&a.lemma, WordKind::Adjective, 1);
        }
        for fw in &seed.function_words {
            lex.insert_word(fw, WordKind::Function, 1);
        }
        for name in &seed.names {
            lex.names.insert(name.clone());
            lex.insert_word(&name.to_lowercase(), WordKind::Name, 1);
        }
        // Also register reserved SCE names
        for name in &["User", "Assistant", "Spoon"] {
            lex.names.insert(name.to_string());
            lex.insert_word(&name.to_lowercase(), WordKind::Name, 5);
        }

        // Load common words (freq=3) - these are known and protected from repair
        let common_path = path.join("common_words.txt");
        if let Ok(text) = std::fs::read_to_string(&common_path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                for word in line.split_whitespace() {
                    let w = word.trim_matches(|c: char| matches!(c, '"' | '\''));
                    if !w.is_empty() {
                        lex.insert_word(w, WordKind::Function, 3);
                    }
                }
            }
        }

        let slang_path = path.join("slang.json");
        let slang_text = std::fs::read_to_string(&slang_path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", slang_path.display()))?;
        let slang: SlangSeed = serde_json::from_str(&slang_text)?;

        let mut reps: Vec<(String, String)> = slang.replacements.into_iter().collect();
        // sort by key length descending so multi-word replacements apply before single-word
        reps.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        lex.replacements = reps;
        lex.fillers = slang.fillers;
        // Also register all filler words/phrases as known so they don't show up as unknowns
        for filler in &lex.fillers {
            for w in filler.split_whitespace() {
                lex.words.entry(w.to_lowercase()).or_insert(WordEntry { freq: 1, kind: WordKind::Function });
            }
        }

        for (word, val) in &slang.number_words {
            let n = match val {
                serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
                _ => continue,
            };
            lex.number_words.insert(word.clone(), n);
            // Also register the word itself as known
            lex.insert_word(word, WordKind::Function, 1);
        }

        Ok(lex)
    }

    fn insert_word(&mut self, word: &str, kind: WordKind, freq: u32) {
        let key = word.to_lowercase();
        let entry = self.words.entry(key).or_insert(WordEntry { freq: 0, kind });
        entry.freq += freq;
    }

    /// Add verbs and nouns from the CAN index.
    pub fn extend_from_can(&mut self, can: &Can) {
        for v in can.verbs() {
            self.insert_word(v, WordKind::Verb, 5);
        }
        for n in can.nouns() {
            self.insert_word(n, WordKind::Noun, 5);
        }
        for concept in can.concepts() {
            for noun in &concept.nouns {
                if noun.chars().next().is_some_and(|c| c.is_uppercase()) {
                    self.names.insert(noun.clone());
                    self.insert_word(&noun.to_lowercase(), WordKind::Name, 5);
                }
            }
        }
    }

    /// Register a list of proper names (for tests and custom entity lists).
    pub fn add_names(&mut self, names: &[&str]) {
        for n in names {
            self.names.insert(n.to_string());
            self.insert_word(&n.to_lowercase(), WordKind::Name, 10);
        }
    }

    /// Learn a user-taught word (canonical form). Freq = 10 overrides seed defaults.
    pub fn learn_word(&mut self, word: &str, canonical: &str) {
        self.insert_word(canonical, WordKind::Noun, 10);
        if word.to_lowercase() != canonical.to_lowercase() {
            // register the variant as pointing at canonical
            let key = word.to_lowercase();
            self.words.insert(key, WordEntry { freq: 10, kind: WordKind::Noun });
        }
    }

    pub fn is_known(&self, word: &str) -> bool {
        let lower = word.to_lowercase();
        self.words.contains_key(&lower) || self.is_name(word)
    }

    pub fn is_name(&self, word: &str) -> bool {
        self.names.contains(word)
    }

    /// Canonical capitalized form for `word` if it is a known name.
    pub fn canonical_name(&self, word: &str) -> Option<&str> {
        let lower = word.to_lowercase();
        self.names.iter().find(|n| n.to_lowercase() == lower).map(|s| s.as_str())
    }

    /// Typo-repair candidates for `word`: (candidate_canonical, similarity_0_to_1).
    ///
    /// Policy:
    /// - Never repair: known word, name, protected (numbers/paths/urls/quoted), len < 4, all-caps.
    /// - len 4-6: Damerau-Levenshtein distance 1 only; must share first letter.
    /// - len >= 7: DL distance 1 or 2; must share first letter; for dist-2 only anagrams (same multiset of letters).
    /// - Best candidate must beat runner-up by > 0.05 margin or be unique.
    pub fn candidates(&self, word: &str) -> Vec<(String, f32)> {
        if should_protect(word) {
            return vec![];
        }
        let lower = word.to_lowercase();
        // Don't repair already-known words
        if self.words.contains_key(&lower) {
            return vec![];
        }
        let word_len = lower.chars().count();
        // min 3 chars to attempt repair (len 1-2 are too ambiguous)
        if word_len < 3 {
            return vec![];
        }
        // All-caps: treat as abbreviation / acronym, don't repair
        if word.chars().all(|c| c.is_uppercase()) {
            return vec![];
        }
        let max_dist = if word_len >= 7 { 2usize } else { 1usize };
        let first_char = lower.chars().next().unwrap_or('_');

        let mut lower_sorted: Vec<char> = lower.chars().collect();
        lower_sorted.sort_unstable();

        let mut hits: Vec<(String, f32)> = vec![];

        for (candidate, entry) in &self.words {
            // Must share the same first letter
            if !candidate.starts_with(first_char) {
                continue;
            }
            let dist = strsim::damerau_levenshtein(&lower, candidate);
            if dist == 0 || dist > max_dist {
                continue;
            }
            if dist == 2 {
                // For 2-edit repairs require the same letter multiset (transpositions + no substitutions)
                let mut cand_sorted: Vec<char> = candidate.chars().collect();
                cand_sorted.sort_unstable();
                if cand_sorted != lower_sorted {
                    continue;
                }
            }
            let jw = strsim::jaro_winkler(&lower, candidate) as f32;
            let len_match = candidate.chars().count() == word_len;
            // Transpositions (same char multiset, DL=1) get a large bonus so they
            // rank above substitutions of the same edit distance.
            let is_transposition = dist == 1 && {
                let mut cand_sorted: Vec<char> = candidate.chars().collect();
                cand_sorted.sort_unstable();
                cand_sorted == lower_sorted
            };
            let score = if is_transposition {
                2.0f32
            } else {
                jw + if len_match { 0.15 } else { 0.0 }
            } + (entry.freq as f32 * 0.001);
            hits.push((candidate.clone(), score));
        }

        if hits.is_empty() {
            return vec![];
        }

        // Sort by score desc
        hits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Require best candidate to beat runner-up by > 0.05 margin (or be unique)
        if hits.len() >= 2 && (hits[0].1 - hits[1].1) <= 0.05 {
            return vec![];
        }

        // If the candidate is a name, return it in its proper capitalized form
        hits.into_iter()
            .map(|(c, sim)| {
                let display = self
                    .canonical_name(&c)
                    .map(|s| s.to_string())
                    .unwrap_or(c);
                (display, sim.min(1.0))
            })
            .collect()
    }

    /// All known verb lemmas (for vocab building).
    pub fn verb_lemmas(&self) -> Vec<String> {
        self.words
            .iter()
            .filter(|(_, e)| e.kind == WordKind::Verb)
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// All known noun lemmas (for vocab building).
    pub fn noun_lemmas(&self) -> Vec<String> {
        self.words
            .iter()
            .filter(|(_, e)| matches!(e.kind, WordKind::Noun | WordKind::Name))
            .map(|(k, _)| k.clone())
            .collect()
    }
}

impl Default for Lexicon {
    fn default() -> Self {
        Lexicon::new()
    }
}

/// Returns true if the word should NOT be modified by typo repair.
/// This covers structural protections. Length and known-word checks are in `candidates`.
pub fn should_protect(word: &str) -> bool {
    if word.is_empty() {
        return true;
    }
    let first = word.chars().next().unwrap();
    // Capitalized words that look like proper names
    if first.is_uppercase() {
        return true;
    }
    // Numbers
    if word.parse::<f64>().is_ok() {
        return true;
    }
    // URLs
    if word.starts_with("http://") || word.starts_with("https://") {
        return true;
    }
    // Paths
    if word.starts_with('/') || word.starts_with("./") || word.starts_with("~/") {
        return true;
    }
    // Quoted strings
    if word.starts_with('"') {
        return true;
    }
    false
}
