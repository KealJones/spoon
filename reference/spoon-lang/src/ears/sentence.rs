//! Sentence-level rewrite rules: the SCE shapes one sentence takes once the
//! text-level rules in `rules` have run and the text is split into sentences.
//!
//! Every rule is a pure string rewrite on a sentence body without its
//! terminator: greetings split off, indirect requests and imperatives become
//! commands, feelings become copula states, corrections and definitions take
//! their fixed shapes, and questions get their `?` back.

use regex::Regex;
use std::sync::OnceLock;

use crate::ears::lexicon::Lexicon;
use crate::ears::loops::loop_rewrite;
use crate::ears::reported::unwrap_reported;
use crate::ears::rules::{is_verbish, strip_discourse, third_person};
use crate::ears::values::{spot_values, SlotKind};
use crate::sce::{pluralize_noun, singularize_noun};

macro_rules! regex {
    ($pat:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pat).expect("static regex"))
    }};
}

/// Apply the sentence rules to one sentence body (no terminator) and return
/// the finished sentence(s) with terminators. `term` is the terminator the
/// user gave (or `.` when there was none).
pub fn apply_sentence_rules(body: &str, term: char, lexicon: &Lexicon) -> Vec<String> {
    let body = strip_discourse(body.trim().trim_end_matches(',').trim());
    if body.is_empty() {
        return vec![];
    }

    if let Some((greeting, rest)) = split_greeting(body, lexicon) {
        let mut out = vec![greeting];
        out.extend(apply_sentence_rules(&rest, term, lexicon));
        return out;
    }
    if let Some((report, rest)) = unwrap_reported(body, lexicon) {
        let mut out = vec![capitalize_opener(&report)];
        out.extend(apply_sentence_rules(&rest, term, lexicon));
        return out;
    }
    if let Some(sentences) = loop_rewrite(body, lexicon) {
        return sentences;
    }
    if let Some(universal) = plural_universal(body, lexicon) {
        return vec![universal];
    }
    if let Some(question) = embedded_question(body) {
        return vec![capitalize_opener(&format!("{question}?"))];
    }
    if let Some(question) = retrieval_question(body) {
        return vec![question];
    }
    if let Some(command) = indirect_request(body) {
        return vec![command];
    }
    if let Some(command) = imperative(body, lexicon) {
        return vec![command];
    }
    if let Some(feeling) = feeling(body) {
        return vec![feeling];
    }
    if let Some(correction) = correction(body) {
        return vec![correction];
    }
    if let Some(definition) = definition(body) {
        return vec![definition];
    }
    if let Some(calc) = arithmetic_question(body) {
        return vec![calc];
    }
    if let Some(question) = copula_less_question(body, lexicon) {
        return vec![question];
    }

    let term = if term == '.' && is_question_start(body) { '?' } else { term };
    vec![capitalize_opener(&format!("{body}{term}"))]
}

/// `Assistant good` (from `you good`) and `John happy` are questions with the
/// copula dropped: `Is Assistant good?`. Only a subject followed by one seed
/// adjective qualifies.
fn copula_less_question(body: &str, lexicon: &Lexicon) -> Option<String> {
    let (subject, adj) = body.split_once(' ')?;
    let subject_ok = matches!(subject, "User" | "Assistant") || lexicon.canonical_name(subject).is_some();
    if !subject_ok || !adj.chars().all(|c| c.is_ascii_lowercase()) || !lexicon.is_adjective(adj) {
        return None;
    }
    Some(format!("Is {subject} {adj}?"))
}

/// A bare plural subject is a universal: `wolves are white` -> `Every wolf
/// is white.`, `dogs are animals` -> `Every dog is an animal.`, `cats have
/// tails` -> `Every cat has a tail.`, `dogs are not cats` -> `No dog is a
/// cat.` Only a seed plural in subject position with a copula, `have`, or a
/// base-form verb qualifies; the object's bare plural becomes `a N`.
fn plural_universal(body: &str, lexicon: &Lexicon) -> Option<String> {
    let (subject, rest) = body.split_once(' ')?;
    if !subject.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    let singular = lexicon.singular(subject)?;
    let (verb, object) = rest.split_once(' ').unwrap_or((rest, ""));
    let (quantifier, verb) = match verb {
        "are" if object.starts_with("not ") => ("No", "is".to_string()),
        "are" => ("Every", "is".to_string()),
        "have" | "own" => ("Every", "has".to_string()),
        v if lexicon.is_base_verb(v) && !lexicon.is_noun(v) => ("Every", third_person(v)),
        _ => return None,
    };
    let object = object.strip_prefix("not ").unwrap_or(object);
    let object = match object.split_once(' ').unwrap_or((object, "")) {
        ("", _) => String::new(),
        (head, tail) => match lexicon.singular(head) {
            Some(s) => format!(" {} {s}{}", article(&s), if tail.is_empty() { String::new() } else { format!(" {tail}") }),
            None => format!(" {object}"),
        },
    };
    Some(format!("{quantifier} {singular} {verb}{object}."))
}

fn article(noun: &str) -> &'static str {
    if noun.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" }
}

/// `hello what is up` -> (`hello.`, `what is up`). A greeting addressed by
/// name (`hey Spoon`) has no remainder to keep.
fn split_greeting(body: &str, lexicon: &Lexicon) -> Option<(String, String)> {
    let re = regex!(
        r"(?i)^(?:hello|hi|hey|yo|sup|hiya|heya|howdy|greetings|good (?:morning|afternoon|evening))(?:\s+(?:there|again|everyone|all|guys|man|dude|girl|friend|friends))*[,!.]?\s+(.+)$"
    );
    let caps = re.captures(body)?;
    let rest = caps[1].trim();
    let single = rest.split_whitespace().count() == 1;
    let addressee = single
        && (rest.eq_ignore_ascii_case("assistant")
            || rest.eq_ignore_ascii_case("spoon")
            || lexicon.canonical_name(rest).is_some());
    let rest = if addressee { String::new() } else { rest.to_string() };
    Some(("hello.".to_string(), rest))
}

/// `can Assistant tell User what the name of Assistant is` -> `what is the name of Assistant`.
fn embedded_question(body: &str) -> Option<String> {
    let re = regex!(
        r"(?i)^(?:Assistant,?\s+)?(?:can|could|would|will)\s+Assistant\s+(?:please\s+)?(?:tell|remind|show)\s+User\s+(?:about\s+)?((?:what|who|where|when|which|how many|how)\s+.+)$"
    );
    let caps = re.captures(body)?;
    let q = caps[1].trim().to_string();
    let inv = regex!(r"(?i)^(what|who|where|when|which)\s+(.+?)\s+(is|are)$");
    Some(match inv.captures(&q) {
        Some(c) => format!("{} is {}", &c[1], &c[2]),
        None => q,
    })
}

/// `get me the titles from X` is a question about X, not an errand. The
/// beneficiary (`me`, already grounded to `User`) is dropped rather than
/// becoming an argument, and the whole family lands on the property question
/// the interior can actually plan: `What are the titles of X?`
///
/// `can you get me all the titles from <url>` -> `What are the titles of <url>?`
/// `get me the title of <url>`               -> `What is the title of <url>?`
fn retrieval_question(body: &str) -> Option<String> {
    let re = regex!(
        r"(?ix)
        ^ (?:Assistant,?\s+)?
          (?:(?:can|could|would|will|please)\s+)?
          (?:Assistant\s+)?
          (?:please\s+)?
          (?:get|fetch|grab|pull|show|give|list|read|display|retrieve)\s+
          (?:User\s+)?
          (.+?)
          (?:\s+for\s+User)?
          \s+(?:from|of|in|inside|within|out\s+of)\s+
          (\S+)
        $"
    );
    let caps = re.captures(body)?;
    let (noun, plural) = property_noun(&caps[1])?;
    let owner = caps[2].trim();
    if owner.is_empty() || owner.eq_ignore_ascii_case("User") || owner.eq_ignore_ascii_case("Assistant") {
        return None;
    }
    Some(match plural {
        true => format!("What are the {} of {owner}?", pluralize_noun(&noun)),
        false => format!("What is the {noun} of {owner}?"),
    })
}

/// Reduce the thing being asked for to one noun plus whether it was plural.
/// `all the titles` -> (`titles`, true), `all of the "title" properties` ->
/// (`title`, true), `the title` -> (`title`, false).
fn property_noun(phrase: &str) -> Option<(String, bool)> {
    let mut words: Vec<String> = phrase.split_whitespace().map(str::to_lowercase).collect();
    let mut plural = false;

    // Leading quantifiers and determiners. `all` is the plural marker.
    while let Some(first) = words.first().map(String::as_str) {
        match first {
            "all" | "both" => plural = true,
            "the" | "a" | "an" | "of" | "its" | "their" | "any" => {}
            _ => break,
        }
        words.remove(0);
    }
    // A trailing container word (`properties`, `values`, `fields`) names the
    // shape, not the property: `"title" properties` is about `title`.
    if let Some(last) = words.last().map(String::as_str) {
        let container = matches!(
            last,
            "property" | "properties" | "value" | "values" | "field" | "fields" | "key" | "keys" | "entry" | "entries"
        );
        if container && words.len() > 1 {
            plural = plural || last != singularize_noun(last);
            words.pop();
        }
    }
    let [noun] = &words[..] else { return None };
    let noun = noun.trim_matches(['"', '\'']).to_string();
    if noun.is_empty() || !noun.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
        return None;
    }
    Some((noun.clone(), plural || singularize_noun(&noun) != noun))
}

/// `can Assistant double 21 for User` / `please reverse "abc"` / `User want
/// Assistant to X` -> `Assistant, X!`.
fn indirect_request(body: &str) -> Option<String> {
    let modal = regex!(r"(?i)^(?:Assistant,?\s+)?(?:can|could|would|will|might)\s+Assistant\s+(?:please\s+)?(.+)$");
    let please_first = regex!(r"(?i)^please\s+(.+)$");
    let please_last = regex!(r"(?i)^(.+?),?\s+please$");
    let want = regex!(r"(?i)^User\s+(?:want|wants|need|needs|would like|would love)\s+Assistant\s+to\s+(.+)$");
    let inner = [&modal, &want, &please_first, &please_last]
        .iter()
        .find_map(|re| re.captures(body).map(|c| c[1].to_string()))?;
    let mut x = inner.trim().to_string();
    x = regex!(r"(?i)\s+for (?:User|me)$").replace(&x, "").into_owned();
    x = regex!(r"(?i)\bplease\b").replace_all(&x, "").into_owned();
    x = regex!(r"(?i)^(?:kindly|just)\s+").replace(&x, "").into_owned();
    x = regex!(r"(?i)^not\s+").replace(&x, "do not ").into_owned();
    let x = x.split_whitespace().collect::<Vec<_>>().join(" ");
    if x.is_empty() || x.eq_ignore_ascii_case("do not") {
        return None;
    }
    // `divide 100 by 4 please` has already become `100 / 4 please`.
    if is_whole_arith(&x) {
        return Some(format!("Assistant, calculate {x}!"));
    }
    Some(format!("Assistant, {x}!"))
}

fn is_whole_arith(text: &str) -> bool {
    spot_values(text).iter().any(|s| s.kind == SlotKind::Arith && s.start == 0 && s.end == text.len())
}

/// A bare imperative is a command: `reverse "hello"` -> `Assistant, reverse
/// "hello"!`, `double 21` -> `Assistant, double 21!`. The first word must be a
/// known verb, or an unknown lowercase word whose whole argument is one value
/// (a number, an expression, a quoted string): that is the shape a new verb
/// arrives in for the learner.
fn imperative(body: &str, lexicon: &Lexicon) -> Option<String> {
    let (first, rest) = body.split_once(' ')?;
    let rest = rest.trim();
    if rest.is_empty() || !first.chars().all(|c| c.is_ascii_lowercase()) || is_question_start(body) {
        return None;
    }
    if matches!(first, "user" | "assistant" | "not" | "no" | "yes" | "ok" | "okay") {
        return None;
    }
    let spots = spot_values(rest);
    let single_value = spots.iter().any(|s| s.start == 0 && s.end == rest.len());
    let rest_is_clause = is_verbish(rest.split_whitespace().next().unwrap_or(""), lexicon);
    // Adjectives pass: `double 21` is a verb use of a word the seed lists as an adjective.
    let can_open = lexicon.is_verb(first) || !lexicon.is_non_verb(first);
    if (lexicon.is_verb(first) && !rest_is_clause) || (can_open && single_value) {
        return Some(format!("Assistant, {first} {rest}!"));
    }
    None
}

/// `User is feeling kind of down today` -> `User is sad.` SCE only has the
/// copula for states, so feel/feeling collapse onto `is` and the synonym table
/// maps slang states onto the adjectives the interior knows.
fn feeling(body: &str) -> Option<String> {
    let re = regex!(
        r"(?i)^User\s+(?:is|am|are|feel|feels)(?:\s+feeling)?(?:\s+(?:kind of|sort of|so|really|very|pretty|a bit|a little|quite|super|extremely|honestly|just|too|rather|somewhat|slightly|totally|low|lowkey))*\s+([a-z][a-z-]*)(?:\s+(?:today|right now|now|lately|these days|tonight|this morning|at the moment|currently|again|still|tbh))?$"
    );
    let caps = re.captures(body)?;
    let raw = &caps[1];
    // Names are not states: `User is John` stays as it is.
    if !raw.chars().next().is_some_and(|c| c.is_lowercase()) {
        return None;
    }
    let adj = raw.to_lowercase();
    if matches!(
        adj.as_str(),
        "like" | "not" | "a" | "an" | "the" | "going" | "here" | "there" | "on" | "in" | "at" | "to" | "up" | "out"
            | "off" | "done" | "back" | "about" | "with" | "all" | "no" | "so" | "kind" | "sort" | "feeling"
    ) {
        return None;
    }
    let adj = match adj.as_str() {
        "down" | "low" | "blue" | "bummed" | "depressed" | "gloomy" | "unhappy" | "miserable" => "sad",
        "pumped" | "stoked" | "hyped" | "thrilled" | "psyched" => "excited",
        "wiped" | "beat" | "exhausted" | "drained" | "sleepy" | "knackered" => "tired",
        "mad" | "furious" | "pissed" | "livid" => "angry",
        "glad" | "cheerful" | "joyful" => "happy",
        "worried" | "uneasy" => "anxious",
        other => other,
    };
    Some(format!("User is {adj}."))
}

/// `no User meant Mary` / `no, Mary` -> `User means Mary.`
fn correction(body: &str) -> Option<String> {
    let meant = regex!(r"^(?i:no,?\s+)?User\s+(?i:mean|means|meant)\s+([A-Z][A-Za-z-]*)$");
    let bare = regex!(r"^(?i:no),?\s+([A-Z][A-Za-z-]*)$");
    let name = meant
        .captures(body)
        .or_else(|| bare.captures(body))
        .map(|c| c[1].to_string())?;
    if name == "User" || name == "Assistant" {
        return None;
    }
    Some(format!("User means {name}."))
}

/// `pup means dog` -> `"pup" means "dog".` (two bare words, no names).
fn definition(body: &str) -> Option<String> {
    let re = regex!(r"^([a-z][a-z-]*)\s+means\s+([a-z][a-z-]*)$");
    let caps = re.captures(body)?;
    let (x, y) = (&caps[1], &caps[2]);
    let banned = ["that", "this", "it", "what", "which", "user", "assistant", "the", "a", "an"];
    if banned.contains(&x) || banned.contains(&y) {
        return None;
    }
    Some(format!("\"{x}\" means \"{y}\"."))
}

/// `what is 3 * 4` / `calculate 3 * 4` -> `Assistant, calculate 3 * 4!`.
/// Only fires when the remainder is one whole arithmetic expression; the
/// numbers pass through verbatim.
fn arithmetic_question(body: &str) -> Option<String> {
    let re = regex!(r"(?i)^(?:(?:what is|calculate|compute|evaluate)\s+)?(.+)$");
    let expr = re.captures(body)?[1].trim().to_string();
    if !is_whole_arith(&expr) {
        return None;
    }
    Some(format!("Assistant, calculate {expr}!"))
}

fn is_question_start(body: &str) -> bool {
    let first = body.split_whitespace().next().unwrap_or("").to_lowercase();
    matches!(
        first.as_str(),
        "who" | "what" | "where" | "when" | "which" | "why" | "how" | "is" | "are" | "does" | "do" | "did"
            | "can" | "could" | "should" | "will" | "would" | "may" | "must" | "am" | "was" | "were"
    )
}

/// Capitalize a sentence-initial SCE keyword. Unknown first words stay
/// lowercase on purpose: a manufactured capital turns them into Names.
pub fn capitalize_opener(sentence: &str) -> String {
    let first = sentence.split_whitespace().next().unwrap_or("");
    let core = first.trim_end_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let opener = matches!(
        core.as_str(),
        "a" | "an" | "the" | "every" | "no" | "some" | "not" | "at" | "exactly" | "more" | "if" | "for"
            | "there" | "it" | "who" | "what" | "which" | "where" | "when" | "how" | "why" | "does" | "do"
            | "did" | "is" | "are" | "am" | "was" | "were" | "can" | "cannot" | "could" | "should" | "will"
            | "would" | "must" | "may" | "user" | "assistant" | "somebody" | "someone" | "nobody" | "everybody"
            | "everyone" | "anyone"
    );
    if !opener {
        return sentence.to_string();
    }
    let mut chars = sentence.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ears::lexicon::Lexicon;

    fn lex() -> Lexicon {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let seed = manifest.parent().unwrap().parent().unwrap().join("data/seed");
        let mut lex = Lexicon::load_seed_dir(&seed).expect("seed lexicon");
        lex.add_names(&["John", "Mary", "Bob"]);
        lex
    }

    fn rules(body: &str) -> Vec<String> {
        apply_sentence_rules(body, '.', &lex())
    }

    #[test]
    fn imperatives_become_commands() {
        assert_eq!(rules("reverse \"hello\""), vec!["Assistant, reverse \"hello\"!"]);
        assert_eq!(rules("double 21"), vec!["Assistant, double 21!"]);
        assert_eq!(rules("save the file"), vec!["Assistant, save the file!"]);
        // A noun subject or a clause after the first word is not an imperative.
        assert_eq!(rules("dogs bark"), vec!["dogs bark."]);
        assert_eq!(rules("pup means dog"), vec!["\"pup\" means \"dog\"."]);
    }

    #[test]
    fn question_restoration() {
        assert_eq!(rules("who owns a dog"), vec!["Who owns a dog?"]);
        assert_eq!(rules("is John happy"), vec!["Is John happy?"]);
        assert_eq!(rules("does Mary own a cat"), vec!["Does Mary own a cat?"]);
        assert_eq!(rules("what is the double of 100"), vec!["What is the double of 100?"]);
        // Not a question: keeps its period, keyword capitalized.
        assert_eq!(rules("every dog is an animal"), vec!["Every dog is an animal."]);
    }

    #[test]
    fn indirect_requests_become_commands() {
        assert_eq!(rules("can Assistant double 21 for User"), vec!["Assistant, double 21!"]);
        assert_eq!(rules("please reverse \"abc\""), vec!["Assistant, reverse \"abc\"!"]);
        assert_eq!(rules("reverse \"abc\" please"), vec!["Assistant, reverse \"abc\"!"]);
        assert_eq!(rules("User want Assistant to save the file"), vec!["Assistant, save the file!"]);
        assert_eq!(rules("could Assistant please open the door"), vec!["Assistant, open the door!"]);
    }

    #[test]
    fn embedded_questions_surface() {
        assert_eq!(
            rules("can Assistant tell User what the name of Assistant is"),
            vec!["What is the name of Assistant?"]
        );
        assert_eq!(rules("could Assistant tell User who owns a dog"), vec!["Who owns a dog?"]);
    }

    #[test]
    fn feelings_become_copula_states() {
        assert_eq!(rules("User is feeling kind of down today"), vec!["User is sad."]);
        assert_eq!(rules("User feel so pumped right now"), vec!["User is excited."]);
        assert_eq!(rules("User is exhausted"), vec!["User is tired."]);
        assert_eq!(rules("User is anxious"), vec!["User is anxious."]);
        // `i feel like X` is an opinion, not a state: left for the LLM.
        assert_eq!(rules("User feel like giving up"), vec!["User feel like giving up."]);
        assert_eq!(rules("User is a doctor"), vec!["User is a doctor."]);
        assert_eq!(rules("User is not happy"), vec!["User is not happy."]);
    }

    #[test]
    fn corrections_and_definitions() {
        assert_eq!(rules("no User meant Mary"), vec!["User means Mary."]);
        assert_eq!(rules("User meant Mary"), vec!["User means Mary."]);
        assert_eq!(rules("no, Mary"), vec!["User means Mary."]);
        assert_eq!(rules("pup means dog"), vec!["\"pup\" means \"dog\"."]);
        assert_eq!(rules("User means Mary"), vec!["User means Mary."]);
    }

    #[test]
    fn arithmetic_questions_become_calculate() {
        assert_eq!(rules("what is 3 * 4"), vec!["Assistant, calculate 3 * 4!"]);
        assert_eq!(rules("calculate 3 / 500 * 3600"), vec!["Assistant, calculate 3 / 500 * 3600!"]);
        // A single number is not an expression.
        assert_eq!(rules("what is 42"), vec!["What is 42?"]);
    }

    #[test]
    fn greetings_split_off() {
        assert_eq!(rules("hello what is up"), vec!["hello.", "What is up?"]);
        assert_eq!(rules("hello John is home"), vec!["hello.", "John is home."]);
        assert_eq!(rules("hello Spoon"), vec!["hello."]);
        assert_eq!(rules("hello"), vec!["hello."]);
    }

    #[test]
    fn copula_less_and_whole_arithmetic() {
        assert_eq!(rules("Assistant good"), vec!["Is Assistant good?"]);
        assert_eq!(rules("John happy"), vec!["Is John happy?"]);
        assert_eq!(rules("John dog"), vec!["John dog."]);
        assert_eq!(rules("100 / 4 please"), vec!["Assistant, calculate 100 / 4!"]);
        assert_eq!(rules("100 / 4"), vec!["Assistant, calculate 100 / 4!"]);
    }
}
