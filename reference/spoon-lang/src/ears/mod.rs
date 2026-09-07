//! Ears: messy text -> `EarsResult` (SCE clauses).
//!
//! Per utterance: normalize (rules) -> whole-utterance phrasing -> pristine
//! SCE -> per sentence: learned phrasing (aligned, slots filled), clean
//! direct parse -> LLM normalizer for the sentences that are still open ->
//! dirty direct parse as the last resort. A direct parse is "clean" only when
//! the parser and the lexicon place every word; the one exception is a command
//! whose only unknown word is its verb, which is how new verbs reach the learner.
//! Every LLM hit is stored as a `Pair` so the native recognizer can learn the
//! phrasing and the LLM is needed less.
//!
//! Owned by the ears subagent. The SCE parser is reached only through the
//! `Gate` trait so this module compiles independently of `sce/`.

pub mod arith;
pub mod bench;
pub mod gate;
pub mod guard;
pub mod induce;
pub mod lexicon;
pub mod llm;
pub mod loops;
pub mod normalize;
pub mod phrasings;
pub mod reported;
pub mod rules;
pub mod sentence;
pub mod values;

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use spoon_core::llm::{LlmClient, LlmConfig};
use spoon_core::types::clause::{Act, Clause, EarsPath, EarsResult, Quant, Term};
use spoon_core::types::episode::Pair;

use crate::ears::guard::{invents_numbers, is_garbage, NO_MEANING};
use crate::ears::induce::induce_phrasings;
use crate::ears::lexicon::Lexicon;
use crate::ears::llm::{build_prompt, build_repair_prompt, call_llm_normalizer, strip_non_sce};
use crate::ears::normalize::{normalize, Normalized};
use crate::ears::phrasings::{tokenize_for_bm25, PhrasingStore};
use crate::ears::values::spot_values;

// ---- Gate trait ----

/// The hard parse gate. Implemented by the SCE Earley parser;
/// tests use a `FakeGate`.
pub trait Gate: Send + Sync {
    /// Parse one or more SCE sentences into clauses.
    /// Returns `Err(human_readable_message)` if parsing fails.
    fn parse(&self, sce: &str) -> Result<Vec<Clause>, String>;

    /// `parse` plus the words the parser placed by position only (unknown
    /// verbs, nouns, adjectives). Gates without that information report none.
    fn parse_reported(&self, sce: &str) -> Result<(Vec<Clause>, Vec<String>), String> {
        self.parse(sce).map(|clauses| (clauses, Vec::new()))
    }
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

/// Counters for the LLM seat, read by the bench.
#[derive(Default, Debug)]
pub struct EarsStats {
    pub llm_calls: AtomicU32,
    pub repairs: AtomicU32,
    pub repairs_ok: AtomicU32,
}

impl EarsStats {
    pub fn snapshot(&self) -> (u32, u32, u32) {
        (
            self.llm_calls.load(Ordering::Relaxed),
            self.repairs.load(Ordering::Relaxed),
            self.repairs_ok.load(Ordering::Relaxed),
        )
    }
}

/// Confidence of a direct parse that carries unknown words (the last resort).
const DIRTY_DIRECT_CONFIDENCE: f32 = 0.3;

/// Confidence of a direct parse whose frame is known and whose only unknown
/// words are new vocabulary (`The Avengers are fictional super heroes.`).
const NEW_VOCABULARY_CONFIDENCE: f32 = 0.85;

// ---- Ears ----

pub struct Ears {
    lexicon: Lexicon,
    phrasings: PhrasingStore,
    llm: Option<(LlmClient, LlmConfig)>,
    config: EarsConfig,
    /// Contents of normalizer_base.md, loaded once.
    normalizer_prompt: String,
    stats: EarsStats,
}

/// One resolved sentence (or whole utterance).
#[derive(Debug, Clone)]
struct Resolved {
    sce: String,
    clauses: Vec<Clause>,
    path: EarsPath,
    confidence: f32,
    unknown: Vec<String>,
}

/// A sentence after the native paths: resolved, or still open for the LLM.
#[derive(Debug, Clone)]
enum Outcome {
    Done(Resolved),
    Open(String),
}

/// What the LLM seat came back with.
enum LlmOutcome {
    Hit(Resolved),
    /// The model declined (`??`) or invented SCE from nothing: the utterance
    /// is Failed and the words nobody knows are reported.
    Garbage(Vec<String>),
    /// Nothing parseable; the last resort gets its turn.
    Miss,
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
            stats: EarsStats::default(),
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

    pub fn stats(&self) -> &EarsStats {
        &self.stats
    }

    // ---- public API ----

    /// Full pipeline including LLM fallback (async).
    pub async fn hear(&self, text: &str, gate: &dyn Gate) -> EarsResult {
        let normed = normalize(text, &self.lexicon);
        let mut outcomes = match self.resolve_native(text, &normed, gate) {
            Ok(whole) => return self.finish(vec![Outcome::Done(whole)], &normed, text),
            Err(outcomes) => outcomes,
        };

        // Any open sentence sends the whole normalized utterance to the LLM
        // normalizer: the few-shot examples are whole utterances, and a small
        // model rewrites "hello. What is going on?" better than the fragment.
        if let Some((client, cfg)) = &self.llm {
            if outcomes.iter().any(|o| matches!(o, Outcome::Open(_))) {
                let joined = normed.sentences.join(" ");
                let llm_input = if joined.trim().is_empty() { text } else { joined.as_str() };
                match self.hear_llm(llm_input, gate, client, cfg).await {
                    LlmOutcome::Hit(resolved) => return self.finish(vec![Outcome::Done(resolved)], &normed, text),
                    LlmOutcome::Garbage(unknown) => {
                        let mut failed = self.failed_from(&normed, text);
                        for u in unknown {
                            push_unknown(&mut failed.unknown_words, u);
                        }
                        return failed;
                    }
                    LlmOutcome::Miss => {}
                }
            }
        }

        self.close_with_last_resort(&mut outcomes, gate);
        self.finish(outcomes, &normed, text)
    }

    /// Synchronous paths only (phrasings, clean direct parses, retrieval). No LLM.
    /// Returns None if any sentence needs the LLM (caller should try LLM or
    /// fall back to `hear_offline`).
    pub fn hear_native(&self, text: &str, gate: &dyn Gate) -> Option<EarsResult> {
        let normed = normalize(text, &self.lexicon);
        match self.resolve_native(text, &normed, gate) {
            Ok(whole) => Some(self.finish(vec![Outcome::Done(whole)], &normed, text)),
            Err(outcomes) => {
                if outcomes.iter().all(|o| matches!(o, Outcome::Done(_))) {
                    Some(self.finish(outcomes, &normed, text))
                } else {
                    None
                }
            }
        }
    }

    /// Everything `hear` does without an LLM: native paths, then the dirty
    /// direct parse as the last resort, then `Failed`.
    pub fn hear_offline(&self, text: &str, gate: &dyn Gate) -> EarsResult {
        let normed = normalize(text, &self.lexicon);
        let mut outcomes = match self.resolve_native(text, &normed, gate) {
            Ok(whole) => return self.finish(vec![Outcome::Done(whole)], &normed, text),
            Err(outcomes) => outcomes,
        };
        self.close_with_last_resort(&mut outcomes, gate);
        self.finish(outcomes, &normed, text)
    }

    // ---- native resolution ----

    /// Native paths. `Ok` is a whole-utterance hit (phrasing or pristine
    /// SCE); `Err` carries the per-sentence outcomes.
    fn resolve_native(&self, text: &str, normed: &Normalized, gate: &dyn Gate) -> Result<Resolved, Vec<Outcome>> {
        let sentences: Vec<String> = if normed.sentences.is_empty() {
            vec![text.trim().to_string()]
        } else {
            normed.sentences.clone()
        };

        // (1) The whole utterance as one learned phrasing ("hey how's it going").
        let joined = sentences.join(" ");
        if sentences.len() > 1 {
            if let Some(r) = self.phrasing(&joined, gate) {
                return Ok(r);
            }
        }

        // (2) Pristine SCE typed by the user: capitalized, terminated, and
        // parsed with every word placed. Anything else is normalized first
        // (the grammar happens to accept `which customer bought the thing?`),
        // and `Wolves are white.` is a universal, not a fact about a Name.
        let pristine = looks_pristine(text);
        if pristine {
            if let Some(r) = self.clean_direct(text, gate).filter(|r| r.confidence >= 1.0) {
                return Ok(r);
            }
        }

        let outcomes: Vec<Outcome> = sentences
            .iter()
            .map(|sentence| match self.resolve_sentence(sentence, gate) {
                Some(r) => Outcome::Done(r),
                None => Outcome::Open(sentence.clone()),
            })
            .collect();

        // (3) The rules had their turn and did not close the utterance. If the
        // user wrote SCE, take it as written: the words the rules could not
        // place are new vocabulary, not noise.
        if pristine && outcomes.iter().any(|o| matches!(o, Outcome::Open(_))) {
            if let Some(r) = self.clean_direct(text, gate) {
                return Ok(r);
            }
        }
        Err(outcomes)
    }

    /// One sentence: learned phrasing, then a clean direct parse.
    fn resolve_sentence(&self, sentence: &str, gate: &dyn Gate) -> Option<Resolved> {
        self.phrasing(sentence, gate).or_else(|| self.clean_direct(sentence, gate))
    }

    /// Phrasing alignment: every stored pattern whose words match the text,
    /// slots filled from the spotted values; the first one the gate accepts wins.
    /// Alignment must consume the whole text, so this is exact matching with
    /// slots, not fuzzy retrieval.
    fn phrasing(&self, text: &str, gate: &dyn Gate) -> Option<Resolved> {
        let stripped = text.trim_end_matches(|c: char| matches!(c, '.' | '?' | '!'));
        let spotted = spot_values(text);
        let tokens = tokenize_for_bm25(stripped);
        if tokens.is_empty() {
            return None;
        }
        for sce in self.phrasings.exact_matches(&tokens, &spotted) {
            if let Ok((clauses, unknown)) = gate.parse_reported(&sce) {
                return Some(Resolved { sce, clauses, path: EarsPath::Phrasing, confidence: 1.0, unknown });
            }
        }
        None
    }

    /// Direct parse accepted when the parser placed every word, or when it
    /// placed every *frame* word and the rest is new vocabulary (see module docs).
    fn clean_direct(&self, text: &str, gate: &dyn Gate) -> Option<Resolved> {
        let (clauses, unknown) = self.parse_with_unknowns(text, gate)?;
        let confidence = match judge_direct(&clauses, &unknown, &self.lexicon) {
            DirectVerdict::Clean => 1.0,
            DirectVerdict::NewVocabulary => NEW_VOCABULARY_CONFIDENCE,
            DirectVerdict::Reject => return None,
        };
        Some(Resolved { sce: text.trim().to_string(), clauses, path: EarsPath::Direct, confidence, unknown })
    }

    /// Direct parse that tolerates unknown words: the last resort.
    fn dirty_direct(&self, text: &str, gate: &dyn Gate) -> Option<Resolved> {
        let (clauses, unknown) = self.parse_with_unknowns(text, gate)?;
        Some(Resolved {
            sce: text.trim().to_string(),
            clauses,
            path: EarsPath::Direct,
            confidence: DIRTY_DIRECT_CONFIDENCE,
            unknown,
        })
    }

    /// Words nobody knows: parser-reported unknowns that are not ordinary
    /// vocabulary in the ears lexicon, plus proper names the lexicon has never
    /// seen. (`nurse` is unknown to the SCE grammar but a fine noun; `whats`
    /// and `Hello`-the-Name are junk.)
    fn parse_with_unknowns(&self, text: &str, gate: &dyn Gate) -> Option<(Vec<Clause>, Vec<String>)> {
        let (clauses, mut unknown) = gate.parse_reported(text).ok()?;
        if clauses.is_empty() {
            return None;
        }
        unknown.retain(|w| !self.lexicon.is_content_word(w));
        for name in unknown_names(&clauses, &self.lexicon) {
            if !unknown.contains(&name) {
                unknown.push(name);
            }
        }
        Some((clauses, unknown))
    }

    fn close_with_last_resort(&self, outcomes: &mut [Outcome], gate: &dyn Gate) {
        for outcome in outcomes.iter_mut() {
            if let Outcome::Open(sentence) = outcome {
                if let Some(r) = self.dirty_direct(sentence, gate) {
                    *outcome = Outcome::Done(r);
                }
            }
        }
    }

    /// Combine outcomes into the result. Any sentence still open means the
    /// whole utterance failed: the interior must not act on half of it.
    fn finish(&self, outcomes: Vec<Outcome>, normed: &Normalized, text: &str) -> EarsResult {
        let mut resolved = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            match outcome {
                Outcome::Done(r) => resolved.push(r),
                Outcome::Open(_) => return self.failed_from(normed, text),
            }
        }
        let mut clauses: Vec<Clause> = vec![];
        let mut parts: Vec<String> = vec![];
        let mut path = EarsPath::Direct;
        let mut confidence = 1.0f32;
        let mut unknown: Vec<String> = vec![];
        for r in resolved {
            let sce = r.sce.trim().to_string();
            // "hi whats up" is one greeting, not two.
            if parts.last().is_some_and(|prev| *prev == sce) {
                continue;
            }
            path = worse_path(path, r.path);
            confidence = confidence.min(r.confidence);
            for u in r.unknown {
                push_unknown(&mut unknown, u);
            }
            clauses.extend(r.clauses);
            parts.push(sce);
        }
        for u in &normed.unknown_words {
            push_unknown(&mut unknown, u.clone());
        }
        EarsResult { clauses, path, sce: parts.join(" "), confidence, unknown_words: unknown }
    }

    fn failed_from(&self, normed: &Normalized, text: &str) -> EarsResult {
        let sce = if normed.sentences.is_empty() { text.to_string() } else { normed.sentences.join(" ") };
        EarsResult {
            clauses: vec![],
            path: EarsPath::Failed,
            sce,
            confidence: 0.0,
            unknown_words: normed.unknown_words.clone(),
        }
    }

    // ---- LLM path ----

    /// LLM normalizer over the normalized utterance. One repair retry on parse
    /// failure. `??` (the prompt's escape hatch) and results with no known
    /// content word are garbage, not misses.
    async fn hear_llm(&self, text: &str, gate: &dyn Gate, client: &LlmClient, cfg: &LlmConfig) -> LlmOutcome {
        // Vocabulary: known words that overlap with input stems
        let input_tokens = tokenize_for_bm25(text);
        let mut vocab: Vec<String> = self
            .lexicon
            .verb_lemmas()
            .into_iter()
            .chain(self.lexicon.noun_lemmas())
            .filter(|w| input_tokens.iter().any(|t| t.starts_with(w.as_str()) || w.starts_with(t.as_str())))
            .take(60)
            .collect();
        vocab.sort();
        vocab.dedup();
        let debug = std::env::var("SPOON_EARS_DEBUG").is_ok();

        let msgs = build_prompt(&self.normalizer_prompt, &vocab, &[], text);
        self.stats.llm_calls.fetch_add(1, Ordering::Relaxed);
        let Ok(raw) = call_llm_normalizer(client, cfg, msgs).await else {
            return LlmOutcome::Miss;
        };
        let sce = strip_non_sce(&raw);
        if sce == NO_MEANING {
            return LlmOutcome::Garbage(vec![]);
        }
        let parser_err = match self.parse_with_unknowns(&sce, gate) {
            Some((clauses, unknown)) => return self.accept_llm(text, sce, clauses, unknown, 0.8),
            None => gate.parse(&sce).err().unwrap_or_else(|| "no clauses".to_string()),
        };
        if debug {
            eprintln!("DBG llm input={text:?} raw={raw:?} err={parser_err:?}");
        }

        // ONE repair retry: tight system prompt with the error message
        self.stats.repairs.fetch_add(1, Ordering::Relaxed);
        self.stats.llm_calls.fetch_add(1, Ordering::Relaxed);
        let msgs2 = build_repair_prompt(text, &sce, &parser_err, &vocab);
        let Ok(raw2) = call_llm_normalizer(client, cfg, msgs2).await else {
            return LlmOutcome::Miss;
        };
        let sce2 = strip_non_sce(&raw2);
        if debug {
            eprintln!("DBG repair raw={raw2:?} err={:?}", gate.parse(&sce2).err());
        }
        if sce2 == NO_MEANING {
            return LlmOutcome::Garbage(vec![]);
        }
        let Some((clauses, unknown)) = self.parse_with_unknowns(&sce2, gate) else {
            return LlmOutcome::Miss;
        };
        self.stats.repairs_ok.fetch_add(1, Ordering::Relaxed);
        self.accept_llm(text, sce2, clauses, unknown, 0.7)
    }

    /// A parsed LLM result is a hit unless the guard finds no known content
    /// word, or a number the input never mentioned (a copied prompt example).
    fn accept_llm(&self, input: &str, sce: String, clauses: Vec<Clause>, unknown: Vec<String>, confidence: f32) -> LlmOutcome {
        if is_garbage(&sce, &unknown) || invents_numbers(&sce, input) {
            if std::env::var("SPOON_EARS_DEBUG").is_ok() {
                eprintln!("DBG garbage sce={sce:?} unknown={unknown:?}");
            }
            return LlmOutcome::Garbage(unknown);
        }
        LlmOutcome::Hit(Resolved { sce, clauses, path: EarsPath::Llm, confidence, unknown })
    }

    // ---- mutation API ----

    /// Feed new (utterance, SCE) pairs to the phrasing store and induction engine.
    /// Utterances are normalized first so learned patterns live in the same
    /// token space the recognizer queries.
    pub fn add_pairs(&mut self, pairs: &[Pair]) {
        let normalized: Vec<Pair> = pairs
            .iter()
            .map(|p| {
                let normed = normalize(&p.utterance, &self.lexicon);
                let utterance = if normed.sentences.is_empty() { p.utterance.clone() } else { normed.sentences.join(" ") };
                Pair { utterance, ..p.clone() }
            })
            .collect();
        let new_phrasings = induce_phrasings(&normalized);
        if !new_phrasings.is_empty() {
            self.phrasings.add_many(new_phrasings);
        }
        // Also add pairs as literal phrasings (with normalized tokens)
        for pair in &normalized {
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
        self.failed_from(&normed, text)
    }

    /// Access to the lexicon (for tests).
    pub fn lexicon_mut(&mut self) -> &mut Lexicon {
        &mut self.lexicon
    }
}

// ---- clean-parse policy ----

/// Starts with a capital (or a quote) and ends with an SCE terminator.
fn looks_pristine(text: &str) -> bool {
    let t = text.trim();
    t.starts_with(|c: char| c.is_uppercase() || c == '"') && t.ends_with(['.', '?', '!'])
}

/// How much a direct parse is worth.
#[derive(Debug, PartialEq, Eq)]
enum DirectVerdict {
    /// Every word placed and known.
    Clean,
    /// The frame is known and every unknown word sits in a vocabulary slot:
    /// a proper name, a noun, or a modifier. New words, not junk.
    NewVocabulary,
    /// Not good enough for the direct path.
    Reject,
}

/// The direct-path policy.
///
/// Junk parses because SCE's lexicon is open: `Hello whats up.` reads as a
/// Name plus a verb nobody knows, and `Sup dude.` the same. A real assertion
/// about new things (`The Avengers are fictional super heroes.`) has the same
/// unknown-word count but a frame the interior recognizes: the copula or a
/// known verb, with the unknowns only in naming positions. That is the line
/// this draws.
fn judge_direct(clauses: &[Clause], unknown: &[String], lexicon: &Lexicon) -> DirectVerdict {
    if unknown.is_empty() {
        return DirectVerdict::Clean;
    }
    // A command whose only unknown word is its verb: how new verbs reach the learner.
    if let [c] = clauses {
        let only_the_verb =
            unknown.iter().all(|u| c.conditions.iter().any(|p| p.pred.eq_ignore_ascii_case(u)));
        if matches!(c.act, Act::Command) && only_the_verb {
            return DirectVerdict::Clean;
        }
    }
    if !clauses.iter().all(|c| frame_is_known(c, lexicon)) {
        return DirectVerdict::Reject;
    }
    let slots = vocabulary_slots(clauses);
    if unknown.iter().all(|u| slots.iter().any(|s| s.eq_ignore_ascii_case(u))) {
        DirectVerdict::NewVocabulary
    } else {
        DirectVerdict::Reject
    }
}

/// Every predicate of the clause is the copula or a verb the ears know.
/// An unknown verb is the one thing an assertion cannot recover from: it is
/// what separates `whats up` from `sees the Avengers movie`.
fn frame_is_known(clause: &Clause, lexicon: &Lexicon) -> bool {
    for pred in clause.conditions.iter().chain(clause.then.iter()) {
        if pred.pred != "be" && !lexicon.knows_verb(&pred.pred) {
            return false;
        }
        for term in &pred.args {
            if let Term::Sub { clause } = term {
                if !frame_is_known(clause, lexicon) {
                    return false;
                }
            }
        }
    }
    true
}

/// Words the parse placed as vocabulary rather than structure: proper names,
/// head nouns, modifiers, and copula attributes. An unknown word here is a
/// word to learn; anywhere else it is junk.
fn vocabulary_slots(clauses: &[Clause]) -> Vec<String> {
    fn walk(clause: &Clause, out: &mut Vec<String>) {
        for r in clause.referents.iter().chain(clause.then_referents.iter()) {
            if let Quant::Named(name) = &r.quant {
                out.push(name.clone());
            }
            out.extend(r.noun.clone());
            out.extend(r.mods.iter().cloned());
        }
        for pred in clause.conditions.iter().chain(clause.then.iter()) {
            out.extend(pred.attr.clone());
            for term in &pred.args {
                if let Term::Sub { clause } = term {
                    walk(clause, out);
                }
            }
        }
    }
    let mut out = vec![];
    for c in clauses {
        walk(c, &mut out);
    }
    // A plural in the text, a singular in the parse (`heroes` / `hero`):
    // report both spellings so the unknown-word list matches either way.
    let inflections: Vec<String> = out
        .iter()
        .flat_map(|w| [format!("{w}s"), format!("{w}es")])
        .collect();
    out.extend(inflections);
    out
}

/// Proper names in the clauses that the lexicon does not know. Reserved names,
/// variables (`X`, `Y1`) and minted names (`Object-X`) are SCE syntax and never
/// count as unknown.
fn unknown_names(clauses: &[Clause], lexicon: &Lexicon) -> Vec<String> {
    fn walk(clause: &Clause, lexicon: &Lexicon, out: &mut Vec<String>) {
        for r in clause.referents.iter().chain(clause.then_referents.iter()) {
            if let Quant::Named(name) = &r.quant {
                if is_unknown_name(name, lexicon) && !out.contains(name) {
                    out.push(name.clone());
                }
            }
        }
        for pred in clause.conditions.iter().chain(clause.then.iter()) {
            for term in &pred.args {
                if let Term::Sub { clause } = term {
                    walk(clause, lexicon, out);
                }
            }
        }
    }
    let mut out = vec![];
    for c in clauses {
        walk(c, lexicon, &mut out);
    }
    out
}

fn is_unknown_name(name: &str, lexicon: &Lexicon) -> bool {
    if matches!(name, "User" | "Assistant" | "Spoon" | "It") {
        return false;
    }
    let mut chars = name.chars();
    let is_var = matches!(chars.next(), Some(c) if c.is_uppercase()) && chars.all(|c| c.is_ascii_digit());
    let minted = name.split_once('-').is_some_and(|(_, after)| after.starts_with(|c: char| c.is_uppercase()));
    if is_var || minted {
        return false;
    }
    !lexicon.is_name(name) && lexicon.canonical_name(name).is_none()
}

// ---- path ordering ----

/// Unknown words are reported once, whatever their casing (`Kealan` / `kealan`).
fn push_unknown(list: &mut Vec<String>, word: String) {
    if !list.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
        list.push(word);
    }
}

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
        // Same normalization as a live query, so patterns and queries share one token space.
        let normed = normalize(utterance, lexicon);
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
