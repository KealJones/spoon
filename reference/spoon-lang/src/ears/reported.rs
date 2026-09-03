//! Reported speech, one level. `Jake told me "Sarah said Bob hates me"` ->
//! `Jake tells User that Sarah says that Bob hates Jake.` The outer quote is
//! unwrapped once: the perspective shifts to the speaker (`I/me/my` ->
//! `Jake/Jake's`, `you/your` -> the addressee), the tense moves to the
//! present, emphasis capitals settle, and whatever follows the closing quote
//! becomes its own sentence. A quote nested inside the quote is handed on
//! exactly as written; the LLM gets that level.

use regex::Regex;
use std::sync::OnceLock;

use crate::ears::lexicon::Lexicon;
use crate::ears::loops::agree;
use crate::ears::rules::{is_verbish, present_tense};

macro_rules! regex {
    ($pat:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pat).expect("static regex"))
    }};
}

/// Unwrap the first quoted report in `body` (a sentence without its
/// terminator). Returns the unwrapped sentence and whatever followed the
/// quote (possibly empty), for the caller to run the sentence rules on.
pub fn unwrap_reported(body: &str, lexicon: &Lexicon) -> Option<(String, String)> {
    let re = regex!(
        r#"^(?:(?:today|yesterday|earlier|so|and|then)\s+)*(User's [a-z]+|the [a-z]+|[A-Za-z][a-z-]*)\s+(tells|told|says|said|goes|went|is like|was like|texts|texted|writes|wrote|asks|asked)\s+(?:(User's [a-z]+|[A-Z][a-z]+)\s+)?(?:that\s+)?"([^"]*)"(.*)$"#
    );
    let caps = re.captures(body)?;
    let speaker = speaker_name(&caps[1], lexicon)?;
    let verb = &caps[2];
    let addressee = match caps.get(3) {
        Some(m) if verb.starts_with("t") && !verb.starts_with("texts") && !verb.starts_with("texted") => {
            Some(m.as_str().to_string())
        }
        Some(_) => return None,
        None => None,
    };
    if verb.starts_with("asks") || verb.starts_with("asked") {
        return None;
    }
    let listener = addressee.clone().unwrap_or_else(|| "User".to_string());

    let quote = caps[4].trim();
    let quote = strip_quote_openers(quote);
    let quote = quote.trim_end_matches(['.', '!']).trim();
    if quote.ends_with('?') || is_question(quote) || is_conditional(quote) || !is_clause(quote, lexicon) {
        return None;
    }
    let inner = shift_perspective(quote, &speaker, &listener, lexicon);

    let sentence = match addressee {
        Some(to) => format!("{speaker} tells {to} that {inner}."),
        None => format!("{speaker} says that {inner}."),
    };
    let rest = regex!(r"^[\s,;]*(?:(?:and|but|so|then|and then)\s+)?(.*)$").captures(&caps[5]).map(|c| c[1].trim().to_string());
    Some((sentence, rest.unwrap_or_default()))
}

/// The speaker as an SCE noun phrase. Kin and role words become `User's N`,
/// pronouns become unknown people, an unknown lowercase word is a name the
/// normalizer had lowercased, and anything else known is not a speaker.
fn speaker_name(raw: &str, lexicon: &Lexicon) -> Option<String> {
    if let Some(kin) = raw.strip_prefix("User's ") {
        return Some(match kin {
            "dad" | "daddy" | "papa" => "User's father".into(),
            "mom" | "mum" | "mommy" | "mama" => "User's mother".into(),
            _ => raw.to_string(),
        });
    }
    if raw.starts_with("the ") || raw == "User" || raw == "Assistant" {
        return Some(raw.to_string());
    }
    if let Some(name) = lexicon.canonical_name(raw) {
        return Some(name.to_string());
    }
    match raw {
        "dad" | "father" | "daddy" | "papa" => return Some("User's father".into()),
        "mom" | "mother" | "mum" | "mommy" | "mama" => return Some("User's mother".into()),
        "boss" | "manager" | "wife" | "husband" | "brother" | "sister" | "friend" | "roommate" | "coworker" => {
            return Some(format!("User's {raw}"))
        }
        "she" => return Some("an unknown woman".into()),
        "he" => return Some("an unknown man".into()),
        _ => {}
    }
    if raw.starts_with(|c: char| c.is_uppercase()) {
        return Some(raw.to_string());
    }
    if lexicon.is_known(raw) {
        return None;
    }
    Some(capitalize(raw))
}

/// Discourse noise at the start of a quote: `and I told him` -> `I told him`.
fn strip_quote_openers(quote: &str) -> &str {
    let re = regex!(r"(?i)^(?:(?:and|but|so|well|dude|bro|man|wait|nah|no|oh|ok|okay|hey|look|listen|what\?|like)[,!.]?\s+)+");
    match re.find(quote) {
        Some(m) => &quote[m.end()..],
        None => quote,
    }
}

fn is_question(quote: &str) -> bool {
    let first = quote.split_whitespace().next().unwrap_or("").to_lowercase();
    matches!(
        first.as_str(),
        "who" | "what" | "where" | "when" | "which" | "why" | "how" | "is" | "are" | "does" | "do" | "did" | "can"
            | "could" | "should" | "will" | "would" | "was" | "were"
    )
}

fn is_conditional(quote: &str) -> bool {
    let first = quote.split_whitespace().next().unwrap_or("").to_lowercase();
    matches!(first.as_str(), "if" | "when" | "whenever" | "unless")
}

/// A report has to report something said: at least two words, one of them a
/// verb. `"hi there"` is a literal, not a clause.
fn is_clause(quote: &str, lexicon: &Lexicon) -> bool {
    let words: Vec<&str> = quote.split_whitespace().collect();
    words.len() >= 2 && words.iter().any(|w| is_verbish(&w.to_lowercase(), lexicon))
}

/// `I/me/my` -> speaker, `you/your` -> listener, past -> present, emphasis
/// capitals -> ordinary words. A nested single-quoted span is kept verbatim.
fn shift_perspective(quote: &str, speaker: &str, listener: &str, lexicon: &Lexicon) -> String {
    let speaker_ref = definite(speaker);
    let mut out: Vec<String> = vec![];
    let mut in_nested = false;
    for word in quote.split_whitespace() {
        let opens = word.starts_with('\'');
        let closes = word.ends_with('\'') || word.ends_with("'.") || word.ends_with("',") || word.ends_with("'!");
        if in_nested || opens {
            out.push(word.to_string());
            in_nested = (in_nested || opens) && !closes;
            continue;
        }
        let core = word.trim_end_matches([',', ';', ':', '.', '!']);
        let suffix = &word[core.len()..];
        let (stem, possessive) = match core.strip_suffix("'s") {
            Some(s) => (s, true),
            None => (core, false),
        };
        let shifted = match stem.to_lowercase().as_str() {
            "i" | "me" | "myself" => speaker_ref.clone(),
            "my" | "mine" => format!("{speaker_ref}'s"),
            "you" | "yourself" if speaker != "User" => listener.to_string(),
            "your" | "yours" if speaker != "User" => format!("{listener}'s"),
            "dad" | "daddy" | "papa" => "father".to_string(),
            "mom" | "mum" | "mommy" | "mama" => "mother".to_string(),
            _ => settle_case(stem, lexicon),
        };
        let shifted = if possessive { format!("{shifted}'s") } else { shifted };
        out.push(format!("{shifted}{suffix}"));
    }
    let text = present_tense(&out.join(" "), lexicon);
    let text = agree(&text, &speaker_ref, lexicon);
    let text = agree(&text, listener, lexicon);
    insert_that(&text)
}

/// `thinks User is ready` -> `thinks that User is ready`, `tells Greg User
/// is ready` -> `tells Greg that User is ready`. Only when what follows looks
/// like a clause subject: a capitalized word or a subject pronoun, plus a
/// determiner after verbs that never take a plain object (`says the idea is
/// stupid`). `knows the answer` and `promises a visit` are left alone.
fn insert_that(text: &str) -> String {
    const ATTITUDE: &[&str] =
        &["thinks", "believes", "knows", "says", "means", "assumes", "claims", "hopes", "promises", "swears", "insists", "reports"];
    const CLAUSE_ONLY: &[&str] = &["thinks", "believes", "says", "means", "assumes", "claims", "hopes", "swears", "insists"];
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<&str> = vec![];
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        out.push(word);
        let mut next = i + 1;
        if word == "tells" || word == "told" {
            // Skip the addressee: `Greg`, `User's father`.
            if words.get(next).is_some_and(|w| starts_upper(w)) {
                next += 1;
                if words.get(next - 1).is_some_and(|w| w.ends_with("'s")) {
                    next += 1;
                }
                out.extend(&words[i + 1..next.min(words.len())]);
            } else {
                i += 1;
                continue;
            }
        } else if !ATTITUDE.contains(&word) {
            i += 1;
            continue;
        }
        if let Some(following) = words.get(next) {
            let subject_like = starts_upper(following)
                || matches!(*following, "she" | "he" | "they" | "it")
                || (CLAUSE_ONLY.contains(&word) && matches!(*following, "the" | "a" | "an" | "every" | "no" | "some"));
            if subject_like && *following != "that" {
                out.push("that");
            }
        }
        i = next;
    }
    out.join(" ")
}

fn starts_upper(word: &str) -> bool {
    word.starts_with(|c: char| c.is_uppercase())
}

/// `an unknown woman` said it; inside the quote she is `the unknown woman`.
fn definite(speaker: &str) -> String {
    speaker
        .strip_prefix("an ")
        .or_else(|| speaker.strip_prefix("a "))
        .map(|rest| format!("the {rest}"))
        .unwrap_or_else(|| speaker.to_string())
}

/// `IDEA` -> `idea`, `JOHN` -> `John`, `Sarah` -> `Sarah`, `And` -> `and`.
/// Unknown capitalized words are names and keep their capital.
fn settle_case(word: &str, lexicon: &Lexicon) -> String {
    let lower = word.to_lowercase();
    if let Some(name) = lexicon.canonical_name(word) {
        return name.to_string();
    }
    let all_caps = word.len() > 1 && word.chars().all(|c| !c.is_alphabetic() || c.is_uppercase());
    if all_caps {
        // Emphasis on a known word settles; an unknown all-caps word is an acronym (`HR`).
        return if lexicon.is_known(&lower) { lower } else { word.to_string() };
    }
    if word.starts_with(|c: char| c.is_uppercase()) && lexicon.is_known(&lower) {
        return lower;
    }
    word.to_string()
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex() -> Lexicon {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let seed = manifest.parent().unwrap().parent().unwrap().join("data/seed");
        Lexicon::load_seed_dir(&seed).expect("seed lexicon")
    }

    fn unwrap(body: &str) -> (String, String) {
        unwrap_reported(body, &lex()).unwrap_or_else(|| panic!("no unwrap for {body:?}"))
    }

    #[test]
    fn told_me_becomes_tells_user_with_the_speaker_in_first_person() {
        let (sentence, rest) = unwrap(r#"Jake tells User "Sarah said Bob hates me" but Sarah swears she never says anything"#);
        assert_eq!(sentence, "Jake tells User that Sarah says that Bob hates Jake.");
        assert_eq!(rest, "Sarah swears she never says anything");
    }

    #[test]
    fn said_becomes_says_and_you_is_the_listener() {
        let (sentence, rest) = unwrap(r#"Greg says "Ian told me Dheeraj thinks you are ready""#);
        assert_eq!(sentence, "Greg says that Ian tells Greg that Dheeraj thinks that User is ready.");
        assert_eq!(rest, "");
        let (sentence, _) = unwrap(r#"John tells Sarah "Mary said she would call you""#);
        assert_eq!(sentence, "John tells Sarah that Mary says that she would call Sarah.");
    }

    #[test]
    fn emphasis_capitals_settle_and_openers_drop() {
        let (sentence, _) = unwrap(r#"Mike goes "dude Emma told me you said I was an idiot""#);
        assert_eq!(sentence, "Mike says that Emma tells Mike that User says that Mike is an idiot.");
        let (sentence, _) = unwrap(r#"User is like "what? I said the IDEA was stupid not you""#);
        assert_eq!(sentence, "User says that User says that the idea is stupid not you.");
        let (sentence, _) = unwrap(r#"Mary tells User "I meant I would call JOHN""#);
        assert_eq!(sentence, "Mary tells User that Mary means that Mary would call John.");
    }

    #[test]
    fn kin_pronoun_and_lowercased_speakers() {
        let (sentence, _) = unwrap(r#"User's boss tells User "I never told HR to fire him""#);
        assert_eq!(sentence, "User's boss tells User that User's boss never tells HR to fire him.");
        let (sentence, _) = unwrap(r#"dad says "your mom told me you promised""#);
        assert_eq!(sentence, "User's father says that User's mother tells User's father that User promises.");
        let (sentence, _) = unwrap(r#"she says "he told her I was mad at him""#);
        assert_eq!(sentence, "an unknown woman says that he tells her the unknown woman is mad at him.");
        let (sentence, _) = unwrap(r#"greg says "Bob hates me""#);
        assert_eq!(sentence, "Greg says that Bob hates Greg.");
    }

    #[test]
    fn nested_quotes_are_kept_verbatim() {
        let (sentence, _) = unwrap(r#"Sarah tells User "Mike said 'tell John I do not want his help'""#);
        assert_eq!(sentence, "Sarah tells User that Mike says 'tell John I do not want his help'.");
    }

    #[test]
    fn questions_and_non_reports_are_left_alone() {
        let lex = lex();
        assert_eq!(unwrap_reported(r#"Tom asks "is Jerry helping Tom""#, &lex), None);
        assert_eq!(unwrap_reported(r#"Bob says "who is she""#, &lex), None);
        assert_eq!(unwrap_reported("John owns a dog", &lex), None);
        assert_eq!(unwrap_reported(r#"it says "hello""#, &lex), None);
    }
}
