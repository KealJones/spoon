//! Phrasing induction from (utterance, SCE) pairs.
//! Extracts slotted templates by aligning shared values between messy and SCE sides.

use std::collections::HashMap;

use spoon_core::types::episode::Pair;

use crate::ears::lexicon::Lexicon;
use crate::ears::phrasings::{Phrasing, Tok};
use crate::ears::values::{spot_values, SlotKind, Spotted};

/// Induce slotted `Phrasing`s from a batch of (utterance, sce) pairs.
/// The result can be added to a `PhrasingStore` via `add_many`.
pub fn induce_phrasings(pairs: &[Pair]) -> Vec<Phrasing> {
    let mut out = vec![];
    let mut template_counts: HashMap<String, usize> = HashMap::new();

    for pair in pairs {
        if pair.credit < 0 {
            continue; // skip discredited pairs
        }
        let utt_spots = spot_values(&pair.utterance);
        let sce_spots = spot_values(&pair.sce);

        // Find shared values: numeric / arithmetic values present in both sides
        let shared = find_shared_values(&pair.utterance, &utt_spots, &pair.sce, &sce_spots);

        if shared.is_empty() {
            // No slots possible - store as a literal phrasing
            let pattern: Vec<Tok> = tokenize_raw(&pair.utterance)
                .into_iter()
                .map(Tok::Word)
                .collect();
            let key = render_pattern_key(&pattern);
            *template_counts.entry(key.clone()).or_default() += 1;
            out.push(Phrasing {
                pattern,
                sce: pair.sce.clone(),
                source: pair.source.clone(),
                credit: pair.credit,
            });
            continue;
        }

        // Replace shared values with slots on both sides
        let (utt_pattern, sce_template) = slot_replace(
            &pair.utterance,
            &utt_spots,
            &pair.sce,
            &sce_spots,
            &shared,
        );

        let key = render_pattern_key(&utt_pattern);
        *template_counts.entry(key).or_default() += 1;
        out.push(Phrasing {
            pattern: utt_pattern,
            sce: sce_template,
            source: pair.source.clone(),
            credit: pair.credit,
        });
    }

    out
}

/// Produce `(unknown_word, known_word)` pairs from co-occurrence patterns in pairs.
/// If an unknown word consistently appears alongside a known SCE verb, suggest that mapping.
pub fn induce_word_map(pairs: &[Pair], lexicon: &Lexicon) -> Vec<(String, String)> {
    // Map: unknown_word -> most common SCE verb it co-occurs with
    let mut cooccur: HashMap<String, HashMap<String, usize>> = HashMap::new();

    for pair in pairs {
        if pair.credit < 0 {
            continue;
        }
        let utt_tokens = tokenize_raw(&pair.utterance);
        let sce_verbs = extract_verbs_from_sce(&pair.sce, lexicon);

        for token in &utt_tokens {
            if lexicon.is_known(token) {
                continue; // only unknown words
            }
            for verb in &sce_verbs {
                *cooccur
                    .entry(token.clone())
                    .or_default()
                    .entry(verb.clone())
                    .or_default() += 1;
            }
        }
    }

    let mut result = vec![];
    for (unknown, verb_counts) in cooccur {
        if let Some((best_verb, &count)) = verb_counts.iter().max_by_key(|(_, c)| *c) {
            if count >= 2 {
                result.push((unknown, best_verb.clone()));
            }
        }
    }
    result
}

// ---- shared value detection ----

struct SharedValue {
    utt_start: usize,
    utt_end: usize,
    utt_text: String,
    sce_start: usize,
    sce_end: usize,
    kind: SlotKind,
}

fn find_shared_values(
    _utt: &str,
    utt_spots: &[Spotted],
    _sce: &str,
    sce_spots: &[Spotted],
) -> Vec<SharedValue> {
    let mut shared = vec![];
    for us in utt_spots {
        if !matches!(us.kind, SlotKind::Number | SlotKind::Arith | SlotKind::Quoted | SlotKind::Path) {
            continue;
        }
        let utt_val = normalize_value_text(&us.text);

        // Direct match: same text on both sides
        let mut found = false;
        for ss in sce_spots {
            if !matches!(ss.kind, SlotKind::Number | SlotKind::Arith | SlotKind::Quoted | SlotKind::Path) {
                continue;
            }
            let sce_val = normalize_value_text(&ss.text);
            if utt_val == sce_val {
                shared.push(SharedValue {
                    utt_start: us.start,
                    utt_end: us.end,
                    utt_text: us.text.clone(),
                    sce_start: ss.start,
                    sce_end: ss.end,
                    kind: us.kind.clone(),
                });
                found = true;
                break;
            }
        }
        if found { continue; }

        // Indirect match: utterance Number appears within an SCE Arith span.
        // E.g. utterance "4" appears in SCE arith "4 * 5".
        // We map the utterance Number slot to its position WITHIN the arith expression.
        if matches!(us.kind, SlotKind::Number | SlotKind::Arith) {
            for ss in sce_spots {
                if ss.kind != SlotKind::Arith {
                    continue;
                }
                // Find the utterance value as a word-boundary match within the arith text.
                if let Some(inner_offset) = find_number_in_arith(&utt_val, &ss.text) {
                    let sce_start = ss.start + inner_offset;
                    let sce_end = sce_start + utt_val.len();
                    shared.push(SharedValue {
                        utt_start: us.start,
                        utt_end: us.end,
                        utt_text: us.text.clone(),
                        sce_start,
                        sce_end,
                        kind: SlotKind::Number,
                    });
                    break;
                }
            }
        }
    }
    shared
}

/// Find `num_text` as a token within `arith_text`, returning the byte offset.
/// Only matches whole tokens (surrounded by non-digit/non-dot chars or boundaries).
fn find_number_in_arith(num_text: &str, arith_text: &str) -> Option<usize> {
    let mut search = arith_text;
    let mut base_offset = 0;
    while let Some(pos) = search.find(num_text) {
        let start = base_offset + pos;
        let end = start + num_text.len();
        // Check word boundaries
        let before_ok = if start == 0 { true } else {
            let c = arith_text[..start].chars().last().unwrap_or(' ');
            !c.is_ascii_digit() && c != '.'
        };
        let after_ok = if end >= arith_text.len() { true } else {
            let c = arith_text[end..].chars().next().unwrap_or(' ');
            !c.is_ascii_digit() && c != '.'
        };
        if before_ok && after_ok {
            return Some(start);
        }
        base_offset += pos + 1;
        if base_offset >= arith_text.len() { break; }
        search = &arith_text[base_offset..];
    }
    None
}

fn normalize_value_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---- slot replacement ----

fn slot_replace(
    utt: &str,
    _utt_spots: &[Spotted],
    sce: &str,
    _sce_spots: &[Spotted],
    shared: &[SharedValue],
) -> (Vec<Tok>, String) {
    // Build utterance pattern
    let utt_pattern = build_slotted_pattern(utt, shared, true);
    // Build SCE template
    let sce_template = build_slotted_sce(sce, shared);
    (utt_pattern, sce_template)
}

fn build_slotted_pattern(text: &str, shared: &[SharedValue], is_utt: bool) -> Vec<Tok> {
    // Collect replacement intervals sorted by start
    let mut intervals: Vec<(usize, usize, String, SlotKind)> = shared
        .iter()
        .enumerate()
        .map(|(i, sv)| {
            let (start, end) = if is_utt {
                (sv.utt_start, sv.utt_end)
            } else {
                (sv.sce_start, sv.sce_end)
            };
            (start, end, format!("slot_{}", i), sv.kind.clone())
        })
        .collect();
    intervals.sort_by_key(|(s, _, _, _)| *s);

    let mut tokens: Vec<Tok> = vec![];
    let mut cursor = 0;

    for (start, end, slot_name, kind) in &intervals {
        if *start > cursor {
            let literal = &text[cursor..*start];
            for word in tokenize_raw(literal) {
                if !word.is_empty() {
                    tokens.push(Tok::Word(word));
                }
            }
        }
        tokens.push(Tok::Slot { name: slot_name.clone(), kind: kind.clone() });
        cursor = *end;
    }
    if cursor < text.len() {
        for word in tokenize_raw(&text[cursor..]) {
            if !word.is_empty() {
                tokens.push(Tok::Word(word));
            }
        }
    }

    tokens
}

fn build_slotted_sce(sce: &str, shared: &[SharedValue]) -> String {
    let mut intervals: Vec<(usize, usize, String)> = shared
        .iter()
        .enumerate()
        .map(|(i, sv)| (sv.sce_start, sv.sce_end, format!("slot_{}", i)))
        .collect();
    intervals.sort_by_key(|(s, _, _)| *s);

    let mut result = String::new();
    let mut cursor = 0;

    for (start, end, slot_name) in &intervals {
        if *start > cursor {
            result.push_str(&sce[cursor..*start]);
        }
        result.push('{');
        result.push_str(slot_name);
        result.push('}');
        cursor = *end;
    }
    if cursor < sce.len() {
        result.push_str(&sce[cursor..]);
    }
    result
}

// ---- helpers ----

fn tokenize_raw(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace())
        .map(|t| t.trim_matches(|c: char| matches!(c, '.' | '?' | '!' | ',' | ';' | ':')).to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

fn render_pattern_key(pattern: &[Tok]) -> String {
    pattern
        .iter()
        .map(|t| match t {
            Tok::Word(w) => w.clone(),
            Tok::Slot { name, .. } => format!("{{{}}}", name),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_verbs_from_sce(sce: &str, lexicon: &Lexicon) -> Vec<String> {
    tokenize_raw(sce)
        .into_iter()
        .filter(|t| lexicon.is_known(t) && !t.is_empty())
        .collect()
}
