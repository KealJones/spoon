//! Verb lemmatization and conjugation for SCE.
//!
//! Lemmatization normalizes inflected verb forms to their base (infinitive) form.
//! Conjugation produces the 3rd-person singular present for declarative sentences.

/// Return the base (infinitive) form of a (possibly inflected) English verb.
///
/// Hyphenated phrasal verbs: only the first segment is lemmatized.
/// Examples: owns->own, tries->try, goes->go, has->have, is->be, does->do.
/// looks-for -> look-for
pub fn lemmatize(word: &str) -> String {
    if let Some(idx) = word.find('-') {
        let first = &word[..idx];
        let rest = &word[idx..];
        return format!("{}{}", base_form(first), rest);
    }
    base_form(word)
}

/// Return the 3rd-person singular present tense of a base-form verb.
/// Used by the realizer for declarative sentences.
pub fn conjugate_3sg(lemma: &str) -> String {
    if let Some(idx) = lemma.find('-') {
        let first = &lemma[..idx];
        let rest = &lemma[idx..];
        return format!("{}{}", conjugate_3sg(first), rest);
    }
    match lemma {
        "be" => return "is".to_string(),
        "have" => return "has".to_string(),
        "do" => return "does".to_string(),
        _ => {}
    }
    let w = lemma;
    // -y -> -ies (try->tries, but play->plays because 'a' is vowel before 'y')
    if w.ends_with('y') && w.len() > 1 {
        let second_last = w.as_bytes().get(w.len() - 2).copied().unwrap_or(0) as char;
        if !"aeiou".contains(second_last) {
            return format!("{}ies", &w[..w.len() - 1]);
        }
    }
    // Sibilant stems: add -es
    let sib = w.ends_with('s')
        || w.ends_with('x')
        || w.ends_with('z')
        || w.ends_with("ch")
        || w.ends_with("sh")
        || w.ends_with('o');
    if sib {
        return format!("{}es", w);
    }
    format!("{}s", w)
}

fn base_form(word: &str) -> String {
    let w = word.to_lowercase();
    let w = w.as_str();
    // Irregular forms
    match w {
        "is" | "are" | "was" | "were" | "am" => return "be".into(),
        "has" => return "have".into(),
        "does" => return "do".into(),
        _ => {}
    }
    // -ies -> -y
    if w.ends_with("ies") && w.len() > 3 {
        return format!("{}y", &w[..w.len() - 3]);
    }
    // -es suffix: sibilant stem -> remove "es", vowel-ending stem -> remove "s"
    if w.ends_with("es") && w.len() > 3 {
        let stem = &w[..w.len() - 2]; // candidate after removing "es"
        let stem_last = stem.bytes().last().unwrap_or(0) as char;
        let stem_second_last = stem.bytes().nth(stem.len().saturating_sub(2)).unwrap_or(0) as char;
        let sibilant = stem_last == 's'
            || stem_last == 'x'
            || stem_last == 'z'
            || stem_last == 'o'
            || (stem_second_last == 'c' && stem_last == 'h')
            || (stem_second_last == 's' && stem_last == 'h');
        if sibilant {
            return stem.to_string(); // process+es->process, go+es->go
        } else {
            return w[..w.len() - 1].to_string(); // believ+e+s->believe
        }
    }
    // plain -s (not -ss, already handled -es above)
    if w.ends_with('s') && w.len() > 2 && !w.ends_with("ss") {
        return w[..w.len() - 1].to_string();
    }
    w.to_string()
}

/// Conservative noun singularizer.
///
/// Returns the singular form of a plural noun. Only applies when the
/// transformation is unambiguous:
///   - `-ies` -> `-y`  (stories->story)
///   - `-ses/-xes/-ches/-shes/-zes` -> drop `-es`  (buses->bus, boxes->box)
///   - plain `-s` -> drop (dogs->dog)
///
/// Does NOT transform words ending in `-ss`, `-us`, `-is`, `-ous`, `-news`,
/// or words in the exception list (invariant plurals).
pub fn singularize_noun(word: &str) -> String {
    let w = word.to_lowercase();
    // Irregular plurals: no suffix rule reaches these.
    if let Some((_, singular)) = IRREGULAR_PLURALS.iter().find(|(plural, _)| *plural == w) {
        return singular.to_string();
    }
    // Invariant / already-singular exception patterns
    if w.ends_with("ss")
        || w.ends_with("us")
        || w.ends_with("is")
        || w.ends_with("ous")
        || w.ends_with("ics")
        || w.ends_with("ness")
        || w.ends_with("ess")
        || is_invariant(&w)
    {
        return word.to_string();
    }
    // -ies -> -y
    if w.ends_with("ies") && w.len() > 3 {
        return format!("{}y", &word[..word.len() - 3]);
    }
    // sibilant -es: buses->bus, boxes->box, churches->church, dishes->dish, buzzes->buzz
    if w.len() > 3 {
        let stem = &w[..w.len() - 2];
        let sib = stem.ends_with('s')
            || stem.ends_with('x')
            || stem.ends_with('z')
            || stem.ends_with("ch")
            || stem.ends_with("sh");
        if w.ends_with("es") && sib {
            return word[..word.len() - 2].to_string();
        }
    }
    // plain -s -> drop (not -ss, already handled above)
    if w.ends_with('s') && w.len() > 2 {
        return word[..word.len() - 1].to_string();
    }
    word.to_string()
}

/// Nouns that are the same in both numbers, so neither direction touches them.
fn is_invariant(w: &str) -> bool {
    matches!(
        w,
        "news" | "series" | "species" | "means" | "deer" | "sheep" | "fish"
            | "aircraft" | "data" | "media" | "criteria" | "phenomena"
            | "software" | "hardware" | "access" | "process" | "address"
            | "basis" | "analysis" | "thesis" | "crisis" | "axis"
    )
}

/// Conservative noun pluralizer, the mirror of `singularize_noun`.
///
/// A word that already singularizes to something else is left alone, so
/// `titles` stays `titles` and only a genuine singular grows an ending.
pub fn pluralize_noun(word: &str) -> String {
    let w = word.to_lowercase();
    if let Some((plural, _)) = IRREGULAR_PLURALS.iter().find(|(_, singular)| *singular == w) {
        return plural.to_string();
    }
    // Already plural, or a noun that has no separate plural at all.
    if is_invariant(&w) || singularize_noun(&w) != w {
        return word.to_string();
    }
    let consonant_y = w.ends_with('y')
        && w.len() > 1
        && !matches!(w.as_bytes()[w.len() - 2], b'a' | b'e' | b'i' | b'o' | b'u');
    if consonant_y {
        return format!("{}ies", &word[..word.len() - 1]);
    }
    if w.ends_with('s') || w.ends_with('x') || w.ends_with('z') || w.ends_with("ch") || w.ends_with("sh") {
        return format!("{word}es");
    }
    format!("{word}s")
}

/// Plurals whose singular is not a suffix change.
static IRREGULAR_PLURALS: &[(&str, &str)] = &[
    ("mice", "mouse"), ("men", "man"), ("women", "woman"), ("children", "child"),
    ("people", "person"), ("feet", "foot"), ("teeth", "tooth"), ("geese", "goose"),
    ("oxen", "ox"), ("wolves", "wolf"), ("lives", "life"), ("knives", "knife"),
    ("leaves", "leaf"), ("halves", "half"), ("shelves", "shelf"), ("thieves", "thief"),
];

/// Map number words to their integer values.
pub fn number_word(w: &str) -> Option<u32> {
    match w.to_lowercase().as_str() {
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        "ten" => Some(10),
        "eleven" => Some(11),
        "twelve" => Some(12),
        "dozen" => Some(12),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lemmatize_basic() {
        assert_eq!(lemmatize("owns"), "own");
        assert_eq!(lemmatize("believes"), "believe");
        assert_eq!(lemmatize("tries"), "try");
        assert_eq!(lemmatize("goes"), "go");
        assert_eq!(lemmatize("has"), "have");
        assert_eq!(lemmatize("is"), "be");
        assert_eq!(lemmatize("does"), "do");
        assert_eq!(lemmatize("processes"), "process");
        assert_eq!(lemmatize("watches"), "watch");
    }

    #[test]
    fn lemmatize_phrasal() {
        assert_eq!(lemmatize("looks-for"), "look-for");
        assert_eq!(lemmatize("knocks-out"), "knock-out");
    }

    #[test]
    fn pluralize_is_the_mirror_of_singularize() {
        assert_eq!(pluralize_noun("title"), "titles");
        assert_eq!(pluralize_noun("story"), "stories");
        assert_eq!(pluralize_noun("box"), "boxes");
        assert_eq!(pluralize_noun("day"), "days");
        assert_eq!(pluralize_noun("child"), "children");
        // Already plural or invariant: left alone.
        assert_eq!(pluralize_noun("titles"), "titles");
        assert_eq!(pluralize_noun("series"), "series");
    }

    #[test]
    fn conj_3sg() {
        assert_eq!(conjugate_3sg("own"), "owns");
        assert_eq!(conjugate_3sg("try"), "tries");
        assert_eq!(conjugate_3sg("be"), "is");
        assert_eq!(conjugate_3sg("have"), "has");
        assert_eq!(conjugate_3sg("do"), "does");
        assert_eq!(conjugate_3sg("go"), "goes");
        assert_eq!(conjugate_3sg("process"), "processes");
    }
}
