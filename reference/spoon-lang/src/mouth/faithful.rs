//! Faithfulness checker: verifies that a rendered string expresses exactly
//! the content in the `ResponsePlan` and adds nothing new.

use std::sync::LazyLock;

use regex::Regex;
use spoon_core::types::response::ResponsePlan;

#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub kind: ViolationKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViolationKind {
    MissingMention,
    ExtraNumber,
    ExtraName,
    TooLong,
    ForbiddenPhrase,
    EmDash,
}

static NUMBER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d+(?:\.\d+)?)\b").unwrap());

// Matches "1. " or "1) " at the start of a line (list markers to ignore).
static LIST_MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*\d+[.)]\s").unwrap());

// Capitalized word that could be a proper name.
static NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Z][a-z]+)\b").unwrap());

const FORBIDDEN: &[&str] =
    &["I'd be happy to", "Certainly", "As an AI", "I apologize for any", "Great question"];

/// Check a rendered string against the plan. Returns all violations found.
pub fn check(plan: &ResponsePlan, text: &str) -> Vec<Violation> {
    let mut v: Vec<Violation> = Vec::new();

    // 1. Em-dash / en-dash
    if text.contains('\u{2014}') || text.contains('\u{2013}') {
        v.push(vio(ViolationKind::EmDash, "em-dash or en-dash present"));
    }

    // 2. Forbidden filler phrases
    let text_lc = text.to_lowercase();
    for phrase in FORBIDDEN {
        if text_lc.contains(&phrase.to_lowercase()) {
            v.push(vio(ViolationKind::ForbiddenPhrase, format!("forbidden phrase: {phrase}")));
        }
    }

    // 3. must_mention strings
    for mention in &plan.must_mention {
        if !mention_present(text, mention) {
            v.push(vio(ViolationKind::MissingMention, format!("missing: {mention}")));
        }
    }

    // 4. Numbers in text must all appear in plan
    let plan_json = serde_json::to_value(plan).unwrap_or(serde_json::Value::Null);
    let plan_nums = collect_plan_numbers(&plan_json);

    let marker_ranges: Vec<(usize, usize)> =
        LIST_MARKER_RE.find_iter(text).map(|m| (m.start(), m.end())).collect();

    for cap in NUMBER_RE.captures_iter(text) {
        let mat = cap.get(0).unwrap();
        // Skip numeric list markers like "1. " at line start.
        if marker_ranges.iter().any(|(s, e)| mat.start() >= *s && mat.end() <= *e) {
            continue;
        }
        if let Ok(n) = mat.as_str().parse::<f64>() {
            if !plan_nums.iter().any(|p| nums_eq(*p, n)) {
                v.push(vio(
                    ViolationKind::ExtraNumber,
                    format!("number {} not in plan", mat.as_str()),
                ));
            }
        }
    }

    // 5. Capitalized mid-sentence words that look like proper names must be in plan
    let plan_json_lc = plan_json.to_string().to_lowercase();
    for cap in NAME_RE.captures_iter(text) {
        let mat = cap.get(1).unwrap();
        let word = mat.as_str();
        if word == "I" || sentence_initial(text, mat.start()) {
            continue;
        }
        if !plan_json_lc.contains(&word.to_lowercase()) {
            v.push(vio(ViolationKind::ExtraName, format!("proper name '{word}' not in plan")));
        }
    }

    // 6. Length: text must be <= 4x template realization + 200 chars
    let template_len = super::templates::realize(plan).len();
    let max_len = template_len * 4 + 200;
    if text.len() > max_len {
        v.push(vio(
            ViolationKind::TooLong,
            format!("length {} exceeds max {max_len}", text.len()),
        ));
    }

    v
}

// --- helpers -----------------------------------------------------------------

fn vio(kind: ViolationKind, detail: impl Into<String>) -> Violation {
    Violation { kind, detail: detail.into() }
}

fn nums_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Check whether `mention` appears in `text`. Numeric mentions are compared
/// numerically so "3" matches "3.0".
pub(crate) fn mention_present(text: &str, mention: &str) -> bool {
    let m = mention.trim();
    if let Ok(mn) = m.parse::<f64>() {
        return NUMBER_RE
            .captures_iter(text)
            .any(|c| c[1].parse::<f64>().map(|t| nums_eq(mn, t)).unwrap_or(false));
    }
    let text_norm = normalize_ws(text).to_lowercase();
    let m_norm = normalize_ws(m).to_lowercase();
    text_norm.contains(&m_norm)
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sentence_initial(text: &str, pos: usize) -> bool {
    if pos == 0 {
        return true;
    }
    let before = text[..pos].trim_end_matches(' ');
    before.is_empty()
        || before.ends_with('.')
        || before.ends_with('!')
        || before.ends_with('?')
        || before.ends_with('\n')
}

fn collect_plan_numbers(val: &serde_json::Value) -> Vec<f64> {
    let mut out = Vec::new();
    collect_nums_rec(val, &mut out);
    out
}

fn collect_nums_rec(val: &serde_json::Value, out: &mut Vec<f64>) {
    match val {
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                out.push(f);
            }
        }
        serde_json::Value::String(s) => {
            for cap in NUMBER_RE.captures_iter(s) {
                if let Ok(f) = cap[1].parse::<f64>() {
                    out.push(f);
                }
            }
        }
        serde_json::Value::Array(arr) => arr.iter().for_each(|v| collect_nums_rec(v, out)),
        serde_json::Value::Object(obj) => obj.values().for_each(|v| collect_nums_rec(v, out)),
        _ => {}
    }
}
