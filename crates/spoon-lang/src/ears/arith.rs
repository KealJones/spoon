//! Chained arithmetic. `take 20 subtract 3 then multiply it by 8` is one
//! expression told as a recipe; SCE wants the expression: `(20 - 3) * 8`. The
//! recipe is read step by step into an accumulator that is never evaluated,
//! only spelled. A step that is not understood leaves the whole sentence
//! alone: half a recipe is worse than none.
//!
//! Runs after `arithmetic_verbs`, so binary forms (`add 599 and 32`, `the sum
//! of 2 and 3`) already arrived as `599 + 32` and `(2 + 3)`.

use regex::Regex;
use std::sync::OnceLock;

use crate::ears::rules::{sentence_segments, split_terminator};

macro_rules! regex {
    ($pat:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pat).expect("static regex"))
    }};
}

/// A number or a parenthesized group.
const NUM: &str = r"(?:\d+(?:\.\d+)?|\([^()]+\))";
/// The accumulator as the user refers to it.
const IT: &str = r"(?:it|that|this|the result|the answer|the total)";

/// Rewrite every sentence that is a recipe of two or more steps into its
/// expression; other sentences pass through untouched.
pub fn chain_arithmetic(text: &str) -> String {
    let mut out = String::new();
    for segment in sentence_segments(text) {
        let (body, term) = split_terminator(segment);
        let lead = &body[..body.len() - body.trim_start().len()];
        match recipe(body.trim()) {
            Some(expr) => {
                out.push_str(lead);
                out.push_str(&expr);
                out.push_str(term);
            }
            None => out.push_str(segment),
        }
    }
    out
}

/// The expression a recipe spells, or `None` when the sentence is not one. A
/// lone expression is already spelled and is left alone.
fn recipe(body: &str) -> Option<String> {
    let opener = regex!(
        r"(?i)^(?:(?:(?:can|could|would|will) (?:you|Assistant)|Assistant,?|please|pls|plz|quickly|just|do|what is|whats|calculate|compute|work out|figure out)\s+)+"
    );
    let rest = opener.find(body).map(|m| &body[m.end()..]).unwrap_or(body);
    let rest = regex!(r"(?i)\s+(?:for (?:me|User)|please|pls)$").replace(rest, "");
    let steps = steps(&rest);
    if steps.len() == 1 && regex!(&format!(r"^{NUM}(?:\s+[-+*/]\s+{NUM})*$")).is_match(&steps[0]) {
        return None;
    }
    // `1 plus 2 times 3` is an expression with precedence, not a recipe; the
    // operator words are spelled later. A recipe has at least one verb step.
    let verbal = regex!(r"(?i)\b(?:take|add|subtract|multiply|divide|square|double|halve|half|twice|less than|more than)\b");
    if !steps.iter().any(|s| verbal.is_match(s)) {
        return None;
    }
    let mut acc = Acc::start(&steps[0])?;
    for step in &steps[1..] {
        acc = acc.apply(step)?;
    }
    acc.compound.then_some(acc.text)
}

/// Steps are separated by `then`, `and`, a comma, or simply by the next
/// step verb: `take 20 subtract 3` is two steps.
fn steps(text: &str) -> Vec<String> {
    let text = regex!(r"(?i)\s*(?:,\s*(?:and\s+)?(?:then\s+)?|\s+(?:and\s+then|then|and)\s+)").replace_all(text, " | ");
    let mut out: Vec<Vec<&str>> = vec![vec![]];
    for word in text.split_whitespace() {
        let starts_step = matches!(
            word.to_lowercase().as_str(),
            "add" | "plus" | "subtract" | "minus" | "multiply" | "times" | "divide" | "square" | "double" | "halve" | "take"
        );
        if word == "|" || (starts_step && !out.last().is_some_and(|s| s.is_empty())) {
            out.push(vec![]);
        }
        if word != "|" {
            out.last_mut().expect("one step").push(word);
        }
    }
    out.into_iter().filter(|s| !s.is_empty()).map(|s| s.join(" ")).collect()
}

/// The expression so far. `compound` says whether it needs parentheses when
/// it becomes an operand of `*` or `/`.
struct Acc {
    text: String,
    compound: bool,
}

impl Acc {
    /// The opening step: a number, an expression, `take N`, `square N`, `half
    /// of N`, `twice N`, or `M less than X`.
    fn start(step: &str) -> Option<Acc> {
        let step = step.trim();
        if let Some(caps) = regex!(&format!(r"(?i)^(?:take|start with|begin with|use|from)\s+({NUM})$")).captures(step) {
            return Some(Acc { text: caps[1].to_string(), compound: false });
        }
        if let Some(caps) = regex!(&format!(r"(?i)^square\s+({NUM})$")).captures(step) {
            return Some(Acc { text: format!("{} * {}", &caps[1], &caps[1]), compound: true });
        }
        if let Some(caps) = regex!(&format!(r"(?i)^(?:half|a half) of\s+({NUM})$")).captures(step) {
            return Some(Acc { text: format!("{} / 2", &caps[1]), compound: true });
        }
        // `double 21` stays a command with its own verb; only `twice` is an operator word.
        if let Some(caps) = regex!(&format!(r"(?i)^twice\s+({NUM})$")).captures(step) {
            return Some(Acc { text: format!("2 * {}", &caps[1]), compound: true });
        }
        if let Some(caps) = regex!(&format!(r"(?i)^({NUM})\s+(less|more) than\s+(.+)$")).captures(step) {
            let inner = Acc::start(&caps[3])?;
            let op = if caps[2].eq_ignore_ascii_case("less") { '-' } else { '+' };
            return Some(inner.binary(op, &caps[1]));
        }
        if regex!(&format!(r"^{NUM}(?:\s+[-+*/]\s+{NUM})*$")).is_match(step) {
            return Some(Acc { text: step.to_string(), compound: step.contains(' ') });
        }
        None
    }

    /// One more step on the accumulator.
    fn apply(self, step: &str) -> Option<Acc> {
        let step = step.trim();
        if let Some(caps) = regex!(&format!(r"(?i)^(?:add|plus)\s+({NUM})(?:\s+to\s+{IT})?$")).captures(step) {
            return Some(self.binary('+', &caps[1]));
        }
        if let Some(caps) = regex!(&format!(r"(?i)^(?:subtract|minus|take away|take off)\s+({NUM})(?:\s+from\s+{IT})?$")).captures(step) {
            return Some(self.binary('-', &caps[1]));
        }
        if let Some(caps) = regex!(&format!(r"(?i)^(?:multiply|times)\s+(?:{IT}\s+)?(?:by\s+)?({NUM})$")).captures(step) {
            return Some(self.binary('*', &caps[1]));
        }
        if let Some(caps) = regex!(&format!(r"(?i)^divide\s+(?:{IT}\s+)?by\s+({NUM})$")).captures(step) {
            return Some(self.binary('/', &caps[1]));
        }
        if let Some(caps) = regex!(&format!(r"(?i)^divide\s+({NUM})\s+by\s+{IT}$")).captures(step) {
            return Some(Acc { text: format!("{} / {}", &caps[1], self.wrapped()), compound: true });
        }
        if regex!(&format!(r"(?i)^square\s+{IT}$")).is_match(step) {
            let w = self.wrapped();
            return Some(Acc { text: format!("{w} * {w}"), compound: true });
        }
        if regex!(&format!(r"(?i)^double\s+{IT}$")).is_match(step) {
            return Some(self.binary('*', "2"));
        }
        if regex!(&format!(r"(?i)^(?:halve|half)\s+{IT}$")).is_match(step) {
            return Some(self.binary('/', "2"));
        }
        None
    }

    fn binary(self, op: char, operand: &str) -> Acc {
        let left = if matches!(op, '*' | '/') { self.wrapped() } else { self.text };
        Acc { text: format!("{left} {op} {operand}"), compound: true }
    }

    fn wrapped(&self) -> String {
        if self.compound { format!("({})", self.text) } else { self.text.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipes_become_expressions() {
        assert_eq!(chain_arithmetic("take 20 subtract 3 then multiply it by 8."), "(20 - 3) * 8.");
        assert_eq!(chain_arithmetic("square 12 then add 5 to it"), "12 * 12 + 5");
        assert_eq!(chain_arithmetic("take (2 + 3) and divide 20 by that"), "20 / (2 + 3)");
        assert_eq!(chain_arithmetic("can Assistant quickly 599 + 32 then divide that by 0 and times it by 6?"), "((599 + 32) / 0) * 6?");
        assert_eq!(chain_arithmetic("what is half of 90 plus 10"), "90 / 2 + 10");
        assert_eq!(chain_arithmetic("5 less than twice 9"), "2 * 9 - 5");
        assert_eq!(chain_arithmetic("take 21 then double it"), "21 * 2");
    }

    #[test]
    fn a_lone_verb_command_keeps_its_verb() {
        assert_eq!(chain_arithmetic("Assistant, double 21!"), "Assistant, double 21!");
    }

    #[test]
    fn non_recipes_pass_through() {
        assert_eq!(chain_arithmetic("John owns a dog and Mary owns a cat."), "John owns a dog and Mary owns a cat.");
        assert_eq!(chain_arithmetic("take 20"), "take 20");
        assert_eq!(chain_arithmetic("take the dog and feed it"), "take the dog and feed it");
        assert_eq!(chain_arithmetic("100 / 4"), "100 / 4");
        // Operator words keep their precedence; another rule spells them.
        assert_eq!(chain_arithmetic("what is 1 plus 2 times 3"), "what is 1 plus 2 times 3");
    }
}
