//! Ears: messy text -> `EarsResult` (SCE clauses).
//!
//! Path order: normalize -> native recognizer (phrasings + retrieval) ->
//! LLM normalizer -> parser gate. Every LLM hit is stored as a `Pair` so the
//! native recognizer can learn the phrasing and the LLM is needed less.
//!
//! Owned by the ears subagent. The SCE parser is reached only through the
//! `Gate` trait so this module compiles independently of `sce/`.

pub mod bench;
pub mod gate;
pub mod induce;
pub mod lexicon;
pub mod llm;
pub mod normalize;
pub mod phrasings;
pub mod values;

use std::path::Path;

use spoon_core::llm::{LlmClient, LlmConfig};
use spoon_core::types::clause::{EarsPath, EarsResult};
use spoon_core::types::episode::Pair;

use crate::ears::induce::induce_phrasings;
use crate::ears::lexicon::Lexicon;
use crate::ears::llm::{build_prompt, call_llm_normalizer, strip_non_sce};
use crate::ears::normalize::normalize;
use crate::ears::phrasings::{tokenize_for_bm25, PhrasingStore};
use crate::ears::values::spot_values;

// ---- Gate trait ----

/// The hard parse gate. Implemented by the SCE Earley parser;
/// tests use a `FakeGate`.
pub trait Gate: Send + Sync {
    /// Parse one or more SCE sentences into clauses.
    /// Returns `Err(human_readable_message)` if parsing fails.
    fn parse(&self, sce: &str) -> Result<Vec<spoon_core::types::clause::Clause>, String>;
}

// ---- EarsConfig ----

pub struct EarsConfig {
    pub threshold: f32,
    pub model_timeout: std::time::Duration,
}

impl Default for EarsConfig {
    fn default() -> Self {
        EarsConfig {
            threshold: 0.72,
            model_timeout: std::time::Duration::from_secs(60),
        }
    }
}

// ---- Ears ----

pub struct Ears {
    lexicon: Lexicon,
    phrasings: PhrasingStore,
    llm: Option<(LlmClient, LlmConfig)>,
    config: EarsConfig,
    /// Contents of normalizer_base.md, loaded once.
    normalizer_prompt: String,
}

impl Ears {
    /// Construct from already-built components. Loads the normalizer prompt
    /// from `data/prompts/normalizer_base.md` relative to the executable's
    /// working directory (or falls back to a minimal built-in stub for tests).
    pub fn new(
        lexicon: Lexicon,
        phrasings: PhrasingStore,
        llm: Option<(LlmClient, LlmConfig)>,
    ) -> Self {
        let normalizer_prompt = load_normalizer_prompt();
        Ears {
            lexicon,
            phrasings,
            llm,
            config: EarsConfig::default(),
            normalizer_prompt,
        }
    }

    pub fn with_config(mut self, config: EarsConfig) -> Self {
        self.config = config;
        self
    }

    /// Build from the data directory (loads lexicon.json, slang.json,
    /// dialog_phrasings.json).
    pub fn from_data_dir(data_dir: &Path) -> anyhow::Result<Self> {
        let lexicon = Lexicon::load_seed_dir(&data_dir.join("seed"))?;
        let phrasings = load_phrasings(data_dir, &lexicon)?;
        Ok(Ears::new(lexicon, phrasings, None))
    }

    // ---- public API ----

    /// Full pipeline including LLM fallback (async).
    pub async fn hear(&self, text: &str, gate: &dyn Gate) -> EarsResult {
        // Path 1 + 2 + 3 (native)
        if let Some(result) = self.hear_native(text, gate) {
            return result;
        }

        // Path 4: LLM normalizer
        if let Some((client, cfg)) = &self.llm {
            if let Some(result) = self.hear_llm(text, gate, client, cfg).await {
                return result;
            }
        }

        // Path 5: Failed
        self.failed_result(text)
    }

    /// Synchronous paths only (Direct + Normalize + Native). No LLM.
    /// Returns None if no native path succeeded (caller should try LLM or return Failed).
    pub fn hear_native(&self, text: &str, gate: &dyn Gate) -> Option<EarsResult> {
        // Path 1: Try the original text directly.
        if let Ok(clauses) = gate.parse(text) {
            return Some(EarsResult {
                clauses,
                path: EarsPath::Direct,
                sce: text.to_string(),
                confidence: 1.0,
                unknown_words: vec![],
            });
        }

        // Normalize the whole text.
        let normed = normalize(text, &self.lexicon);
        let normalized_text = normed.sentences.join(" ");

        // Path 2: Try the normalized text (still "Direct" semantically).
        if !normalized_text.is_empty() && normalized_text != text {
            if let Ok(clauses) = gate.parse(&normalized_text) {
                return Some(EarsResult {
                    clauses,
                    path: EarsPath::Direct,
                    sce: normalized_text.clone(),
                    confidence: 1.0,
                    unknown_words: normed.unknown_words,
                });
            }
        }

        // Path 3: Native recognizer per sentence.
        let sentences = if normed.sentences.is_empty() { vec![normalized_text.clone()] } else { normed.sentences.clone() };
        let mut all_clauses = vec![];
        let mut worst_path = EarsPath::Direct;
        let mut all_sce = vec![];
        let mut min_confidence = 1.0f32;

        for sentence in &sentences {
            let sentence_stripped = sentence.trim_end_matches(|c: char| matches!(c, '.' | '?' | '!')).trim();

            // Spot values in this sentence for slot matching
            let spotted = spot_values(sentence);
            let query_tokens = tokenize_for_bm25(sentence_stripped);

            // Try exact phrasing match first
            if let Some(sce) = self.phrasings.exact_match(&query_tokens, &spotted) {
                match gate.parse(&sce) {
                    Ok(clauses) => {
                        worst_path = worse_path(worst_path, EarsPath::Phrasing);
                        all_clauses.extend(clauses);
                        all_sce.push(sce);
                        continue;
                    }
                    Err(_) => {}
                }
            }

            // Try BM25 retrieval + slot alignment
            let threshold = self.config.threshold;
            if let Some((sce, score)) =
                self.phrasings.retrieve_and_align(&query_tokens, &spotted, 8, 0.0)
            {
                match gate.parse(&sce) {
                    Ok(clauses) => {
                        let path = if score >= threshold {
                            EarsPath::Phrasing
                        } else {
                            EarsPath::Retrieval
                        };
                        worst_path = worse_path(worst_path, path);
                        min_confidence = min_confidence.min(score);
                        all_clauses.extend(clauses);
                        all_sce.push(sce);
                        continue;
                    }
                    Err(_) => {}
                }
            }

            // Try direct gate.parse on the sentence (last resort before failure).
            // Also try with the first letter capitalized (normalization lowercases,
            // but SCE parsers expect proper case for sentence-initial words).
            let capitalized = {
                let mut chars = sentence.chars();
                match chars.next() {
                    None => String::new(),
                    Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                }
            };
            let direct_ok = [sentence.as_str(), capitalized.as_str()].iter().find_map(|candidate| {
                gate.parse(candidate).ok().map(|clauses| (clauses, candidate.to_string()))
            });
            if let Some((clauses, sce_str)) = direct_ok {
                worst_path = worse_path(worst_path, EarsPath::Direct);
                all_clauses.extend(clauses);
                all_sce.push(sce_str);
                continue;
            }

            // This sentence failed native
            return None;
        }

        let combined_sce = all_sce.join(" ");
        Some(EarsResult {
            clauses: all_clauses,
            path: worst_path,
            sce: combined_sce,
            confidence: min_confidence,
            unknown_words: normed.unknown_words,
        })
    }

    /// LLM path (path 4). Only called when native fails.
    async fn hear_llm(
        &self,
        text: &str,
        gate: &dyn Gate,
        client: &LlmClient,
        cfg: &LlmConfig,
    ) -> Option<EarsResult> {
        // Build vocabulary: known words that overlap with input stems
        let input_tokens = tokenize_for_bm25(text);
        let mut vocab: Vec<String> = self
            .lexicon
            .verb_lemmas()
            .into_iter()
            .chain(self.lexicon.noun_lemmas())
            .filter(|w| {
                input_tokens.iter().any(|t| {
                    t.starts_with(w.as_str()) || w.starts_with(t.as_str())
                })
            })
            .take(60)
            .collect();
        vocab.sort();
        vocab.dedup();

        // Build few-shot examples from phrasing store (top-8 by BM25)
        let shots: Vec<(String, String)> = {
            let retrieved = self.phrasings.retrieve(&input_tokens, 8);
            // We can't access the raw utterance from the store directly,
            // but we have the SCE. Use the phrasing pattern words as the "utterance".
            // This is an approximation - the real implementation would store original utterances.
            retrieved
                .into_iter()
                .take(8)
                .map(|(_, _)| (String::new(), String::new())) // placeholder
                .filter(|(u, s)| !u.is_empty() && !s.is_empty())
                .collect()
        };

        let msgs = build_prompt(&self.normalizer_prompt, &vocab, &shots, text);

        match call_llm_normalizer(client, cfg, msgs).await {
            Ok(raw) => {
                let sce = strip_non_sce(&raw);
                match gate.parse(&sce) {
                    Ok(clauses) => {
                        return Some(EarsResult {
                            clauses,
                            path: EarsPath::Llm,
                            sce,
                            confidence: 0.8,
                            unknown_words: vec![],
                        });
                    }
                    Err(parser_err) => {
                        // ONE repair retry including the error
                        let repair_prompt_extra = format!(
                            "{}\n\nThe previous attempt produced this SCE which failed to parse: {}\nParser error: {}\nPlease correct the SCE.",
                            text, sce, parser_err
                        );
                        let msgs2 = build_prompt(&self.normalizer_prompt, &vocab, &[], &repair_prompt_extra);
                        if let Ok(raw2) = call_llm_normalizer(client, cfg, msgs2).await {
                            let sce2 = strip_non_sce(&raw2);
                            if let Ok(clauses2) = gate.parse(&sce2) {
                                return Some(EarsResult {
                                    clauses: clauses2,
                                    path: EarsPath::Llm,
                                    sce: sce2,
                                    confidence: 0.7,
                                    unknown_words: vec![],
                                });
                            }
                        }
                    }
                }
            }
            Err(_) => {}
        }
        None
    }

    // ---- mutation API ----

    /// Feed new (utterance, SCE) pairs to the phrasing store and induction engine.
    pub fn add_pairs(&mut self, pairs: &[Pair]) {
        let new_phrasings = induce_phrasings(pairs);
        if !new_phrasings.is_empty() {
            self.phrasings.add_many(new_phrasings);
        }
        // Also add pairs as literal phrasings (with normalized tokens)
        for pair in pairs {
            let tokens = tokenize_for_bm25(&pair.utterance);
            if tokens.is_empty() {
                continue;
            }
            let pattern: Vec<_> = tokens.into_iter().map(phrasings::Tok::Word).collect();
            self.phrasings.add(phrasings::Phrasing {
                pattern,
                sce: pair.sce.clone(),
                source: pair.source.clone(),
                credit: pair.credit,
            });
        }
    }

    /// Teach the lexicon that `word` should be treated as `canonical`.
    pub fn learn_word(&mut self, word: &str, canonical: &str) {
        self.lexicon.learn_word(word, canonical);
    }

    // ---- helpers ----

    pub fn failed_result(&self, text: &str) -> EarsResult {
        let normed = normalize(text, &self.lexicon);
        EarsResult {
            clauses: vec![],
            path: EarsPath::Failed,
            sce: normed.sentences.join(" "),
            confidence: 0.0,
            unknown_words: normed.unknown_words,
        }
    }

    /// Access to the lexicon (for tests).
    pub fn lexicon_mut(&mut self) -> &mut Lexicon {
        &mut self.lexicon
    }
}

// ---- path ordering ----

fn worse_path(a: EarsPath, b: EarsPath) -> EarsPath {
    if path_rank(&b) > path_rank(&a) { b } else { a }
}

fn path_rank(p: &EarsPath) -> u8 {
    match p {
        EarsPath::Direct => 0,
        EarsPath::Phrasing => 1,
        EarsPath::Retrieval => 2,
        EarsPath::Llm => 3,
        EarsPath::Failed => 4,
    }
}

// ---- phrasing loading ----

pub fn load_phrasings(data_dir: &Path, lexicon: &Lexicon) -> anyhow::Result<PhrasingStore> {
    let path = data_dir.join("seed/dialog_phrasings.json");
    PhrasingStore::load(&path, |utterance| {
        // Normalize the utterance using slang/elongation but NOT typo repair or name-casing
        // (we want a stable normalized form for matching)
        let normed = normalize(utterance, lexicon);
        // Return all tokens from all sentences joined
        let combined = normed.sentences.join(" ");
        tokenize_for_bm25(&combined)
    })
}

fn load_normalizer_prompt() -> String {
    // Try loading from the standard data path relative to workspace root.
    // CARGO_MANIFEST_DIR is crates/spoon-lang; go up two levels.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let workspace = std::path::Path::new(manifest)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let prompt_path = workspace.join("data/prompts/normalizer_base.md");
    if let Ok(s) = std::fs::read_to_string(&prompt_path) {
        return s;
    }
    // Fallback: try CWD-relative paths
    for rel in &["data/prompts/normalizer_base.md", "../../data/prompts/normalizer_base.md"] {
        if let Ok(s) = std::fs::read_to_string(rel) {
            return s;
        }
    }
    // Minimal stub
    "You are a normalizer. Output SCE only.\n\n# VOCABULARY\n{{VOCABULARY}}\n\n{{CONTEXT}}\n\nInput: {{UTTERANCE}}".to_string()
}
