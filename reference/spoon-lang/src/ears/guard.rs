//! Garbage guard for the LLM seat. A small model rewrites `zxqv flarp wibble`
//! into the perfectly grammatical `Zxqv is a wibble.`; the parser accepts it
//! because SCE's lexicon is open. The guard rejects any LLM (or repaired)
//! result whose clauses carry no known content word: every word that is not
//! SCE structure is one nobody knows (not in the ears lexicon, reported by
//! the gate as placed by position only, or an unseen Name).
//!
//! `John owns a dog.` from a fresh user passes: `John` is unknown, `owns` and
//! `dog` are not.

/// The normalizer's escape hatch: exactly this output means "no recoverable
/// meaning" and is Failed without a repair round.
pub const NO_MEANING: &str = "??";

/// True when `sce` has content positions and every one of them holds a word
/// from `unknown` (case-insensitive). `unknown` is the merged list from
/// `Ears::parse_with_unknowns`: gate-reported unknowns minus ears vocabulary,
/// plus names the lexicon has never seen.
pub fn is_garbage(sce: &str, unknown: &[String]) -> bool {
    let words = content_words(sce);
    if words.is_empty() {
        return false;
    }
    words.iter().all(|w| unknown.iter().any(|u| u.eq_ignore_ascii_case(w)))
}

/// True when `sce` carries a number the input never mentioned. The prompt
/// forbids arithmetic and the parser cannot tell `(481 + 26) / 4` copied from
/// an example apart from the user's own numbers; this can.
pub fn invents_numbers(sce: &str, input: &str) -> bool {
    let known: Vec<&str> = numbers(input).collect();
    numbers(sce).any(|n| !known.contains(&n))
}

fn numbers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_ascii_digit() && c != '.')
        .map(|t| t.trim_matches('.'))
        .filter(|t| t.chars().any(|c| c.is_ascii_digit()))
}

/// Words in content positions: everything that is not SCE structure, a
/// literal, a variable, or a minted name. Quoted strings are literals and
/// never count either way.
fn content_words(sce: &str) -> Vec<String> {
    let mut out = vec![];
    let mut in_quote = false;
    for raw in sce.split_whitespace() {
        let quotes = raw.matches('"').count();
        let inside = in_quote || raw.starts_with('"');
        if quotes % 2 == 1 {
            in_quote = !in_quote;
        }
        if inside {
            continue;
        }
        let core = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '\'');
        let core = core.strip_suffix("'s").unwrap_or(core);
        if core.is_empty() || is_literal(core) || is_symbol(core) || is_function_word(&core.to_lowercase()) {
            continue;
        }
        out.push(core.to_string());
    }
    out
}

fn is_literal(w: &str) -> bool {
    w.starts_with(|c: char| c.is_ascii_digit())
        || w.parse::<f64>().is_ok()
        || w.starts_with('/')
        || w.starts_with("./")
        || w.starts_with("~/")
        || w.starts_with("http://")
        || w.starts_with("https://")
}

/// Variables (`X`, `Y1`) and minted names (`Object-X`, `File-A`).
fn is_symbol(w: &str) -> bool {
    let mut chars = w.chars();
    let is_var = matches!(chars.next(), Some(c) if c.is_ascii_uppercase()) && chars.all(|c| c.is_ascii_digit());
    let minted = w.split_once('-').is_some_and(|(_, after)| after.starts_with(|c: char| c.is_uppercase()));
    is_var || minted
}

/// SCE structure words: determiners, quantifier pieces, copula and auxiliaries,
/// modals, connectives, prepositions, question words, reserved names, time
/// literals. None of them can carry the meaning of an utterance on its own.
fn is_function_word(w: &str) -> bool {
    matches!(
        w,
        "a" | "an" | "the" | "every" | "no" | "some" | "not" | "at" | "least" | "most" | "exactly" | "more" | "than"
            | "unknown" | "is" | "are" | "does" | "do" | "did" | "was" | "were" | "has" | "have" | "had" | "be"
            | "can" | "cannot" | "should" | "must" | "may" | "will" | "would" | "could" | "that" | "and" | "or"
            | "then" | "if" | "for" | "there" | "it" | "to" | "in" | "on" | "from" | "with" | "about" | "toward"
            | "into" | "of" | "by" | "as" | "before" | "after" | "who" | "what" | "which" | "where" | "when"
            | "how" | "many" | "why" | "user" | "assistant" | "spoon" | "now" | "today" | "tomorrow" | "yesterday"
            | "possible" | "something" | "someone" | "somebody" | "anything" | "nothing"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unk(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn all_unknown_content_is_garbage() {
        assert!(is_garbage("Zxqv is a wibble.", &unk(&["wibble", "Zxqv"])));
        assert!(is_garbage("Zxqv flarps a wibble.", &unk(&["flarp", "flarps", "wibble", "Zxqv"])));
        assert!(is_garbage("It is possible that Zxqv is a wibble.", &unk(&["wibble", "Zxqv"])));
    }

    #[test]
    fn one_known_content_word_rescues_the_sentence() {
        assert!(!is_garbage("John owns a dog.", &unk(&["John"])));
        assert!(!is_garbage("Zxqv is a dog.", &unk(&["Zxqv"])));
        assert!(!is_garbage("Mary likes Zxqv.", &unk(&["Zxqv"])));
    }

    #[test]
    fn numbers_must_come_from_the_input() {
        assert!(invents_numbers("Assistant, calculate (481 + 26) / 4!", "John says that John covers Mike's shift."));
        assert!(!invents_numbers("Assistant, calculate (481 + 26) / 4!", "481 plus 26 divided by 4"));
        assert!(!invents_numbers("There are at least 3 cats.", "there are at least 3 cats in there"));
        assert!(!invents_numbers("User is sad.", "User is sad"));
        assert!(invents_numbers("User owns 2 cats.", "User owns 3 cats"));
    }

    #[test]
    fn structure_only_and_literals_are_not_garbage() {
        assert!(!is_garbage("There is X.", &[]));
        assert!(!is_garbage("Assistant, say \"zxqv\"!", &[]));
        assert!(!is_garbage("What is 42?", &[]));
        assert!(is_garbage("Object-X is a wibble.", &unk(&["wibble"])), "minted names are syntax, not content");
    }
}
