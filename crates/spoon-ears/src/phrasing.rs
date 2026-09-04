//! Learned phrasings: how the native path stops being frozen.
//!
//! [`NativeEars`](crate::NativeEars) handles greetings and arithmetic and
//! nothing else, and it will handle exactly that forever no matter how much
//! Spoon is used. Every reading the model produces that turns out to work is a
//! worked example of how *this* user talks. Abstracted into a template and
//! stored, it lets the native path recognize the same shape next time for free.
//! That is the weaning curve, and this module is the machinery under it.
//!
//! # The template scheme
//!
//! An utterance is normalized (filler and request prefixes stripped, by
//! [`NativeEars::normalize`], so the same cleaning rules apply here as
//! everywhere else) and then tokenized. A token becomes a **slot** when two
//! things are true:
//!
//! 1. It looks variable: an integer, a float, a quoted string, or a
//!    capitalised word shaped like a name.
//! 2. The same value appears somewhere in the concept steps.
//!
//! The second condition is what keeps templates honest. A number in the text
//! that the reading never mentions cannot be a slot, because filling it would
//! change the utterance without changing the steps: `top 5 results` and `top 7
//! results` would produce identical readings, which is a wrong answer dressed
//! as a cheap one. Unlinked variable-looking tokens stay literal, so that
//! template only ever matches itself.
//!
//! The same substitution runs over the steps: the value is replaced by a hole,
//! numbered above any hole the model already put there, so a genuine "something
//! unspecified" hole in the reading is never confused with a slot.
//!
//! Repeated values share one slot. `add 5 and 5` becomes `add {0} and {0}`, so
//! filling it demands both tokens agree. Giving them separate slots would let
//! `add 7 and 9` fill only the first and silently produce `add<7, 7>`.
//!
//! # Why the threshold is high
//!
//! **A wrong native reading is worse than no native reading.** A miss costs a
//! model call, which is money and a second or two. A wrong hit costs the right
//! answer: the model would have understood the sentence, and now nothing will,
//! and the user gets a confident reply about something they did not ask for.
//! So the gates are deliberately strict. Content words must overlap heavily,
//! every slot must be supplied with a value of the right kind, and the final
//! confidence has to clear [`MIN_CONFIDENCE`]. Anything short of that returns
//! `None` and lets the model earn its keep.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use spoon_concept::{Bindings, Concept, HoleId, max_hole, pre_order, substitute};
use spoon_store::Store;
use spoon_store::pairs::PairSource;

use crate::NativeEars;

/// Confidence a reading must reach before the native path will claim it.
///
/// Calibrated so that a template from a fresh model reading clears it on an
/// exact structural match and on a fuzzy match with complete content-word
/// agreement, and on nothing else. A user-confirmed phrasing carries a higher
/// prior and so generalizes a little further, which is the trust ordering doing
/// real work rather than decorating a struct.
pub const MIN_CONFIDENCE: f64 = 0.70;

/// Similarity credited to a token-for-token match. Not 1.0: a template is
/// still an induction from one example.
const EXACT_SIMILARITY: f64 = 0.95;

/// A fuzzy match starts here and earns the rest from agreement, topping out
/// just below an exact match.
const FUZZY_FLOOR: f64 = 0.45;
const FUZZY_SPAN: f64 = 0.45;

/// Content-word agreement below this is not a match at all, whatever else
/// lines up. This is the gate that keeps `double` from answering for `triple`.
const MIN_CONTENT_OVERLAP: f64 = 0.75;

/// Utterances at or under this many tokens count as short, which is where
/// edit distance is allowed to stand in for exact word equality. On a long
/// utterance there is plenty of other signal and every fuzzy pairing is another
/// chance to be wrong.
const SHORT_INPUT_TOKENS: usize = 6;

/// How alike two words must be to count as the same word on a short input.
/// Jaro-Winkler rewards a shared prefix, which is what typos usually preserve.
const WORD_SIMILARITY: f64 = 0.90;

/// An utterance shorter than this carries no shape worth matching. One
/// character can only ever match by accident.
const MIN_CHARS: usize = 2;

/// Pairs read out of the store when the index is built. Ranked by standing, so
/// a cap drops the least trusted first.
const MAX_PAIRS: usize = 2000;

/// Closed-class words: grammar rather than content.
///
/// Overlap is measured on content words only, because function words are
/// present in almost every sentence and counting them would make every template
/// look like every other one. Discourse particles ("yeah", "lol", "ight") are
/// here for the same reason: they mark attitude, not subject matter.
///
/// Three groups are deliberately absent. **Wh-words** stay content, because
/// "who owns a dog" and "what owns a dog" are different questions and merging
/// them would pick the wrong reading. **Negation** stays content, for the same
/// reason. **Verbs** stay content, because "get the file" and "delete the file"
/// share every other word and must not be allowed to match.
const STOPWORDS: &[&str] = &[
    "a", "about", "after", "again", "against", "all", "am", "an", "and", "any", "are", "around",
    "as", "at", "be", "because", "been", "before", "being", "between", "both", "but", "by", "did",
    "do", "does", "doing", "done", "down", "during", "each", "eh", "every", "few", "for", "from",
    "further", "had", "has", "have", "having", "he", "hello", "her", "here", "hers", "hey", "hi",
    "him", "his", "hmm", "huh", "i", "id", "if", "ight", "ill", "im", "in", "into", "is", "it",
    "its", "ive", "k", "lets", "lmao", "lol", "may", "me", "might", "mine", "more", "most", "must",
    "my", "myself", "nah", "naw", "near", "of", "off", "oh", "ok", "okay", "on", "onto", "only",
    "or", "other", "our", "ours", "out", "over", "own", "please", "s", "same", "shall", "she",
    "should", "so", "some", "such", "than", "thanks", "that", "thats", "the", "their", "theirs",
    "them", "then", "there", "these", "they", "this", "those", "through", "to", "too", "u",
    "under", "up", "ur", "us", "very", "was", "we", "were", "while", "will", "with", "would",
    "yeah", "yep", "yes", "you", "your", "yours", "youre", "yourself", "yup",
];

fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&word)
}

// ---------------------------------------------------------------- tokenizing

/// One token, in both the spelling that survived normalization and the
/// spelling the speaker actually used.
///
/// Both are needed. Matching compares the normalized key, but slot detection
/// needs the original: normalization lowercases, and a lowercased `John` is no
/// longer name-shaped.
#[derive(Debug, Clone)]
struct Tok {
    key: String,
    raw: String,
}

/// Split on whitespace, keeping a double-quoted span together as one token.
///
/// Built from `chars`, never from byte slicing, so arbitrary unicode goes
/// through untouched instead of panicking on a boundary. An unterminated quote
/// yields a span with no closing quote, which then fails the quoted-value test
/// and is treated as an ordinary word.
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            let mut span = String::from('"');
            for inner in chars.by_ref() {
                span.push(inner);
                if inner == '"' {
                    break;
                }
            }
            out.push(span);
        } else if c.is_whitespace() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn quoted_inner(tok: &str) -> Option<&str> {
    tok.strip_prefix('"')?.strip_suffix('"')
}

/// Drop punctuation from the edges of a token, keeping the inside intact so
/// `co-op`, `log-ai-use` and `3.14` survive whole.
fn trim_token(tok: &str) -> String {
    if quoted_inner(tok).is_some() {
        return tok.to_string();
    }
    let chars: Vec<char> = tok.chars().collect();
    let mut start = 0;
    while start < chars.len() && !chars[start].is_alphanumeric() {
        start += 1;
    }
    let mut end = chars.len();
    while end > start && !chars[end - 1].is_alphanumeric() {
        end -= 1;
    }
    if start == end {
        return String::new();
    }
    let mut out = String::new();
    // A leading minus belongs to the number it introduces, so "-5" stays -5.
    if start > 0 && chars[start - 1] == '-' && chars[start].is_ascii_digit() {
        out.push('-');
    }
    out.extend(chars[start..end].iter());
    out
}

fn token_key(tok: &str) -> String {
    trim_token(tok).to_lowercase()
}

// ------------------------------------------------------------------- slots

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    Int,
    Float,
    Text,
    Name,
}

/// The value a token stands for, if it looks like one at all.
///
/// Int and float stay separate because they are separate ground identities: 42
/// is not 42.0, and a template that filled an int slot with a float would build
/// a concept the evaluator has never seen.
fn value_of(raw: &str) -> Option<(SlotKind, Concept)> {
    if let Some(inner) = quoted_inner(raw) {
        return Some((SlotKind::Text, Concept::text(inner)));
    }
    let trimmed = trim_token(raw);
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(int) = trimmed.parse::<i64>() {
        return Some((SlotKind::Int, Concept::int(int)));
    }
    // A decimal point or an exponent is what makes it a float rather than a
    // word; `parse` alone would accept "inf" and "nan".
    if (trimmed.contains('.') || trimmed.contains('e') || trimmed.contains('E'))
        && let Ok(float) = trimmed.parse::<f64>()
        && float.is_finite()
    {
        return Some((SlotKind::Float, Concept::float(float)));
    }
    if looks_like_name(&trimmed) {
        return Some((SlotKind::Name, Concept::named(&trimmed)));
    }
    None
}

/// Capitalised, at least two letters, and lowercase after the first.
///
/// `John` qualifies, `CANNOT` and `YOu` do not: shouting and typos are not
/// names. Sentence-initial capitalisation is not filtered out here, because the
/// steps-linkage check does that job better. `Can` at the start of "Can u
/// double 21" never appears in the reading, so it never becomes a slot.
fn looks_like_name(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_uppercase() {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    !rest.is_empty() && rest.iter().all(|c| c.is_alphabetic() && c.is_lowercase())
}

// --------------------------------------------------------------- prepared text

/// An utterance cleaned, tokenized, and scanned for values.
struct Prepared {
    toks: Vec<Tok>,
    /// Aligned with `toks`. `Some` where the token looks like a value.
    values: Vec<Option<(SlotKind, Concept)>>,
    /// Literal content words, deduplicated and sorted.
    content: Vec<String>,
    /// The utterance with every value token replaced by `{}`, for whole-string
    /// similarity on short inputs.
    skeleton: String,
}

impl Prepared {
    fn new(text: &str) -> Option<Prepared> {
        if text.trim().chars().count() < MIN_CHARS {
            return None;
        }
        let toks = align(text);
        if toks.is_empty() {
            return None;
        }
        let values: Vec<Option<(SlotKind, Concept)>> =
            toks.iter().map(|t| value_of(&t.raw)).collect();
        let content = dedup_sorted(
            toks.iter()
                .zip(values.iter())
                .filter(|(tok, value)| value.is_none() && !is_stopword(&tok.key))
                .map(|(tok, _)| tok.key.clone()),
        );
        let skeleton = skeleton_of(toks.iter().zip(values.iter()).map(
            |(tok, value)| match value {
                Some(_) => None,
                None => Some(tok.key.as_str()),
            },
        ));
        Some(Prepared {
            toks,
            values,
            content,
            skeleton,
        })
    }
}

/// Normalize the utterance, then walk the result back onto the original
/// spelling of each surviving token.
///
/// [`NativeEars::normalize`] only ever deletes tokens, lowercases, and trims
/// punctuation off the ends, so the cleaned tokens appear in the original in
/// the same order. A greedy scan therefore recovers the alignment exactly, and
/// reusing that function means filler and request prefixes are defined in one
/// place rather than two that can drift apart.
fn align(text: &str) -> Vec<Tok> {
    let raw = tokenize(text);
    let clean = NativeEars::normalize(text);
    let mut out = Vec::new();
    let mut next = 0usize;
    for cleaned in tokenize(&clean) {
        let key = token_key(&cleaned);
        if key.is_empty() {
            continue;
        }
        let found = raw[next..].iter().position(|r| token_key(r) == key);
        match found {
            Some(offset) => {
                let index = next + offset;
                out.push(Tok {
                    key,
                    raw: raw[index].clone(),
                });
                next = index + 1;
            }
            // Nothing to align to, which normalization should not produce.
            // Falling back to the cleaned spelling loses case, never a token.
            None => out.push(Tok { key, raw: cleaned }),
        }
    }
    out
}

fn dedup_sorted(words: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = words.collect();
    out.sort();
    out.dedup();
    out
}

fn skeleton_of<'a>(pieces: impl Iterator<Item = Option<&'a str>>) -> String {
    let parts: Vec<&str> = pieces.map(|p| p.unwrap_or("{}")).collect();
    parts.join(" ")
}

// ---------------------------------------------------------------- templates

#[derive(Debug, Clone)]
enum Piece {
    Word(String),
    Slot(usize),
}

/// One induced phrasing: the shape of an utterance, and the shape of the
/// reading it produced, sharing a set of slots.
#[derive(Debug, Clone)]
struct Template {
    pieces: Vec<Piece>,
    slots: Vec<SlotKind>,
    /// The hole each slot became in `steps`, numbered clear of any hole the
    /// model already used.
    holes: Vec<HoleId>,
    steps: Vec<Concept>,
    content: Vec<String>,
    skeleton: String,
    standing: f64,
    /// The source prior, kept so standing can be recomputed as evidence
    /// arrives without going back to the store.
    prior: f64,
    successes: u32,
    failures: u32,
    /// The stored pair this came from, when there is one.
    ///
    /// Carried so a reading can be credited or blamed after the turn. Without
    /// it the standing computed in `from_store` can only ever go up, since
    /// nothing downstream knows which pair to charge for a bad reading.
    pair: Option<i64>,
    /// Identity for deduplication: the same shape read the same way twice is
    /// one template, not two votes.
    key: String,
}

impl Template {
    fn build(utterance: &str, steps: &[Concept], standing: f64) -> Option<Template> {
        Template::from_pair(utterance, steps, standing, None)
    }

    fn from_pair(
        utterance: &str,
        steps: &[Concept],
        standing: f64,
        pair: Option<i64>,
    ) -> Option<Template> {
        if steps.is_empty() {
            return None;
        }
        let prepared = Prepared::new(utterance)?;
        let base = steps
            .iter()
            .filter_map(max_hole)
            .max()
            .map_or(0u32, |h| h.0.saturating_add(1));

        let mut slots: Vec<SlotKind> = Vec::new();
        let mut values: Vec<Concept> = Vec::new();
        let mut pieces: Vec<Piece> = Vec::new();
        for (tok, found) in prepared.toks.iter().zip(prepared.values.iter()) {
            match found {
                Some((kind, value)) if steps_mention(steps, value) => {
                    let index = match values.iter().position(|v| v == value) {
                        Some(index) => index,
                        None => {
                            values.push(value.clone());
                            slots.push(*kind);
                            values.len() - 1
                        }
                    };
                    pieces.push(Piece::Slot(index));
                }
                _ => pieces.push(Piece::Word(tok.key.clone())),
            }
        }

        let holes: Vec<HoleId> = (0..slots.len())
            .map(|i| HoleId(base.saturating_add(i as u32)))
            .collect();
        let mut abstracted = steps.to_vec();
        for (index, value) in values.iter().enumerate() {
            let hole = holes[index];
            abstracted = abstracted
                .iter()
                .map(|step| punch(step, value, hole).unwrap_or_else(|| step.clone()))
                .collect();
        }

        let content = dedup_sorted(pieces.iter().filter_map(|piece| match piece {
            Piece::Word(word) if !is_stopword(word) && value_of(word).is_none() => {
                Some(word.clone())
            }
            _ => None,
        }));
        let skeleton = skeleton_of(pieces.iter().map(|piece| match piece {
            Piece::Word(word) => Some(word.as_str()),
            Piece::Slot(_) => None,
        }));
        let key = format!(
            "{skeleton}\u{1}{}",
            abstracted
                .iter()
                .map(|s| s.content_id().short())
                .collect::<Vec<_>>()
                .join(",")
        );

        Some(Template {
            pieces,
            slots,
            holes,
            steps: abstracted,
            content,
            skeleton,
            standing,
            prior: standing,
            successes: 0,
            failures: 0,
            pair,
            key,
        })
    }

    /// Best reading this template can offer for an input, with the confidence
    /// to go with it. Exact first: if the shape is literally the same there is
    /// nothing for fuzzy matching to add.
    fn try_match(&self, input: &Prepared) -> Option<(Vec<Concept>, f64)> {
        if let Some(bindings) = self.exact(input) {
            return Some((self.fill(&bindings), EXACT_SIMILARITY * self.standing));
        }
        let (bindings, similarity) = self.fuzzy(input)?;
        Some((self.fill(&bindings), similarity * self.standing))
    }

    /// Token for token, with slots accepting any value of the right kind.
    fn exact(&self, input: &Prepared) -> Option<Bindings> {
        if self.pieces.len() != input.toks.len() {
            return None;
        }
        let mut bindings = Bindings::new();
        for (piece, (tok, found)) in self
            .pieces
            .iter()
            .zip(input.toks.iter().zip(input.values.iter()))
        {
            match piece {
                Piece::Word(word) => {
                    if tok.key != *word {
                        return None;
                    }
                }
                Piece::Slot(slot) => {
                    let (kind, value) = found.as_ref()?;
                    if self.slots.get(*slot)? != kind {
                        return None;
                    }
                    if !bindings.try_bind(*self.holes.get(*slot)?, value.clone()) {
                        return None;
                    }
                }
            }
        }
        Some(bindings)
    }

    /// Same content, different wrapping. This is what lets "could you please
    /// double 9" reach a template learned from "can u double 21 for me": the
    /// request prefix and the trailing politeness are not what the sentence is
    /// about.
    ///
    /// A template with no content words at all cannot go through here. There
    /// would be nothing to discriminate on, so it would match every input of
    /// the right shape.
    fn fuzzy(&self, input: &Prepared) -> Option<(Bindings, f64)> {
        if self.content.is_empty() || input.content.is_empty() {
            return None;
        }
        let short =
            input.toks.len() <= SHORT_INPUT_TOKENS && self.pieces.len() <= SHORT_INPUT_TOKENS;
        let overlap = content_overlap(&self.content, &input.content, short);
        if overlap < MIN_CONTENT_OVERLAP {
            return None;
        }

        // Every slot occurrence, in order, against every value the input
        // offers, in order. Counts must agree: a leftover value is something
        // the template cannot explain, and an unfilled slot is a hole in the
        // answer.
        let wanted: Vec<usize> = self
            .pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Slot(slot) => Some(*slot),
                Piece::Word(_) => None,
            })
            .collect();
        let supplied: Vec<&(SlotKind, Concept)> = input.values.iter().flatten().collect();
        if wanted.len() != supplied.len() {
            return None;
        }
        let mut bindings = Bindings::new();
        for (slot, (kind, value)) in wanted.iter().zip(supplied) {
            if self.slots.get(*slot)? != kind {
                return None;
            }
            if !bindings.try_bind(*self.holes.get(*slot)?, value.clone()) {
                return None;
            }
        }

        // On a short utterance the word sets are tiny, so whole-string
        // similarity is worth consulting as a second opinion. It can only
        // raise the score, never rescue an input that failed the content gate.
        let blended = if short {
            overlap.max(strsim::jaro_winkler(&self.skeleton, &input.skeleton))
        } else {
            overlap
        };
        Some((bindings, FUZZY_FLOOR + FUZZY_SPAN * blended))
    }

    fn fill(&self, bindings: &Bindings) -> Vec<Concept> {
        self.steps
            .iter()
            .map(|step| substitute(step, bindings))
            .collect()
    }
}

/// Soft Jaccard over content words.
///
/// Symmetric on purpose. Counting only how much of the template the input
/// covers would let "double the pain of 5" match a template about doubling,
/// because the extra content word costs nothing. Here it costs, and that is
/// what stops the template explaining a sentence it does not explain.
fn content_overlap(template: &[String], input: &[String], fuzzy: bool) -> f64 {
    let mut used = vec![false; input.len()];
    let mut matched = 0usize;
    // Exact pairings first, so a fuzzy pairing never steals a word that had a
    // real match waiting.
    for word in template {
        if let Some(index) = input
            .iter()
            .enumerate()
            .position(|(i, other)| !used[i] && other == word)
        {
            used[index] = true;
            matched += 1;
        }
    }
    if fuzzy {
        for word in template {
            if let Some(index) = input
                .iter()
                .enumerate()
                .position(|(i, other)| !used[i] && near_word(word, other))
            {
                used[index] = true;
                matched += 1;
            }
        }
    }
    let union = template.len() + input.len() - matched;
    if union == 0 {
        return 0.0;
    }
    matched as f64 / union as f64
}

/// Two spellings of the same word.
///
/// The shared first character is not decoration: without it "triple" and
/// "double" score high enough on Jaro-Winkler to be confused, and those are
/// opposite instructions. Typos almost never land on the first letter, so the
/// guard costs nothing real and rules out the failure that matters.
fn near_word(a: &str, b: &str) -> bool {
    if a.chars().count() < 4 || b.chars().count() < 4 {
        return false;
    }
    if a.chars().next() != b.chars().next() {
        return false;
    }
    strsim::jaro_winkler(a, b) >= WORD_SIMILARITY
}

fn steps_mention(steps: &[Concept], value: &Concept) -> bool {
    steps.iter().any(|step| pre_order(step).any(|n| n == value))
}

/// Replace every occurrence of `value` with `hole`, reusing untouched
/// subtrees.
///
/// `None` means nothing under here changed, which is what lets the caller hand
/// back the original `Arc` instead of rebuilding an identical tree. Concepts
/// are shared by design and copying them wholesale defeats that.
fn punch(term: &Concept, value: &Concept, hole: HoleId) -> Option<Concept> {
    if term == value {
        return Some(Concept::Hole(hole));
    }
    let Concept::Compound { head, args } = term else {
        return None;
    };
    let new_head = punch(head, value, hole);
    let mut new_args: Option<Vec<Concept>> = None;
    for (index, arg) in args.iter().enumerate() {
        if let Some(replaced) = punch(arg, value, hole) {
            new_args.get_or_insert_with(|| args.to_vec())[index] = replaced;
        }
    }
    if new_head.is_none() && new_args.is_none() {
        return None;
    }
    Some(Concept::Compound {
        head: match new_head {
            Some(built) => Arc::new(built),
            None => head.clone(),
        },
        args: match new_args {
            Some(built) => Arc::from(built),
            None => args.clone(),
        },
    })
}

// ------------------------------------------------------------------- index

/// Templates induced from worked examples, matched against new utterances.
///
/// Built once per brain from the pair store and grown in memory as the model
/// produces readings that work. Lookup is by content word, so adding templates
/// does not make recognition linearly slower in the size of the brain.
#[derive(Debug, Clone, Default)]
pub struct PhrasingIndex {
    templates: Vec<Template>,
    by_word: HashMap<String, Vec<usize>>,
    /// Templates with no content words. They can only ever match exactly, but
    /// they still have to be reachable.
    wordless: Vec<usize>,
    keys: HashSet<String>,
}

impl PhrasingIndex {
    pub fn new() -> Self {
        PhrasingIndex::default()
    }

    /// Build from everything the store has learned, best standing first.
    ///
    /// Standing comes from the pair, so a phrasing the user confirmed
    /// generalizes further than one the model guessed, and one that has failed
    /// repeatedly may no longer clear the threshold at all.
    pub fn from_store(store: &Store) -> spoon_store::Result<PhrasingIndex> {
        let mut index = PhrasingIndex::new();
        for pair in store.all_pairs(MAX_PAIRS)? {
            let standing = pair.standing();
            if let Some(mut template) =
                Template::from_pair(&pair.utterance, &pair.steps, standing, Some(pair.id))
            {
                template.prior = pair.source.prior();
                template.successes = pair.successes;
                template.failures = pair.failures;
                index.push(template);
            }
        }
        Ok(index)
    }

    /// Take a worked example: this utterance produced these steps.
    ///
    /// Silently ignores anything it cannot abstract (an empty utterance, an
    /// empty reading, text that normalizes away to nothing). A caller learning
    /// from a stream of turns should not have to pre-screen them, and refusing
    /// loudly would only invite the caller to ignore the error.
    ///
    /// Templates learned this way carry the model prior, which is the
    /// conservative reading of an unattributed example. [`Self::from_store`]
    /// carries the real source and the real outcome history.
    pub fn learn(&mut self, utterance: &str, steps: &[Concept]) {
        if let Some(template) = Template::build(utterance, steps, PairSource::Model.prior()) {
            self.push(template);
        }
    }

    /// Note how a reading built from a stored pair turned out.
    ///
    /// The store keeps the durable count; this keeps the index in step within
    /// the run. Without it a phrasing demoted on turn three keeps winning
    /// until the next restart, which for a long run means it never stops.
    pub fn record(&mut self, pair: i64, worked: bool) {
        for template in &mut self.templates {
            if template.pair != Some(pair) {
                continue;
            }
            if worked {
                template.successes = template.successes.saturating_add(1);
            } else {
                template.failures = template.failures.saturating_add(1);
            }
            template.standing = spoon_store::pairs::standing(
                template.prior,
                template.successes,
                template.failures,
            );
        }
    }

    /// The best reading for this utterance, or `None` when nothing is confident
    /// enough to be worth the risk of being wrong.
    pub fn recognize(&self, text: &str) -> Option<(Vec<Concept>, f64)> {
        self.recognize_from(text)
            .map(|(steps, confidence, _)| (steps, confidence))
    }

    /// The best reading, with the stored pair it came from.
    ///
    /// The caller needs the id to say afterwards whether the reading worked.
    pub fn recognize_from(&self, text: &str) -> Option<(Vec<Concept>, f64, Option<i64>)> {
        let input = Prepared::new(text)?;
        let mut best: Option<(Vec<Concept>, f64, Option<i64>)> = None;
        for index in self.candidates(&input) {
            let Some(template) = self.templates.get(index) else {
                continue;
            };
            let Some((steps, confidence)) = template.try_match(&input) else {
                continue;
            };
            if confidence < MIN_CONFIDENCE {
                continue;
            }
            if best.as_ref().is_none_or(|(_, top, _)| confidence > *top) {
                best = Some((steps, confidence, template.pair));
            }
        }
        best
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    fn push(&mut self, template: Template) {
        if !self.keys.insert(template.key.clone()) {
            return;
        }
        let index = self.templates.len();
        if template.content.is_empty() {
            self.wordless.push(index);
        }
        for word in &template.content {
            self.by_word.entry(word.clone()).or_default().push(index);
        }
        self.templates.push(template);
    }

    /// Templates worth trying against this input.
    ///
    /// A short utterance is checked against everything, because that is exactly
    /// where a typo can leave it with no content word in common with the
    /// template that should match it. Long utterances go through the word
    /// index: there, fuzzy word matching is off anyway, so a template sharing
    /// no content word could never clear the overlap gate.
    fn candidates(&self, input: &Prepared) -> Vec<usize> {
        if input.toks.len() <= SHORT_INPUT_TOKENS {
            return (0..self.templates.len()).collect();
        }
        let mut found: Vec<usize> = self.wordless.clone();
        for word in &input.content {
            if let Some(ids) = self.by_word.get(word) {
                found.extend(ids.iter().copied());
            }
        }
        found.sort_unstable();
        found.dedup();
        found
    }
}
