//! Curriculum (pretrain lesson plan) types and parsing for the teacher seat.

use std::collections::HashSet;

/// A single learning task in a pretrain curriculum.
#[derive(Debug, Clone, PartialEq)]
pub enum Lesson {
    /// A new capability Spoon should gain (to be synthesized from a spec).
    Capability { description: String, signature_hint: String },
    /// Factual SCE sentences to assert into the knowledge store.
    Facts { sce: Vec<String> },
    /// Phrasings for an existing action that Spoon should recognise.
    Phrasings { sce: String, verb: String },
    /// A topic Spoon should form a stance on.
    Opinion { topic: String },
    /// A noun Spoon should model as a concept.
    Concept { noun: String },
}

impl Lesson {
    /// Lesson kind as a lowercase label ("capability", "facts", ...).
    pub fn kind(&self) -> &'static str {
        match self {
            Lesson::Capability { .. } => "capability",
            Lesson::Facts { .. } => "facts",
            Lesson::Phrasings { .. } => "phrasings",
            Lesson::Opinion { .. } => "opinion",
            Lesson::Concept { .. } => "concept",
        }
    }

    /// Stable identity derived from the content: dedup within a curriculum
    /// and the `teach.done` ledger across runs.
    pub fn key(&self) -> String {
        match self {
            Lesson::Capability { description, .. } => format!("cap:{}", description.trim().to_lowercase()),
            Lesson::Facts { sce } => format!("facts:{}", sce.join("|")),
            Lesson::Phrasings { sce, verb } => format!("phrasings:{sce}:{verb}"),
            Lesson::Opinion { topic } => format!("opinion:{}", topic.trim().to_lowercase()),
            Lesson::Concept { noun } => format!("concept:{}", noun.trim().to_lowercase()),
        }
    }
}

/// Parse a teacher-produced curriculum JSON string into a deduplicated
/// `Vec<Lesson>`.
///
/// Expected shape: `{"lessons": [...]}`. Malformed lessons are dropped so one
/// bad entry from a small model does not cost the whole curriculum; the
/// result is an error only when no lesson survives.
pub fn parse_lessons_json(json: &str) -> Result<Vec<Lesson>, String> {
    let json = super::strip_fences(json);
    let raw: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("curriculum JSON parse error: {e}"))?;
    super::check_no_code(&raw)?;

    let arr = raw
        .get("lessons")
        .and_then(|l| l.as_array())
        .ok_or_else(|| "curriculum JSON must have a 'lessons' array".to_string())?;

    if arr.is_empty() {
        return Err("lessons array is empty".to_string());
    }

    let mut first_error: Option<String> = None;
    let raw_lessons: Vec<Lesson> = arr
        .iter()
        .filter_map(|v| match parse_wire_lesson(v) {
            Ok(l) => Some(l),
            Err(e) => {
                tracing::warn!("curriculum: dropped lesson ({e}): {v}");
                first_error.get_or_insert(e);
                None
            }
        })
        .collect();
    if raw_lessons.is_empty() {
        return Err(format!("no usable lesson: {}", first_error.unwrap_or_default()));
    }

    // Deduplicate by a stable key derived from lesson content.
    let mut seen: HashSet<String> = HashSet::new();
    let lessons: Vec<Lesson> = raw_lessons
        .into_iter()
        .filter(|l| seen.insert(l.key()))
        .collect();

    Ok(lessons)
}

/// The verb of a canonical SCE sentence when the teacher left it out:
/// `Assistant, reverse "abc"!` -> `reverse`; `What is the length of "abc"?`
/// -> `length`. Anything else is not guessed.
pub fn verb_from_sce(sce: &str) -> Option<String> {
    let words: Vec<String> = sce
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let verb = match words.as_slice() {
        [first, verb, ..] if first == "assistant" => verb,
        _ => words.iter().position(|w| w == "the").and_then(|i| words.get(i + 1))?,
    };
    (verb.chars().all(|c| c.is_ascii_lowercase() || c == '-')).then(|| verb.clone())
}

fn parse_wire_lesson(v: &serde_json::Value) -> Result<Lesson, String> {
    let kind = v
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or_else(|| "lesson missing 'kind' field".to_string())?;

    match kind {
        "capability" => {
            let signature_hint = v
                .get("signature_hint")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            // Small models drop the description but keep the hint; the
            // function name in the hint is description enough.
            let description = match v.get("description").and_then(|d| d.as_str()).map(str::trim) {
                Some(d) if !d.is_empty() => d.to_string(),
                _ => signature_hint
                    .split('(')
                    .next()
                    .map(|name| name.trim().replace('_', " "))
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| "capability lesson missing 'description'".to_string())?,
            };
            Ok(Lesson::Capability { description, signature_hint })
        }
        "facts" => {
            let sce_arr = v
                .get("sce")
                .and_then(|s| s.as_array())
                .ok_or_else(|| "facts lesson missing 'sce' array".to_string())?;
            let sce: Vec<String> = sce_arr
                .iter()
                .map(|s| {
                    s.as_str()
                        .ok_or_else(|| "sce entry must be a string".to_string())
                        .map(str::to_string)
                })
                .collect::<Result<_, _>>()?;
            Ok(Lesson::Facts { sce })
        }
        "phrasings" => {
            let sce = v
                .get("sce")
                .and_then(|s| s.as_str())
                .ok_or_else(|| "phrasings lesson missing 'sce'".to_string())?
                .to_string();
            let verb = match v.get("verb").and_then(|s| s.as_str()).map(str::trim) {
                Some(verb) if !verb.is_empty() => verb.to_string(),
                _ => verb_from_sce(&sce).ok_or_else(|| "phrasings lesson missing 'verb'".to_string())?,
            };
            Ok(Lesson::Phrasings { sce, verb })
        }
        "opinion" => {
            let topic = v
                .get("topic")
                .and_then(|t| t.as_str())
                .ok_or_else(|| "opinion lesson missing 'topic'".to_string())?
                .to_string();
            Ok(Lesson::Opinion { topic })
        }
        "concept" => {
            let noun = v
                .get("noun")
                .and_then(|n| n.as_str())
                .ok_or_else(|| "concept lesson missing 'noun'".to_string())?
                .to_string();
            Ok(Lesson::Concept { noun })
        }
        other => Err(format!("unknown lesson kind: '{other}'")),
    }
}