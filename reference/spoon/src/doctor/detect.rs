//! Symptom detectors: classify WHY a turn failed.
//!
//! Each detector takes raw episode text and returns an `Option<detail>` (or bool).
//! Detectors are independent and pure; no I/O, no state.

use spoon_core::types::{Episode, Move};

// ---- public types ----------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SymptomResult {
    /// Primary cluster key.
    pub primary: String,
    /// Additional symptoms that also fired.
    pub secondaries: Vec<String>,
    /// Human-readable detail (e.g. "movie -> move").
    pub details: String,
}

/// Run all detectors and return the most-specific match as primary,
/// with remaining matches as secondaries.
///
/// Priority (most specific first): normalizer_inversion > typo_repair_damage >
/// discourse_lead_in > unparsed
pub fn detect_symptom(user_text: &str, sce: &str) -> SymptomResult {
    let mut hits: Vec<(u8, &'static str, String)> = Vec::new();

    if let Some(detail) = detect_normalizer_inversion(sce) {
        hits.push((0, "normalizer_inversion", detail));
    }
    if let Some(detail) = detect_typo_repair_damage(user_text, sce) {
        hits.push((1, "typo_repair_damage", detail));
    }
    if detect_discourse_lead_in(sce) {
        hits.push((2, "discourse_lead_in", String::new()));
    }

    if hits.is_empty() {
        return SymptomResult {
            primary: "unparsed".into(),
            secondaries: vec![],
            details: String::new(),
        };
    }

    hits.sort_by_key(|(p, _, _)| *p);
    let (_, primary_name, primary_detail) = hits.remove(0);
    let secondaries = hits.into_iter().map(|(_, name, _)| name.to_string()).collect();

    SymptomResult { primary: primary_name.into(), secondaries, details: primary_detail }
}

/// Detect: SCE addresses `Assistant` as the actor and puts `User` in object
/// position (normalizer got subject/object backwards).
///
/// Covers `Assistant, get User all the titles from ...` and
/// `could Assistant summarize ... for User?`
pub fn detect_normalizer_inversion(sce: &str) -> Option<String> {
    let l = sce.to_lowercase();
    let asst_pos = l.find("assistant")?;
    let user_pos = l.find("user")?;
    if user_pos <= asst_pos {
        return None;
    }
    let asst_in_subject = l.starts_with("assistant") || l.contains("assistant,");
    let user_in_object = l.contains(" for user")
        || l.contains(" to user")
        || {
            let before = &l[..user_pos];
            let last_word = before.split_whitespace().next_back().unwrap_or("");
            !matches!(
                last_word.trim_end_matches(','),
                "the" | "a" | "an" | "of" | "with" | "from" | "by" | "and" | "or"
            ) && l[user_pos..].contains(' ')
        };
    if asst_in_subject || user_in_object {
        Some("assistant in subject, user in object".into())
    } else {
        None
    }
}

/// Detect: a token in user_text was replaced by a close (DL=1) but different
/// token in the SCE, where the original was a real content word.
///
/// This is local typo repair (lexicon.rs + common_words.txt), not the LLM.
/// Conservative: only fires when the original is 4+ chars, all-alpha,
/// not a function word, and the edit distance to the SCE token is exactly 1.
pub fn detect_typo_repair_damage(user_text: &str, sce: &str) -> Option<String> {
    let user_tokens = content_tokens(user_text);
    let sce_tokens = content_tokens(sce);

    let mut damaged: Vec<(String, String)> = Vec::new();

    for u in &user_tokens {
        if sce_tokens.iter().any(|s| s == u) {
            continue;
        }
        for s in &sce_tokens {
            if strsim::damerau_levenshtein(u.as_str(), s.as_str()) == 1 {
                damaged.push((u.clone(), s.clone()));
                break;
            }
        }
    }

    if damaged.is_empty() {
        None
    } else {
        let pairs: Vec<String> = damaged.iter().map(|(a, b)| format!("{a} -> {b}")).collect();
        Some(pairs.join(", "))
    }
}

/// Detect: a conversational filler prefix survived into the SCE.
pub fn detect_discourse_lead_in(sce: &str) -> bool {
    let l = sce.to_lowercase();
    const PREFIXES: &[&str] = &[
        "not much,",
        "not much.",
        "well,",
        "well.",
        "oh,",
        "oh.",
        "hey,",
        "sure,",
        "yeah,",
        "yes,",
        "no,",
        "hmm,",
        "okay,",
        "ok,",
        "actually,",
        "honestly,",
        "honestly ",
    ];
    PREFIXES.iter().any(|p| l.starts_with(p) || l.contains(&format!(" {p}")))
}

/// Extract lowercase alphabetic content tokens (4+ chars, not function words,
/// not SCE reserved names).
pub fn content_tokens(text: &str) -> Vec<String> {
    const SCE_RESERVED: &[&str] = &["user", "assistant", "spoon"];
    text.split_whitespace()
        .map(|t| t.to_lowercase())
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_alphabetic()).to_string())
        .filter(|t| {
            t.len() >= 4
                && t.chars().all(|c| c.is_ascii_alphabetic())
                && !is_function_word(t.as_str())
                && !SCE_RESERVED.contains(&t.as_str())
        })
        .collect()
}

pub fn is_honest_unknown(reply: &str) -> bool {
    let l = reply.to_lowercase();
    l.contains("don't know")
        || l.contains("dont know")
        || l.contains("not sure")
        || l.contains("no information")
        || l.contains("can't say")
        || l.contains("cant say")
        || l.contains("don't have")
        || l.contains("dont have")
        || l.contains("couldn't find")
        || l.contains("couldnt find")
        || l.contains("i don't")
        || l.contains("i dont")
}

pub fn has_clarify_move(ep: &Episode) -> bool {
    ep.response.moves.iter().any(|m| matches!(m, Move::Clarify { .. }))
}

pub fn is_runtime_error(ep: &Episode) -> bool {
    let l = ep.reply_text.to_lowercase();
    l.contains("something went wrong")
        || l.contains("something broke")
        || ep.response.moves.iter().any(|m| matches!(m, Move::Error { .. }))
}

pub fn is_function_word(w: &str) -> bool {
    matches!(
        w,
        "a" | "about"
            | "am"
            | "an"
            | "and"
            | "are"
            | "at"
            | "be"
            | "been"
            | "by"
            | "can"
            | "cant"
            | "could"
            | "did"
            | "do"
            | "does"
            | "dont"
            | "doesnt"
            | "for"
            | "from"
            | "had"
            | "has"
            | "have"
            | "he"
            | "hello"
            | "her"
            | "hey"
            | "hi"
            | "him"
            | "his"
            | "how"
            | "i"
            | "id"
            | "if"
            | "ill"
            | "im"
            | "in"
            | "into"
            | "is"
            | "it"
            | "its"
            | "ive"
            | "just"
            | "may"
            | "me"
            | "might"
            | "must"
            | "my"
            | "no"
            | "not"
            | "of"
            | "ok"
            | "okay"
            | "on"
            | "or"
            | "our"
            | "out"
            | "please"
            | "shall"
            | "she"
            | "should"
            | "so"
            | "than"
            | "that"
            | "the"
            | "their"
            | "them"
            | "then"
            | "there"
            | "they"
            | "theyre"
            | "to"
            | "up"
            | "us"
            | "was"
            | "we"
            | "were"
            | "weve"
            | "what"
            | "when"
            | "where"
            | "who"
            | "why"
            | "will"
            | "with"
            | "wont"
            | "would"
            | "yes"
            | "you"
            | "your"
    )
}
