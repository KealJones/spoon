//! Text normalization: unicode cleanup, slang expansion, elongation squash,
//! typo repair, filler removal, sentence splitting.

use std::collections::HashSet;

use crate::ears::lexicon::{Lexicon, should_protect};
use crate::ears::values::{spot_values, Spotted};

// ---- public output type ----

#[derive(Debug, Clone)]
pub struct Normalized {
    /// Individual SCE-destined sentences, each ending with ., !, or ?
    pub sentences: Vec<String>,
    /// Tokens that were not in the lexicon after all repairs.
    pub unknown_words: Vec<String>,
    /// Protected spans in the *original* text (for reference).
    pub protected: Vec<Spotted>,
}

// ---- pipeline entry point ----

pub fn normalize(text: &str, lexicon: &Lexicon) -> Normalized {
    // 1. Unicode cleanup
    let cleaned = unicode_cleanup(text);

    // 2. Apply slang replacements (multi-word first, already sorted by length desc in Lexicon)
    let expanded = expand_slang(&cleaned, &lexicon.replacements);

    // 3. Elongation squash (3+ same consecutive chars -> 1)
    let squashed = squash_elongation(&expanded);

    // 4. Spot values before case-folding (so arithmetic is preserved)
    let protected = spot_values(&squashed);

    // 5. Lowercase everything except: tokens inside protected spans OR tokens recognized as names
    let lowered = lowercase_except_names(&squashed, lexicon, &protected);

    // 6. Strip fillers at sentence/clause starts
    let defilled = strip_fillers(&lowered, &lexicon.fillers);

    // 7. Tokenize and typo-repair non-protected words
    let (repaired, unknown_words) = repair_tokens(&defilled, lexicon, &protected);

    // 8. Re-spot values after repair (some numbers/arith may have changed position)
    let protected_final = spot_values(&repaired);

    // 9. Split into sentences
    let sentences = split_sentences(&repaired, &protected_final);

    Normalized { sentences, unknown_words, protected: protected_final }
}

// ---- unicode cleanup ----

fn unicode_cleanup(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',  // curly single quotes
            '\u{201C}' | '\u{201D}' => '"',   // curly double quotes
            '\u{2014}' | '\u{2013}' => '-',   // em-dash, en-dash
            '\u{2026}' => '.', // ellipsis
            '\t' => ' ',
            '\r' => ' ',
            _ => c,
        })
        .collect::<String>()
        // collapse multiple spaces
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- slang expansion ----

fn expand_slang(text: &str, replacements: &[(String, String)]) -> String {
    let mut result = text.to_lowercase();
    for (from, to) in replacements {
        if from.is_empty() {
            continue;
        }
        if to.is_empty() {
            // Filler - just replace with space
            result = replace_word_boundary(&result, from, " ");
        } else {
            result = replace_word_boundary(&result, from, to);
        }
        // Collapse whitespace after each replacement
        result = result.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    result
}

/// Replace `needle` in `haystack` only at word boundaries (start/end of string or
/// surrounded by non-alphanumeric characters), case-insensitively.
fn replace_word_boundary(haystack: &str, needle: &str, replacement: &str) -> String {
    // For multi-word needles, just do a plain substring replacement
    // (slang dict was designed for direct substring matching)
    let lower = haystack.to_lowercase();
    let lower_needle = needle.to_lowercase();
    let mut result = String::with_capacity(haystack.len());
    let mut rest = lower.as_str();
    let mut offset = 0;

    while let Some(pos) = rest.find(lower_needle.as_str()) {
        let abs_pos = offset + pos;
        // Check word boundaries
        let before_ok = if abs_pos == 0 {
            true
        } else {
            let prev = haystack[..abs_pos].chars().last().unwrap_or(' ');
            !prev.is_alphanumeric() && prev != '\''
        };
        let after_pos = abs_pos + needle.len();
        let after_ok = if after_pos >= haystack.len() {
            true
        } else {
            let next = haystack[after_pos..].chars().next().unwrap_or(' ');
            !next.is_alphanumeric() && next != '\''
        };

        if before_ok && after_ok {
            result.push_str(&haystack[offset..abs_pos]);
            result.push_str(replacement);
            offset = after_pos;
            rest = &lower[offset..];
        } else {
            // not a word boundary; advance past this occurrence
            result.push_str(&haystack[offset..abs_pos + 1]);
            offset = abs_pos + 1;
            rest = &lower[offset..];
        }
    }
    result.push_str(&haystack[offset..]);
    result
}

// ---- elongation squash ----

/// Collapse runs of 3+ identical characters down to 1.
fn squash_elongation(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut count = 1;
        while i + count < chars.len() && chars[i + count] == c {
            count += 1;
        }
        // Keep at most 1 if 3+, otherwise keep all
        let keep = if count >= 3 { 1 } else { count };
        for _ in 0..keep {
            out.push(c);
        }
        i += count;
    }
    out
}

// ---- lowercase except names ----

fn lowercase_except_names(text: &str, lexicon: &Lexicon, protected: &[Spotted]) -> String {
    let mut out = String::with_capacity(text.len());
    for (byte_pos, word) in word_positions(text) {
        // Check if this span is inside a protected region
        let word_end = byte_pos + word.len();
        let in_protected = protected.iter().any(|s| byte_pos >= s.start && byte_pos < s.end);

        if in_protected {
            out.push_str(word);
        } else if word.chars().next().is_some_and(|c| c.is_uppercase()) {
            // If it's a recognized name, keep capitalized; else lowercase
            if lexicon.is_name(word) || lexicon.canonical_name(word).is_some() {
                // Use the canonical capitalized form
                let canonical = lexicon.canonical_name(word).unwrap_or(word);
                out.push_str(canonical);
            } else {
                // Not a known name - lowercase it
                out.push_str(&word.to_lowercase());
            }
        } else {
            out.push_str(word);
        }
        out.push(' ');
        let _ = word_end;
    }
    out.trim_end().to_string()
}

/// Yields (byte_start, token) for word-like tokens in `text`,
/// preserving inter-word content (spaces, punctuation) as separate tokens.
fn word_positions(text: &str) -> impl Iterator<Item = (usize, &str)> {
    // Split by whitespace only; punctuation attached to words is included.
    // This preserves "dog." and "girl?" as units.
    text.split(' ')
        .scan(0usize, |offset, word| {
            let start = *offset;
            *offset += word.len() + 1; // +1 for the space
            Some((start, word))
        })
        .filter(|(_, w)| !w.is_empty())
}

// ---- filler stripping ----

/// Strip fillers only at the start of each "clause" (start of text, or after a sentence terminator).
fn strip_fillers(text: &str, fillers: &[String]) -> String {
    // Sort fillers by length desc for greedy matching
    let mut sorted: Vec<&str> = fillers.iter().map(|s| s.as_str()).collect();
    sorted.sort_by(|a, b| b.len().cmp(&a.len()));

    let mut result = text.to_string();
    let mut changed = true;
    while changed {
        changed = false;
        // Strip fillers from the very start
        for filler in &sorted {
            let lower = result.to_lowercase();
            let lower_filler = filler.to_lowercase();
            if lower.starts_with(lower_filler.as_str()) {
                let rest = result[filler.len()..].trim_start();
                result = rest.to_string();
                changed = true;
                break;
            }
        }
    }

    // Strip after sentence terminators (. ? !)
    let mut out = String::new();
    let mut after_term = false;
    let mut buf = String::new();

    for ch in result.chars() {
        if after_term {
            if ch == ' ' || ch == '\t' {
                buf.push(ch);
            } else {
                // Tentatively accumulate the next clause start to strip fillers
                buf.push(ch);
                // Check if buf (lowercased, trimmed) starts with a filler
                let lower_buf = buf.trim_start().to_lowercase();
                let found = sorted.iter().any(|f| {
                    let fl = f.to_lowercase();
                    lower_buf.starts_with(fl.as_str())
                        && (lower_buf.len() == fl.len()
                            || lower_buf.as_bytes().get(fl.len()).is_some_and(|&b| {
                                b == b' ' || b == b'\t'
                            }))
                });
                if !found {
                    out.push_str(&buf);
                    buf.clear();
                    after_term = false;
                }
            }
        } else if ch == '.' || ch == '?' || ch == '!' {
            out.push(ch);
            after_term = true;
            buf.clear();
        } else {
            out.push(ch);
        }
    }
    out.push_str(&buf);
    out
}

// ---- typo repair ----

fn repair_tokens(text: &str, lexicon: &Lexicon, protected: &[Spotted]) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut unknown: Vec<String> = vec![];
    let mut seen_unknown: HashSet<String> = HashSet::new();
    let mut byte_offset = 0usize;

    let words: Vec<&str> = text.split(' ').collect();
    for (i, word) in words.iter().enumerate() {
        let word_start = byte_offset;
        byte_offset += word.len() + 1; // +1 for space

        // Strip trailing punctuation for lookup
        let stripped = word.trim_end_matches(|c: char| matches!(c, '.' | '?' | '!' | ',' | ';' | ':'));
        let punct_suffix = &word[stripped.len()..];

        // Is this word inside a protected span?
        let in_protected = protected.iter().any(|s| word_start >= s.start && word_start < s.end);

        if in_protected || stripped.is_empty() || should_protect(stripped) || lexicon.is_known(stripped) {
            if i > 0 { out.push(' '); }
            out.push_str(word);
        } else {
            // Try typo repair
            let candidates = lexicon.candidates(stripped);
            if let Some((best, _)) = candidates.first() {
                if i > 0 { out.push(' '); }
                // Check if best is a name - capitalize it
                if lexicon.is_name(best) {
                    out.push_str(best); // already capitalized
                } else {
                    out.push_str(best);
                }
                out.push_str(punct_suffix);
            } else {
                if i > 0 { out.push(' '); }
                out.push_str(word);
                let unk = stripped.to_lowercase();
                if !unk.is_empty() && seen_unknown.insert(unk.clone()) {
                    unknown.push(unk);
                }
            }
        }
        let _ = word_start;
    }

    (out, unknown)
}

// ---- sentence splitting ----

/// Split text into sentences at `.`, `?`, `!` that are NOT inside protected spans.
/// Each sentence retains its terminator. Sentences without a terminator get a `.` appended.
pub fn split_sentences(text: &str, protected: &[Spotted]) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut sentences: Vec<String> = vec![];
    let mut current = String::new();
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i] as char;
        let in_protected = protected.iter().any(|s| i >= s.start && i < s.end);

        if !in_protected && (c == '.' || c == '?' || c == '!') {
            current.push(c);
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
            current.clear();
        } else {
            current.push(c);
        }
        i += 1;
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        // Add a period if no terminator
        if trimmed.ends_with('.') || trimmed.ends_with('?') || trimmed.ends_with('!') {
            sentences.push(trimmed);
        } else {
            sentences.push(format!("{}.", trimmed));
        }
    }

    // Filter out single-char or obviously empty sentences
    sentences.retain(|s| s.len() > 1);
    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_cleanup_works() {
        assert_eq!(unicode_cleanup("\u{201C}hi\u{201D}"), "\"hi\"");
    }

    #[test]
    fn elongation_squash_works() {
        assert_eq!(squash_elongation("yoooo"), "yo");
        assert_eq!(squash_elongation("sooo"), "so");
        assert_eq!(squash_elongation("cool"), "cool"); // only 2 o's, unchanged
    }

    #[test]
    fn sentence_split_basic() {
        let spots = vec![];
        let sentences = split_sentences("John owns a dog. Who is John?", &spots);
        assert_eq!(sentences.len(), 2);
        assert_eq!(sentences[0], "John owns a dog.");
        assert_eq!(sentences[1], "Who is John?");
    }

    #[test]
    fn sentence_split_no_terminator() {
        let spots = vec![];
        let sentences = split_sentences("John owns a dog", &spots);
        assert_eq!(sentences.len(), 1);
        assert!(sentences[0].ends_with('.'));
    }
}
