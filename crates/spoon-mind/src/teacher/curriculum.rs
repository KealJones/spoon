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

/// Parse a teacher-produced curriculum JSON string into a deduplicated
/// `Vec<Lesson>`.
///
/// Expected shape: `{"lessons": [...]}`
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

    let raw_lessons: Vec<Lesson> = arr
        .iter()
        .map(parse_wire_lesson)
        .collect::<Result<_, _>>()?;

    // Deduplicate by a stable key derived from lesson content.
    let mut seen: HashSet<String> = HashSet::new();
    let lessons: Vec<Lesson> = raw_lessons
        .into_iter()
        .filter(|l| seen.insert(lesson_key(l)))
        .collect();

    Ok(lessons)
}

fn parse_wire_lesson(v: &serde_json::Value) -> Result<Lesson, String> {
    let kind = v
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or_else(|| "lesson missing 'kind' field".to_string())?;

    match kind {
        "capability" => {
            let description = v
                .get("description")
                .and_then(|d| d.as_str())
                .ok_or_else(|| "capability lesson missing 'description'".to_string())?
                .to_string();
            let signature_hint = v
                .get("signature_hint")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
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
            let verb = v
                .get("verb")
                .and_then(|s| s.as_str())
                .ok_or_else(|| "phrasings lesson missing 'verb'".to_string())?
                .to_string();
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

fn lesson_key(l: &Lesson) -> String {
    match l {
        Lesson::Capability { description, .. } => format!("cap:{description}"),
        Lesson::Facts { sce } => format!("facts:{}", sce.join("|")),
        Lesson::Phrasings { sce, verb } => format!("phrasings:{sce}:{verb}"),
        Lesson::Opinion { topic } => format!("opinion:{topic}"),
        Lesson::Concept { noun } => format!("concept:{noun}"),
    }
}
