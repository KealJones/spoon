//! Value spotting: numbers, arithmetic, quoted strings, paths, URLs, times, dates.
//! Returns protected spans that normalization must not modify.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotKind {
    Number,
    Arith,
    Quoted,
    Path,
    Url,
    Time,
    Date,
    Name,
}

#[derive(Debug, Clone)]
pub struct Spotted {
    pub kind: SlotKind,
    pub text: String,
    pub start: usize,
    pub end: usize,
}

// ---- regex helpers ----

fn quoted_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""[^"]*""#).unwrap())
}

fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"https?://\S+").unwrap())
}

fn path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:~/|\.{1,2}/|/)[a-zA-Z0-9_./-]+").unwrap())
}

fn time_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b\d{1,2}:\d{2}(?::\d{2})?\b|\b\d{1,2}(?:am|pm)\b").unwrap())
}

fn date_iso_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b\d{4}-\d{2}-\d{2}\b").unwrap())
}

/// Arithmetic expression: at least two numbers connected by operators (+,-,*,/,^,%),
/// possibly with spaces and parentheses.
fn arith_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Match expressions like: "3 / 500 * 3600" or "2 * (3 + 4)"
        // Must contain at least one operator and at least two numeric atoms.
        Regex::new(
            r"[\d.]+(?:\s*[\+\-\*/\^%]\s*(?:[\d.]+|\([^\)]+\)))+(?:\s*[\+\-\*/\^%]\s*(?:[\d.]+|\([^\)]+\)))*"
        ).unwrap()
    })
}

/// Bare number (integer or float), not already caught by arith.
fn number_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b").unwrap())
}

// ---- date word lists ----

static DATE_WORDS: &[&str] = &[
    "tomorrow", "yesterday", "today", "monday", "tuesday", "wednesday",
    "thursday", "friday", "saturday", "sunday",
];

static DATE_PHRASES: &[&str] = &[
    "next monday", "next tuesday", "next wednesday", "next thursday",
    "next friday", "next saturday", "next sunday", "next week", "last week",
    "next month", "last month",
];

// ---- main entry point ----

/// Scan `text` for typed value spans. Spans are non-overlapping and sorted by start.
pub fn spot_values(text: &str) -> Vec<Spotted> {
    let mut spans: Vec<Spotted> = vec![];

    // Priority order: quoted > url > path > time > date > arith > number
    add_regex_matches(text, quoted_re(), SlotKind::Quoted, &mut spans);
    add_regex_matches(text, url_re(), SlotKind::Url, &mut spans);
    add_regex_matches(text, path_re(), SlotKind::Path, &mut spans);
    add_regex_matches(text, time_re(), SlotKind::Time, &mut spans);
    add_regex_matches(text, date_iso_re(), SlotKind::Date, &mut spans);

    // Date keywords (case-insensitive)
    let lower = text.to_lowercase();
    for phrase in DATE_PHRASES {
        let mut search = lower.as_str();
        let mut offset = 0usize;
        while let Some(pos) = search.find(phrase) {
            let start = offset + pos;
            let end = start + phrase.len();
            spans.push(Spotted { kind: SlotKind::Date, text: text[start..end].to_string(), start, end });
            offset += pos + phrase.len();
            search = &lower[offset..];
        }
    }
    for word in DATE_WORDS {
        // match as whole word boundary
        let pat = format!(r"\b{}\b", word);
        if let Ok(re) = Regex::new(&pat) {
            let ltext = text.to_lowercase();
            for m in re.find_iter(&ltext) {
                let s = m.start();
                let e = m.end();
                spans.push(Spotted {
                    kind: SlotKind::Date,
                    text: text[s..e].to_string(),
                    start: s,
                    end: e,
                });
            }
        }
    }

    // Arithmetic BEFORE bare numbers (arith subsumes its constituent numbers)
    add_regex_matches(text, arith_re(), SlotKind::Arith, &mut spans);
    add_regex_matches(text, number_re(), SlotKind::Number, &mut spans);

    resolve_overlaps(&mut spans);
    spans
}

fn add_regex_matches(text: &str, re: &Regex, kind: SlotKind, out: &mut Vec<Spotted>) {
    for m in re.find_iter(text) {
        out.push(Spotted {
            kind: kind.clone(),
            text: m.as_str().to_string(),
            start: m.start(),
            end: m.end(),
        });
    }
}

/// Keep the longest non-overlapping spans, sorted by start position.
/// When spans overlap, prefer: longer span, then higher-priority kind.
fn resolve_overlaps(spans: &mut Vec<Spotted>) {
    // Sort by start asc, length desc
    spans.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut kept: Vec<Spotted> = vec![];
    let mut max_end = 0usize;
    for s in spans.drain(..) {
        if s.start >= max_end {
            max_end = s.end;
            kept.push(s);
        }
        // If overlapping, the first (higher priority / longer) wins
    }
    *spans = kept;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arith_spotted() {
        let text = "calculate 3 / 500 * 3600";
        let spots = spot_values(text);
        let arith: Vec<_> = spots.iter().filter(|s| s.kind == SlotKind::Arith).collect();
        assert!(!arith.is_empty(), "expected an Arith spot");
        assert_eq!(arith[0].text, "3 / 500 * 3600");
    }

    #[test]
    fn number_spotted() {
        let text = "what is 42 times 7";
        let spots = spot_values(text);
        let nums: Vec<_> = spots.iter().filter(|s| s.kind == SlotKind::Number).collect();
        assert_eq!(nums.len(), 2);
    }

    #[test]
    fn quoted_spotted() {
        let text = r#"save it as "hello world""#;
        let spots = spot_values(text);
        assert!(spots.iter().any(|s| s.kind == SlotKind::Quoted && s.text == r#""hello world""#));
    }
}
