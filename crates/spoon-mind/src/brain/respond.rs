//! Response building helpers for the brain turn loop.

use spoon_core::types::*;

/// Build a ResponsePlan from executor outcome, merging accumulated moves.
pub fn exec_result_plan(
    value: Value,
    action_id: &ActionId,
    steps: usize,
    moves_before: Vec<Move>,
) -> ResponsePlan {
    let mut moves = moves_before;
    // dialog.* primitives return Value::Json that deserializes into Move.
    if let Value::Json(ref j) = value {
        if let Ok(m) = serde_json::from_value::<Move>(j.clone()) {
            moves.push(m);
            return ResponsePlan::new(moves);
        }
    }
    moves.push(Move::Result {
        action: action_id.clone(),
        value,
        steps,
    });
    ResponsePlan::new(moves)
}

/// Permission answer heuristic.
pub fn parse_permission(text: &str) -> Option<bool> {
    let lower = text.trim().to_lowercase();
    let stripped = lower
        .trim_end_matches(|c: char| ".!?".contains(c))
        .trim();
    let yes = [
        "yes", "y", "ok", "okay", "sure", "go ahead", "go", "do it", "yep",
        "yup", "yeah", "affirmative", "proceed",
    ];
    let no = [
        "no", "n", "nope", "stop", "cancel", "abort", "don't", "nah",
        "negative",
    ];
    if yes.contains(&stripped) {
        return Some(true);
    }
    if no.contains(&stripped) {
        return Some(false);
    }
    None
}

/// Choice answer: try to parse a 1-based number.
pub fn parse_choice(text: &str) -> Option<usize> {
    text.trim()
        .parse::<usize>()
        .ok()
        .filter(|&n| n >= 1)
        .map(|n| n - 1)
}
