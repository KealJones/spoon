//! Stance (opinion) types and parsing for the teacher seat.

/// An opinion Spoon can hold and defend, as produced by the teacher.
///
/// The caller (brain) converts this to a `store::Stance` when persisting.
/// Counterpoints are kept here for richer discourse but dropped on storage.
#[derive(Debug, Clone, PartialEq)]
pub struct TaughtStance {
    pub topic: String,
    /// Plain English statement of the position, max 200 chars.
    pub stance: String,
    pub reasons: Vec<String>,
    pub counterpoints: Vec<String>,
    pub confidence: f32,
}

/// Parse a teacher-produced stance JSON string into a `TaughtStance`.
///
/// Expected shape:
/// ```json
/// {
///   "topic": "...",
///   "stance": "...",
///   "reasons": ["...", "..."],
///   "counterpoints": ["..."],
///   "confidence": 0.8
/// }
/// ```
pub fn parse_stance_json(json: &str) -> Result<TaughtStance, String> {
    let json = super::strip_fences(json);
    let raw: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("stance JSON parse error: {e}"))?;
    super::check_no_code(&raw)?;

    let topic = raw
        .get("topic")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "stance missing 'topic'".to_string())?
        .to_string();

    let stance_text = raw
        .get("stance")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "stance missing 'stance'".to_string())?
        .to_string();

    if stance_text.len() > 200 {
        return Err(format!(
            "stance text exceeds 200 chars ({})",
            stance_text.len()
        ));
    }

    let reasons: Vec<String> = raw
        .get("reasons")
        .and_then(|r| r.as_array())
        .ok_or_else(|| "stance missing 'reasons' array".to_string())?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| "reason must be a string".to_string())
                .map(str::to_string)
        })
        .collect::<Result<_, _>>()?;

    if reasons.len() < 2 || reasons.len() > 4 {
        return Err(format!(
            "stance must have 2-4 reasons, got {}",
            reasons.len()
        ));
    }

    let counterpoints: Vec<String> = raw
        .get("counterpoints")
        .and_then(|c| c.as_array())
        .ok_or_else(|| "stance missing 'counterpoints' array".to_string())?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| "counterpoint must be a string".to_string())
                .map(str::to_string)
        })
        .collect::<Result<_, _>>()?;

    let confidence = raw
        .get("confidence")
        .and_then(|c| c.as_f64())
        .ok_or_else(|| "stance missing 'confidence' float".to_string())? as f32;

    if !(0.0..=1.0).contains(&confidence) {
        return Err(format!(
            "confidence {confidence} is out of range 0.0-1.0"
        ));
    }

    Ok(TaughtStance {
        topic,
        stance: stance_text,
        reasons,
        counterpoints,
        confidence,
    })
}
