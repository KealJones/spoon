//! Correction detection.
//!
//! `detect` is pure text heuristics over the raw user utterance, run BEFORE
//! the ears. `detect_clause` recognises the two SCE-shaped corrections the
//! brain acts on (`"pup" means "dog".`, `User means Mary.`) after parsing.
//! No LLM anywhere.

use spoon_core::types::*;

#[derive(Debug, Clone, PartialEq)]
pub enum Correction {
    /// "no", "wrong", "that's wrong" - a flat rejection.
    Wrong,
    /// "no, I meant X" - rejection with clarification.
    Meant { text: String },
    /// "X means Y" - vocabulary teaching.
    Synonym { word: String, means: String },
    /// "yes", "correct", "exactly" - confirmation of last output.
    Confirm,
    /// "undo", "never mind", "nvm" - retract last turn.
    Undo,
}

/// Detect a correction/teaching signal in a raw user utterance.
/// Returns None when no known pattern matches.
pub fn detect(text: &str) -> Option<Correction> {
    let lower = text.trim().to_lowercase();
    let stripped = lower
        .trim_end_matches(|c: char| ".!?".contains(c))
        .trim();

    // ---- Undo ----
    let undo_patterns = [
        "undo",
        "forget that",
        "never mind",
        "nevermind",
        "nvm",
        "scratch that",
        "disregard that",
        "ignore that",
    ];
    for pat in &undo_patterns {
        if stripped == *pat
            || stripped.starts_with(&format!("{},", pat))
            || stripped.starts_with(&format!("{} ", pat))
        {
            return Some(Correction::Undo);
        }
    }

    // ---- Confirm (standalone only) ----
    let confirm_patterns = [
        "yes",
        "yep",
        "yup",
        "yeah",
        "correct",
        "right",
        "exactly",
        "affirmative",
        "indeed",
        "that's right",
        "that is right",
        "that's correct",
    ];
    for pat in &confirm_patterns {
        if stripped == *pat {
            return Some(Correction::Confirm);
        }
    }

    // ---- Synonym teaching ----
    if let Some(s) = detect_synonym(stripped) {
        return Some(s);
    }

    // ---- Wrong (exact) ----
    if stripped == "wrong"
        || stripped == "nope"
        || stripped == "no"
        || stripped == "incorrect"
        || stripped == "not that"
        || stripped == "that's wrong"
    {
        return Some(Correction::Wrong);
    }

    // ---- Wrong + optional Meant ----
    let wrong_starters: &[&str] = &[
        "no, ",
        "no,",
        "no ",
        "nope, ",
        "nope,",
        "nope ",
        "wrong, ",
        "wrong,",
        "wrong ",
        "that's wrong",
        "not that ",
        "not that,",
        "incorrect, ",
        "incorrect,",
        "incorrect ",
    ];
    for starter in wrong_starters {
        if stripped.starts_with(starter) {
            let rest = stripped[starter.len()..].trim();
            if let Some(meant_text) = extract_meant(rest) {
                return Some(Correction::Meant { text: meant_text });
            }
            return Some(Correction::Wrong);
        }
    }

    None
}

/// Detect a correction stated in SCE, after parsing:
/// `"pup" means "dog".` -> Synonym, `User means Mary.` -> Meant.
pub fn detect_clause(clause: &Clause) -> Option<Correction> {
    if !matches!(clause.act, Act::Assert) || clause.conditions.len() != 1 {
        return None;
    }
    let p = &clause.conditions[0];
    if p.pred != "mean" || p.negated || p.args.len() != 2 {
        return None;
    }
    let quant_of = |term: &Term| match term {
        Term::Var { var } => clause.referents.iter().find(|r| &r.var == var).map(|r| r.quant.clone()),
        Term::Value { value } => Some(Quant::Literal(value.clone())),
        _ => None,
    };
    match (quant_of(&p.args[0])?, quant_of(&p.args[1])?) {
        (Quant::Literal(Value::Text(word)), Quant::Literal(Value::Text(means))) => {
            Some(Correction::Synonym { word, means })
        }
        (Quant::Named(who), Quant::Named(name)) if who == "User" => Some(Correction::Meant { text: name }),
        _ => None,
    }
}

fn detect_synonym(stripped: &str) -> Option<Correction> {
    // "X means Y" - X must be a single word
    if let Some(pos) = stripped.find(" means ") {
        let word = stripped[..pos].trim();
        let means = stripped[pos + 7..].trim();
        if !word.is_empty() && !means.is_empty() && !word.contains(' ') {
            return Some(Correction::Synonym {
                word: word.to_string(),
                means: means.to_string(),
            });
        }
    }

    // "by X i mean Y"
    if stripped.starts_with("by ") {
        let rest = &stripped[3..];
        if let Some(i_pos) = rest.find(" i mean ") {
            let word = rest[..i_pos].trim();
            let means = rest[i_pos + 8..].trim();
            if !word.is_empty() && !means.is_empty() {
                return Some(Correction::Synonym {
                    word: word.to_string(),
                    means: means.to_string(),
                });
            }
        }
    }

    // "when i say X i mean Y"
    if stripped.starts_with("when i say ") {
        let rest = &stripped[11..];
        if let Some(i_pos) = rest.find(" i mean ") {
            let word = rest[..i_pos].trim();
            let means = rest[i_pos + 8..].trim();
            if !word.is_empty() && !means.is_empty() {
                return Some(Correction::Synonym {
                    word: word.to_string(),
                    means: means.to_string(),
                });
            }
        }
    }

    // "X is another word for Y"
    if let Some(pos) = stripped.find(" is another word for ") {
        let word = stripped[..pos].trim();
        let means = stripped[pos + 21..].trim();
        if !word.is_empty() && !means.is_empty() {
            return Some(Correction::Synonym {
                word: word.to_string(),
                means: means.to_string(),
            });
        }
    }

    // "define X as Y"
    if stripped.starts_with("define ") {
        let rest = &stripped[7..];
        if let Some(as_pos) = rest.find(" as ") {
            let word = rest[..as_pos].trim();
            let means = rest[as_pos + 4..].trim();
            if !word.is_empty() && !means.is_empty() {
                return Some(Correction::Synonym {
                    word: word.to_string(),
                    means: means.to_string(),
                });
            }
        }
    }

    None
}

fn extract_meant(rest: &str) -> Option<String> {
    let patterns = [
        "i meant ",
        "i mean ",
        "i was referring to ",
        "i meant to say ",
        "meant ",
    ];
    for p in &patterns {
        if let Some(pos) = rest.find(p) {
            let text = rest[pos + p.len()..].trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}
