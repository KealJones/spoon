//! Deterministic rewrite rules: the SCE shapes that messy English never has.
//!
//! Every rule is a pure string rewrite. Nothing here evaluates, guesses a
//! meaning, or consults an LLM. These are the text-level rules `normalize`
//! runs on the whole grounded text (contractions, agreement, tense,
//! coordination, operator words). The per-sentence rules (greetings,
//! requests, feelings, corrections, questions) live in `sentence`.

use regex::Regex;
use std::sync::OnceLock;

use crate::ears::lexicon::Lexicon;
use crate::ears::values::{spot_values, SlotKind};

macro_rules! regex {
    ($pat:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pat).expect("static regex"))
    }};
}

// ---- text-level rules ----

/// Expand English contractions so every later rule sees full words:
/// `whats` / `what's` -> `what is`, `i'm` -> `i am`, `don't` -> `do not`.
pub fn expand_contractions(text: &str) -> String {
    let re = regex!(
        r"(?i)\b(what's|whats|how's|hows|where's|wheres|who's|whos|when's|that's|thats|there's|theres|here's|it's|i'm|im|you're|youre|we're|they're|theyre|he's|hes|she's|shes|i've|ive|you've|youve|we've|they've|i'd|you'd|i'll|you'll|youll|we'll|they'll|it'll|don't|dont|doesn't|doesnt|didn't|didnt|can't|cant|cannot|won't|wont|wouldn't|wouldnt|couldn't|couldnt|shouldn't|shouldnt|isn't|isnt|aren't|arent|wasn't|wasnt|weren't|werent|haven't|havent|hasn't|hasnt|hadn't|hadnt|ain't|let's|lets)\b"
    );
    re.replace_all(text, |caps: &regex::Captures| {
        let word = caps[1].to_lowercase().replace('\'', "");
        let expanded = match word.as_str() {
            "whats" => "what is",
            "hows" => "how is",
            "wheres" => "where is",
            "whos" => "who is",
            "whens" => "when is",
            "thats" => "that is",
            "theres" => "there is",
            "heres" => "here is",
            "its" => "it is",
            "im" => "i am",
            "youre" => "you are",
            "were" => "we are",
            "theyre" => "they are",
            "hes" => "he is",
            "shes" => "she is",
            "ive" => "i have",
            "youve" => "you have",
            "weve" => "we have",
            "theyve" => "they have",
            "id" => "i would",
            "youd" => "you would",
            "ill" => "i will",
            "youll" => "you will",
            "well" => "we will",
            "theyll" => "they will",
            "itll" => "it will",
            "dont" => "do not",
            "doesnt" => "does not",
            "didnt" => "did not",
            "cant" | "cannot" => "cannot",
            "wont" => "will not",
            "wouldnt" => "would not",
            "couldnt" => "could not",
            "shouldnt" => "should not",
            "isnt" | "aint" => "is not",
            "arent" => "are not",
            "wasnt" => "was not",
            "werent" => "were not",
            "havent" => "have not",
            "hasnt" => "has not",
            "hadnt" => "had not",
            "lets" => "let us",
            _ => return caps[0].to_string(),
        };
        expanded.to_string()
    })
    .into_owned()
}

/// `User` and `Assistant` are singular names, so the copula and do-support
/// they inherited from `I`/`you` must agree: `User am` -> `User is`,
/// `do Assistant think` -> `does Assistant think`, `User do not` -> `User does not`.
pub fn fix_agreement(text: &str) -> String {
    let mut s = regex!(r"\bUser am\b").replace_all(text, "User is").into_owned();
    s = regex!(r"\b(User|Assistant) are\b").replace_all(&s, "$1 is").into_owned();
    s = regex!(r"\bam User\b").replace_all(&s, "is User").into_owned();
    s = regex!(r"\bare (User|Assistant)\b").replace_all(&s, "is $1").into_owned();
    s = regex!(r"\b(User|Assistant) do not\b").replace_all(&s, "$1 does not").into_owned();
    s = regex!(r"(?i)(^|[.?!]\s+)(?:do|did) (User|Assistant)\b")
        .replace_all(&s, "${1}does $2")
        .into_owned();
    s = regex!(r"(?i)(^|[.?!]\s+)(what|who|where|when|which|how|why) (?:do|did) (User|Assistant)\b")
        .replace_all(&s, "${1}$2 does $3")
        .into_owned();
    s
}

/// A bare verb right after a clause-initial `User`/`Assistant` takes the third
/// person: `User own two cats` -> `User owns two cats`. Only verbs the lexicon
/// knows are touched, and only in subject position (`does User own` is left
/// alone because `does` precedes the subject).
pub fn conjugate_fixed_subjects(text: &str, lexicon: &Lexicon) -> String {
    let re = regex!(r"(^|[.?!]\s+|\b(?:and|then|if|when|because|but)\s+)(User|Assistant)\s+([a-z]+)\b");
    re.replace_all(text, |caps: &regex::Captures| {
        let verb = &caps[3];
        let auxiliary = matches!(
            verb,
            "is" | "are" | "was" | "were" | "has" | "have" | "does" | "do" | "did" | "can" | "cannot" | "could"
                | "should" | "must" | "may" | "will" | "would" | "am" | "not" | "meant"
        );
        if auxiliary || verb.ends_with('s') || !lexicon.is_verb(verb) {
            return caps[0].to_string();
        }
        format!("{}{} {}", &caps[1], &caps[2], third_person(verb))
    })
    .into_owned()
}

fn third_person(verb: &str) -> String {
    if verb.ends_with("sh") || verb.ends_with("ch") || verb.ends_with('x') || verb.ends_with('z') || verb.ends_with('o') || verb.ends_with('s') {
        format!("{verb}es")
    } else if verb.ends_with('y') && !verb.ends_with("ay") && !verb.ends_with("ey") && !verb.ends_with("oy") && !verb.ends_with("uy") {
        format!("{}ies", &verb[..verb.len() - 1])
    } else {
        format!("{verb}s")
    }
}

/// Possession with a determiner is `own`: `john has this dog` -> `john owns a
/// dog`, `User have two cats` -> `User owns two cats`. `have` as an auxiliary
/// (`have been`, `has to`) has no determiner after it and is left alone. A
/// narrative `this` after such a verb introduces something new, so it is `a`;
/// a clause-initial `this/that N` refers back, so it is `the N`.
pub fn possession(text: &str) -> String {
    let dets = r"a|an|the|this|that|these|those|some|no|\d+|one|two|three|four|five|six|seven|eight|nine|ten";
    let has = regex!(&format!(r"(?i)\b(has|have)\s+(?:got\s+)?({dets})\b"));
    let mut s = has
        .replace_all(text, |caps: &regex::Captures| {
            let verb = if caps[1].eq_ignore_ascii_case("has") { "owns" } else { "own" };
            format!("{verb} {}", &caps[2])
        })
        .into_owned();
    s = regex!(r"\b(owns|own|likes|like|wants|want|needs|need|got|sees|see|knows|know|buys|bought|gets|get)\s+this\s+")
        .replace_all(&s, "$1 a ")
        .into_owned();
    s = regex!(r"(?i)(^|[.?!]\s+)(?:this|that)\s+([a-z]+\s+(?:is|are|was|were|owns|has|likes|can|cannot|does|did|will|should|must))\b")
        .replace_all(&s, "${1}the $2")
        .into_owned();
    s
}

/// SCE is simple present: known past forms take the third person (`bought`
/// -> `buys`, `was` -> `is`). Regular `-ed` forms the seed does not list are
/// left for the LLM.
pub fn present_tense(text: &str, lexicon: &Lexicon) -> String {
    let mut in_quote = false;
    text.split(' ')
        .map(|word| {
            let quotes = word.matches('"').count();
            let inside = in_quote || quotes > 0;
            if quotes % 2 == 1 {
                in_quote = !in_quote;
            }
            if inside {
                return word.to_string();
            }
            let core = word.trim_end_matches(|c: char| !c.is_alphanumeric());
            let suffix = &word[core.len()..];
            let lower = core.to_lowercase();
            let present = match lower.as_str() {
                "was" => Some("is"),
                "were" => Some("are"),
                "did" => Some("does"),
                "had" => Some("has"),
                _ => lexicon.present_of.get(&lower).map(|s| s.as_str()),
            };
            match present {
                Some(p) => format!("{p}{suffix}"),
                None => word.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `all dogs are animals` -> `every dog is an animal`, `no cats are dogs` ->
/// `no cat is a dog`. Only plurals the seed lists are singularized.
pub fn singular_quantifiers(text: &str, lexicon: &Lexicon) -> String {
    let re = regex!(r"(?i)\b(all|every|each|no)\s+([a-z]+)\s+are\s+(?:(?:a|an|some)\s+)?([a-z]+)\b");
    re.replace_all(text, |caps: &regex::Captures| {
        let det = if caps[1].eq_ignore_ascii_case("all") { "every" } else { &caps[1] };
        let Some(subject) = lexicon.singular_of.get(&caps[2].to_lowercase()) else {
            return caps[0].to_string();
        };
        let object = &caps[3];
        match lexicon.singular_of.get(&object.to_lowercase()) {
            Some(single) => format!("{det} {subject} is {} {single}", article(single)),
            None => format!("{det} {subject} is {object}"),
        }
    })
    .into_owned()
}

fn article(noun: &str) -> &'static str {
    if noun.starts_with(|c: char| matches!(c, 'a' | 'e' | 'i' | 'o' | 'u')) { "an" } else { "a" }
}

/// Wh-word expletives carry no meaning: `where the hell is Bob` -> `where is Bob`.
pub fn strip_expletives(text: &str) -> String {
    let s = regex!(r"(?i)\b(who|what|where|when|which|why|how)\s+(?:the\s+(?:hell|heck|fuck|f)|on\s+earth|in\s+the\s+world|exactly|even)\b")
        .replace_all(text, "$1");
    // Swearing as an intensifier: `slow as shit` -> `very slow`, `this
    // fucking server` -> `this server`.
    let s = regex!(r"(?i)\b([a-z]+) as (?:shit|hell|fuck|balls|heck)\b").replace_all(&s, "very $1");
    regex!(r"(?i)\b(?:fucking|freaking|frickin|friggin|goddamn|damn|bloody|effing)\s+").replace_all(&s, "").into_owned()
}

/// Modal periphrases become SCE modals: `has to` -> `must`, `is allowed to`
/// -> `may`. Quantity idioms become SCE quantifiers: `no more than 2` -> `at
/// most 2`, `no less than 2` -> `at least 2`.
pub fn modals_and_quantities(text: &str) -> String {
    // `does not have to` has no SCE modal (`must not` means the opposite), so
    // the negated periphrasis is kept in its canonical form.
    let mut s = regex!(r"(?i)\b(not\s+)?(?:has|have|needs?|got|gotta|ought) to\b")
        .replace_all(text, |caps: &regex::Captures| if caps.get(1).is_some() { "not have to" } else { "must" })
        .into_owned();
    s = regex!(r"(?i)\bgotta\b").replace_all(&s, "must").into_owned();
    s = regex!(r"(?i)\b(?:is|are) (?:allowed|permitted) to\b").replace_all(&s, "may").into_owned();
    s = regex!(r"(?i)\b(?:is|are) able to\b").replace_all(&s, "can").into_owned();
    s = regex!(r"(?i)\bno more than\b").replace_all(&s, "at most").into_owned();
    s = regex!(r"(?i)\b(?:no|not) (?:less|fewer) than\b").replace_all(&s, "at least").into_owned();
    regex!(r"(?i)\b(\d+) or more\b").replace_all(&s, "at least $1").into_owned()
}

/// Progressive becomes simple present: `people are waiting` -> `people
/// wait`, `John is going` -> `John goes`. The stem must be a verb the seed
/// knows, so `is interesting` and `the building` are untouched.
pub fn progressive(text: &str, lexicon: &Lexicon) -> String {
    let re = regex!(r"(?i)\b(is|are)\s+([a-z]+ing)\b");
    re.replace_all(text, |caps: &regex::Captures| {
        let ing = caps[2].to_lowercase();
        let stem = &ing[..ing.len() - 3];
        // Participles that are really adjectives (`is boring`, `is missing`) stay.
        let adjectival = |c: &str| {
            matches!(
                c,
                "be" | "interest" | "bore" | "amaze" | "excite" | "annoy" | "miss" | "will" | "charm" | "disappoint"
                    | "confuse" | "frustrate" | "tire" | "surprise" | "encourage" | "overwhelm" | "please" | "outstand"
            )
        };
        let lemma = [stem.to_string(), format!("{stem}e"), stem[..stem.len().saturating_sub(1)].to_string()]
            .into_iter()
            .find(|c| c.len() >= 2 && lexicon.verb_lemmas.contains(c.as_str()) && !adjectival(c));
        match lemma {
            Some(lemma) if caps[1].eq_ignore_ascii_case("is") => third_person(&lemma),
            Some(lemma) => lemma,
            None => caps[0].to_string(),
        }
    })
    .into_owned()
}

/// A self-correction replaces what came before it: `john owns a dog wait no
/// sorry mary owns the dog` -> `mary owns the dog`.
pub fn self_correction(text: &str) -> String {
    let re = regex!(r"(?i)^.*\b(?:wait,? no|no,? wait|wait,? sorry|no,? sorry|sorry,? no|actually,? no|scratch that|i mean)[,\s]+(.+)$");
    match re.captures(text) {
        Some(caps) => caps[1].to_string(),
        None => text.to_string(),
    }
}

/// Sentence-initial discourse markers: `yeah but User is not sleepy` ->
/// `User is not sleepy`. `no` is kept: it is a quantifier and a correction.
pub fn strip_discourse(body: &str) -> &str {
    let re = regex!(r"(?i)^(?:yeah|yes|yep|yup|nah|okay|ok|well|anyway|alright|but|and|so|oh|ah|hmm|um|uh)(?:[,\s]+(?:yeah|but|so|and|like|basically|anyway|then|ok|okay))*[,\s]+(.+)$");
    match re.captures(body) {
        Some(caps) => body[caps.get(1).unwrap().start()..].trim(),
        None => body,
    }
}

/// `its raining` -> `it is raining` (the possessive `its dog` is untouched),
/// then weather-it becomes a state of the weather: `it is raining` -> `the
/// weather is rainy`.
pub fn weather(text: &str) -> String {
    let s = regex!(r"(?i)\bits\s+(raining|snowing|sunny|cold|hot|windy|cloudy|rainy|snowy|warm|a|an|the|not|so|too|very|really)\b")
        .replace_all(text, "it is $1")
        .into_owned();
    let s = regex!(r"(?i)\bit\s+(?:is|was)\s+(raining|snowing|sunny|cold|hot|windy|cloudy|rainy|snowy|warm)\b")
        .replace_all(&s, |caps: &regex::Captures| {
            let state = match caps[1].to_lowercase().as_str() {
                "raining" => "rainy".to_string(),
                "snowing" => "snowy".to_string(),
                other => other.to_string(),
            };
            format!("the weather is {state}")
        })
        .into_owned();
    regex!(r"(?i)\bit\s+(rains|snows)\b")
        .replace_all(&s, |caps: &regex::Captures| {
            let state = if caps[1].eq_ignore_ascii_case("rains") { "rainy" } else { "snowy" };
            format!("the weather is {state}")
        })
        .into_owned()
}

/// Arithmetic verbs become operators: `divide 100 by 4` -> `100 / 4`,
/// `multiply 3 by 4` -> `3 * 4`, `add 2 and 3` -> `2 + 3`, `subtract 2 from
/// 10` -> `10 - 2`. Nothing is evaluated.
pub fn arithmetic_verbs(text: &str) -> String {
    let num = r"(\d[\d.]*|\([^()]*\))";
    let mut s = regex!(&format!(r"(?i)\bdivide\s+{num}\s+by\s+{num}")).replace_all(text, "$1 / $2").into_owned();
    s = regex!(&format!(r"(?i)\bmultiply\s+{num}\s+(?:by|and|with)\s+{num}")).replace_all(&s, "$1 * $2").into_owned();
    s = regex!(&format!(r"(?i)\badd\s+{num}\s+(?:and|to|plus)\s+{num}")).replace_all(&s, "$1 + $2").into_owned();
    s = regex!(&format!(r"(?i)\bsubtract\s+{num}\s+from\s+{num}")).replace_all(&s, "$2 - $1").into_owned();
    s = regex!(&format!(r"(?i)\b(?:the\s+)?sum\s+of\s+{num}\s+and\s+{num}")).replace_all(&s, "($1 + $2)").into_owned();
    s = regex!(&format!(r"(?i)\b(?:the\s+)?product\s+of\s+{num}\s+and\s+{num}")).replace_all(&s, "($1 * $2)").into_owned();
    s
}

/// Number words become digits when they count something: `two cats` -> `2
/// cats`. `one` is left alone (`the one`, `no one`, `one of`).
pub fn digits(text: &str, lexicon: &Lexicon) -> String {
    let words: Vec<&str> = text.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    for (i, word) in words.iter().enumerate() {
        let lower = word.to_lowercase();
        let counts_something = words.get(i + 1).is_some_and(|next| next.starts_with(|c: char| c.is_alphabetic()));
        match lexicon.number_words.get(&lower) {
            Some(n) if counts_something && lower != "one" && lower != "a" && n.fract() == 0.0 => {
                out.push((*n as i64).to_string());
            }
            _ => out.push(word.to_string()),
        }
    }
    out.join(" ")
}

/// `the word hello` / `the string abc` -> `"hello"` / `"abc"`.
pub fn quote_mentions(text: &str) -> String {
    regex!(r#"(?i)\bthe (?:word|string|text|phrase) ([a-z][a-z0-9-]*)\b"#)
        .replace_all(text, "\"$1\"")
        .into_owned()
}

/// Split `S1 and S2` into two sentences when S2 starts like a new clause
/// (a subject noun phrase followed by a verb). VP coordination (`owns a dog
/// and likes a cat`) and NP coordination (`a dog and a cat`) are left alone,
/// as are conditionals, whose `and` belongs to the rule.
pub fn split_coordinated(text: &str, lexicon: &Lexicon) -> String {
    let coord = regex!(r"\s+and(?:\s+(?:then|like|also|so))?\s+");
    let mut out = String::new();
    for segment in sentence_segments(text) {
        let (body, term) = split_terminator(segment);
        let lower = body.trim_start().to_lowercase();
        if lower.starts_with("if ") || lower.starts_with("when ") || lower.starts_with("whenever ") {
            out.push_str(segment);
            continue;
        }
        let quoted = spot_values(body);
        let mut rest = body;
        let mut base = 0usize;
        loop {
            let found = coord.find_iter(rest).find(|m| {
                let abs = base + m.start();
                let in_quote = quoted.iter().any(|s| s.kind == SlotKind::Quoted && abs >= s.start && abs < s.end);
                !in_quote && starts_new_clause(&rest[m.end()..], lexicon)
            });
            match found {
                Some(m) => {
                    out.push_str(rest[..m.start()].trim_end());
                    out.push_str(". ");
                    base += m.end();
                    rest = &rest[m.end()..];
                }
                None => {
                    out.push_str(rest);
                    out.push_str(term);
                    break;
                }
            }
        }
    }
    out
}

/// True when `rest` begins with a subject noun phrase followed by a verb:
/// `mary is a nurse`, `the dog is brown`, `User owns a cat`, `User's dog barks`.
fn starts_new_clause(rest: &str, lexicon: &Lexicon) -> bool {
    let words: Vec<&str> = rest.split_whitespace().take(3).collect();
    if words.len() < 2 {
        return false;
    }
    let w0 = words[0].trim_end_matches(|c: char| !c.is_alphanumeric() && c != '\'');
    let is_subject_word = |w: &str| {
        let lw = w.to_lowercase();
        lw == "user" || lw == "assistant" || lexicon.canonical_name(w).is_some()
    };
    if is_subject_word(w0) {
        return is_verbish(words[1], lexicon);
    }
    let det = matches!(
        w0.to_lowercase().as_str(),
        "the" | "a" | "an" | "every" | "no" | "some" | "this" | "that" | "these" | "those" | "each"
    ) || w0.ends_with("'s");
    det && words.len() == 3 && is_verbish(words[2], lexicon)
}

pub(crate) fn is_verbish(word: &str, lexicon: &Lexicon) -> bool {
    let w = word.trim_end_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    matches!(
        w.as_str(),
        "is" | "are" | "was" | "were" | "has" | "have" | "had" | "does" | "do" | "did" | "can"
            | "cannot" | "could" | "should" | "must" | "may" | "will" | "would" | "owns" | "likes"
            | "loves" | "hates" | "wants" | "needs" | "knows" | "thinks" | "says"
    ) || lexicon.is_verb(&w)
}

/// Spell arithmetic operator words as operators between numbers so the value
/// spotter sees one expression: `3 times 4` -> `3 * 4`, `481 plus 26` -> `481 + 26`.
/// Nothing is evaluated; the numbers are untouched.
pub fn convert_operator_words(text: &str) -> String {
    let re = regex!(
        r"(?i)(\d[\d.]*|\))\s+(times|multiplied by|x|plus|minus|divided by|over|mod|modulo|to the power of)\s+(\d|\()"
    );
    let mut s = text.to_string();
    loop {
        let next = re
            .replace(&s, |caps: &regex::Captures| {
                let op = match caps[2].to_lowercase().as_str() {
                    "times" | "multiplied by" | "x" => "*",
                    "plus" => "+",
                    "minus" => "-",
                    "divided by" | "over" => "/",
                    "mod" | "modulo" => "%",
                    _ => "^",
                };
                format!("{} {} {}", &caps[1], op, &caps[3])
            })
            .into_owned();
        if next == s {
            return s;
        }
        s = next;
    }
}

/// Sentence segments of `text`, each keeping its terminator (if any).
fn sentence_segments(text: &str) -> Vec<&str> {
    let mut segments = vec![];
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if matches!(c, '.' | '?' | '!') {
            segments.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        segments.push(&text[start..]);
    }
    segments
}

fn split_terminator(segment: &str) -> (&str, &str) {
    match segment.chars().last() {
        Some('.') | Some('?') | Some('!') => (&segment[..segment.len() - 1], &segment[segment.len() - 1..]),
        _ => (segment, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex() -> Lexicon {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let seed = manifest.parent().unwrap().parent().unwrap().join("data/seed");
        let mut lex = Lexicon::load_seed_dir(&seed).expect("seed lexicon");
        lex.add_names(&["John", "Mary", "Bob"]);
        lex
    }

    #[test]
    fn contractions_expand() {
        assert_eq!(expand_contractions("whats the double of 100"), "what is the double of 100");
        assert_eq!(expand_contractions("I'm fine and it's ok"), "i am fine and it is ok");
        assert_eq!(expand_contractions("whatsoever"), "whatsoever");
    }

    #[test]
    fn agreement_follows_the_fixed_names() {
        assert_eq!(fix_agreement("User am happy"), "User is happy");
        assert_eq!(fix_agreement("are Assistant ok"), "is Assistant ok");
        assert_eq!(fix_agreement("what do Assistant think about dogs"), "what does Assistant think about dogs");
        assert_eq!(fix_agreement("do Assistant like music"), "does Assistant like music");
        assert_eq!(fix_agreement("User do not know"), "User does not know");
    }

    #[test]
    fn coordinated_clauses_split() {
        let lex = lex();
        assert_eq!(
            split_coordinated("john is a doctor and mary is a nurse", &lex),
            "john is a doctor. mary is a nurse"
        );
        assert_eq!(
            split_coordinated("john has this dog right and like the dog is brown", &lex),
            "john has this dog right. the dog is brown"
        );
        // VP and NP coordination stay in one sentence.
        assert_eq!(split_coordinated("john owns a dog and likes a cat", &lex), "john owns a dog and likes a cat");
        assert_eq!(split_coordinated("john owns a dog and a cat", &lex), "john owns a dog and a cat");
        // Conditionals keep their `and`.
        let rule = "if john owns a dog and mary owns a cat then john is happy";
        assert_eq!(split_coordinated(rule, &lex), rule);
    }

    #[test]
    fn possession_and_demonstratives() {
        assert_eq!(possession("john has this dog right"), "john owns a dog right");
        assert_eq!(possession("User have two cats"), "User own two cats");
        assert_eq!(possession("this dog is brown"), "the dog is brown");
        // Auxiliary `have` keeps its shape.
        assert_eq!(possession("User have been busy"), "User have been busy");
        assert_eq!(possession("john has to go"), "john has to go");
    }

    #[test]
    fn fixed_subjects_take_third_person() {
        let lex = lex();
        assert_eq!(conjugate_fixed_subjects("User own 2 cats", &lex), "User owns 2 cats");
        assert_eq!(conjugate_fixed_subjects("User is happy", &lex), "User is happy");
        assert_eq!(conjugate_fixed_subjects("does User own a cat", &lex), "does User own a cat");
        assert_eq!(conjugate_fixed_subjects("User owns a cat", &lex), "User owns a cat");
    }

    #[test]
    fn number_words_and_mentions() {
        let lex = lex();
        assert_eq!(digits("User owns two cats", &lex), "User owns 2 cats");
        assert_eq!(digits("no one knows", &lex), "no one knows");
        assert_eq!(digits("what is two", &lex), "what is two");
        assert_eq!(quote_mentions("reverse the word hello"), "reverse \"hello\"");
    }

    #[test]
    fn past_becomes_present_and_plurals_singular() {
        let lex = lex();
        assert_eq!(present_tense("which customer bought the red thing", &lex), "which customer buys the red thing");
        assert_eq!(present_tense("John was happy", &lex), "John is happy");
        assert_eq!(present_tense("say \"bought\" again", &lex), "say \"bought\" again");
        assert_eq!(singular_quantifiers("no cats are dogs", &lex), "no cat is a dog");
        assert_eq!(singular_quantifiers("all dogs are animals", &lex), "every dog is an animal");
        assert_eq!(singular_quantifiers("all dogs are brown", &lex), "every dog is brown");
    }

    #[test]
    fn modals_progressive_corrections_and_discourse() {
        assert_eq!(modals_and_quantities("john has to leave"), "john must leave");
        assert_eq!(modals_and_quantities("User needs to sleep"), "User must sleep");
        assert_eq!(modals_and_quantities("bob is allowed to drive"), "bob may drive");
        assert_eq!(modals_and_quantities("john has a dog"), "john has a dog");
        assert_eq!(modals_and_quantities("john does not have to leave"), "john does not have to leave");
        assert_eq!(modals_and_quantities("john does not need to leave"), "john does not have to leave");
        assert_eq!(strip_expletives("this fucking server is slow as shit"), "this server is very slow");
        assert_eq!(modals_and_quantities("no more than 2 cats"), "at most 2 cats");
        assert_eq!(modals_and_quantities("no less than 3 dogs"), "at least 3 dogs");
        assert_eq!(modals_and_quantities("2 or more dogs"), "at least 2 dogs");
        let lex = lex();
        assert_eq!(progressive("people are waiting", &lex), "people wait");
        assert_eq!(progressive("john is going home", &lex), "john goes home");
        assert_eq!(progressive("the movie is boring", &lex), "the movie is boring");
        assert_eq!(progressive("the file is missing", &lex), "the file is missing");
        assert_eq!(self_correction("john owns a dog wait no sorry mary owns the dog"), "mary owns the dog");
        assert_eq!(self_correction("no i mean every dog does not like cats"), "every dog does not like cats");
        assert_eq!(self_correction("no i meant mary"), "no i meant mary");
        assert_eq!(self_correction("you know what i mean"), "you know what i mean");
        assert_eq!(strip_discourse("yeah but User is not sleepy"), "User is not sleepy");
        assert_eq!(strip_discourse("ok so basically mary works at the bank"), "mary works at the bank");
        assert_eq!(strip_discourse("no dog likes cats"), "no dog likes cats");
        assert_eq!(strip_discourse("ok"), "ok");
    }

    #[test]
    fn contractions_without_apostrophes() {
        assert_eq!(expand_contractions("john doesnt like cats"), "john does not like cats");
        assert_eq!(expand_contractions("im tired and i cant sleep"), "i am tired and i cannot sleep");
        assert_eq!(expand_contractions("it was well done"), "it was well done");
        assert_eq!(expand_contractions("they were here"), "they were here");
    }

    #[test]
    fn weather_and_arithmetic_verbs() {
        assert_eq!(strip_expletives("where the hell is bob"), "where is bob");
        assert_eq!(weather("if its raining then bob stays home"), "if the weather is rainy then bob stays home");
        assert_eq!(weather("its dog is brown"), "its dog is brown");
        assert_eq!(arithmetic_verbs("divide 100 by 4 please"), "100 / 4 please");
        assert_eq!(arithmetic_verbs("subtract 2 from 10"), "10 - 2");
        assert_eq!(arithmetic_verbs("the sum of 2 and 3"), "(2 + 3)");
    }

    #[test]
    fn operator_words_become_operators() {
        assert_eq!(convert_operator_words("3 times 4"), "3 * 4");
        assert_eq!(convert_operator_words("481 plus 26 divided by 4"), "481 + 26 / 4");
        assert_eq!(convert_operator_words("the double of 100"), "the double of 100");
    }
}
