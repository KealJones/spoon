//! The model-free path.
//!
//! Deliberately narrow. It handles the shapes that recur constantly in
//! conversation, and hands everything else to the model rather than guessing.
//! A native reading that is wrong is worse than no native reading, because the
//! model would have got it right and now nothing will.

use spoon_concept::Concept;
use spoon_seat::{Ears, Heard, LlmError, Turn};

/// Filler that carries no meaning and only confuses matching.
const FILLER: &[&str] = &[
    "um",
    "uh",
    "like",
    "you know",
    "i mean",
    "sorta",
    "kinda",
    "basically",
    "actually",
    "literally",
    "just",
    "really",
    "so",
    "well",
    "okay",
    "ok",
];

/// Openers that mark a request without changing what is being asked for.
const REQUEST_PREFIX: &[&str] = &[
    "can you",
    "could you",
    "can u",
    "would you",
    "will you",
    "please",
    "i want you to",
    "i need you to",
    "go ahead and",
    "lets",
    "let's",
];

const GREETING: &[&str] = &[
    "hi",
    "hello",
    "hey",
    "yo",
    "sup",
    "howdy",
    "morning",
    "good morning",
];

const THANKS: &[&str] = &[
    "thanks",
    "thank you",
    "ty",
    "thx",
    "cheers",
    "appreciate it",
];

const FAREWELL: &[&str] = &["bye", "goodbye", "later", "see ya", "cya", "night"];

/// Words that can sit around a greeting without turning it into a request.
///
/// A greeting is a whole turn or it is not a greeting. "hey" is hello; "hey
/// quick one, 356 minus 43" is arithmetic with a polite opener, and reading
/// the first word and stopping threw the question away and answered hello.
/// So the test is whether anything substantive is left, not whether it starts
/// with a greeting.
const SMALL_TALK: &[&str] = &[
    "there", "you", "u", "how", "hows", "how's", "are", "is", "it", "going", "goin", "on",
    "doing", "doin", "up", "whats", "what's", "wassup", "wasup", "good", "well", "list-all",
    "right", "alright", "man", "dude", "girl", "bro", "buddy", "friend", "again", "morning",
    "afternoon", "evening", "everyone", "everybody", "yall", "y'all", "spoon", "lol", "haha",
];

/// Markers that a speaker is repairing what they just said.
///
/// A finite, learnable set. Detecting a correction does not need to understand
/// the sentence, only to notice the repair, which is why this works without a
/// model.
pub const CORRECTION_MARKERS: &[&str] = &[
    "no wait",
    "wait no",
    "actually no",
    "i mean",
    "sorry i meant",
    "i meant",
    "not that",
    "scratch that",
    "never mind",
    "nvm",
    "correction",
    "no,",
];

pub struct NativeEars;

impl NativeEars {
    pub fn new() -> Self {
        NativeEars
    }

    /// Strip what carries no meaning, so matching sees the content.
    pub fn normalize(text: &str) -> String {
        let lowered = text.trim().to_lowercase();
        let mut cleaned = lowered.clone();
        for prefix in REQUEST_PREFIX {
            if let Some(rest) = cleaned.strip_prefix(prefix) {
                cleaned = rest.trim().to_string();
            }
        }
        let words: Vec<&str> = cleaned
            .split_whitespace()
            .filter(|w| {
                let bare = w.trim_matches(|c: char| !c.is_alphanumeric());
                !FILLER.contains(&bare)
            })
            .collect();
        words
            .join(" ")
            .trim_matches(|c: char| c == '?' || c == '!' || c == '.')
            .to_string()
    }

    /// Does this utterance repair a previous one?
    pub fn correction_marker(text: &str) -> Option<&'static str> {
        let lowered = text.to_lowercase();
        CORRECTION_MARKERS
            .iter()
            .copied()
            .find(|m| lowered.contains(m))
    }

    /// Is this utterance nothing but pleasantries?
    ///
    /// Exposed because the model needs checking against it. A 4b model shown
    /// "hey reverse spoon lol" often returns a greeting and drops the request,
    /// and answering hello to a question is worse than admitting confusion.
    pub fn is_social_only(text: &str) -> bool {
        Self::social(text).is_some()
    }

    fn social(text: &str) -> Option<Concept> {
        let n = Self::normalize(text);
        fn word(w: &str) -> &str {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        }
        // Nothing substantive left over. A multi-word set entry like "see ya"
        // is checked against the whole utterance; single words are checked per
        // word so ordering and repetition do not matter ("yo yo yo").
        let only = |set: &[&str]| {
            let words: Vec<&str> = n
                .split_whitespace()
                .map(word)
                .filter(|w| !w.is_empty())
                .collect();
            if words.is_empty() {
                return false;
            }
            let opens = set.iter().any(|w| n == *w || n.starts_with(&format!("{w} ")));
            let all_social = words
                .iter()
                .all(|w| set.contains(w) || SMALL_TALK.contains(w));
            opens && all_social
        };
        let matches = |set: &[&str]| only(set);
        if matches(GREETING) {
            return Some(Concept::call("chat", [Concept::call("greet", [])]));
        }
        if matches(THANKS) {
            return Some(Concept::call(
                "chat",
                [Concept::call("acknowledge-thanks", [])],
            ));
        }
        if matches(FAREWELL) {
            return Some(Concept::call("chat", [Concept::call("farewell", [])]));
        }
        None
    }

    /// A source research request. The source name is a Concept, so this
    /// shape works for built-in and user-registered sources alike.
    fn research(text: &str) -> Option<Concept> {
        let normalized = Self::normalize(text);
        let mut parts = normalized.splitn(3, ' ');
        let verb = parts.next()?;
        if !matches!(verb, "research" | "search") {
            return None;
        }
        let source = parts.next()?.trim();
        let query = parts.next()?.trim();
        if source.is_empty() || query.is_empty() {
            return None;
        }
        Some(Concept::call(
            "do",
            [Concept::call(
                "research-search",
                [Concept::named(source), Concept::text(query)],
            )],
        ))
    }

    /// Arithmetic written the way people actually write it.
    fn arithmetic(text: &str) -> Option<Concept> {
        let n = Self::normalize(text);
        let expr = n
            .trim_start_matches("what is")
            .trim_start_matches("whats")
            .trim_start_matches("what's")
            .trim_start_matches("calculate")
            .trim_start_matches("compute")
            .trim();
        let spelled = expr
            .replace(" plus ", " + ")
            .replace(" minus ", " - ")
            .replace(" times ", " * ")
            .replace(" divided by ", " / ")
            .replace(" over ", " / ");
        let tokens: Vec<&str> = spelled.split_whitespace().collect();
        if tokens.len() != 3 {
            return None;
        }
        let left: i64 = tokens[0].parse().ok()?;
        let right: i64 = tokens[2].parse().ok()?;
        let op = match tokens[1] {
            "+" => "math-add",
            "-" => "math-sub",
            "*" | "x" => "math-mul",
            "/" => "math-div",
            _ => return None,
        };
        Some(Concept::call(
            "do",
            [Concept::call(op, [Concept::int(left), Concept::int(right)])],
        ))
    }
}

impl Default for NativeEars {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Ears for NativeEars {
    /// With no model configured there is nothing further to try, so a miss is a
    /// miss. Saying so beats inventing a reading.
    async fn hear(
        &self,
        text: &str,
        _vocabulary: &[String],
        _recent: &[Turn],
        _rules: &[String],
    ) -> Result<Heard, LlmError> {
        self.hear_native(text).ok_or(LlmError::NoSeat("ears"))
    }

    fn hear_native(&self, text: &str) -> Option<Heard> {
        if text.trim().is_empty() {
            return None;
        }
        if let Some(step) = Self::social(text) {
            return Some(Heard::native(vec![step], 0.95));
        }
        if let Some(step) = Self::research(text) {
            return Some(Heard::native(vec![step], 0.9));
        }
        if let Some(step) = Self::arithmetic(text) {
            return Some(Heard::native(vec![step], 0.9));
        }
        None
    }
}
