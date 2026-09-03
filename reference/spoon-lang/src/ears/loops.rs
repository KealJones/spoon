//! Loop-like logic. Messy English iterates (`for each customer give them a
//! receipt if they bought something`); SCE quantifies (`For every customer X
//! if X buys something then Assistant gives a receipt to X.`). Every rule
//! here is a pure string rewrite: the loop noun gets the variable `X` (or the
//! definite `the N` outside a `For every` sentence), the pronouns that point
//! at it become that variable, and an imperative consequence puts `Assistant`
//! in the acting seat. Bare conditionals without `then` (`if any task has
//! more than 3 failures stop retrying that task`) and event loops (`every
//! time a build fails retry it`) share the same machinery.

use regex::Regex;
use std::sync::OnceLock;

use crate::ears::lexicon::Lexicon;
use crate::ears::rules::{is_verbish, third_person};

macro_rules! regex {
    ($pat:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pat).expect("static regex"))
    }};
}

/// Apply the loop rules to one sentence body (no terminator). `None` when
/// the sentence is not loop-shaped or a part of it is not understood: a
/// half rewrite would drop content, so the whole sentence is left alone.
pub fn loop_rewrite(body: &str, lexicon: &Lexicon) -> Option<Vec<String>> {
    for_each(body, lexicon)
        .or_else(|| every_time(body, lexicon))
        .or_else(|| keep_doing(body, lexicon))
        .or_else(|| bare_conditional(body, lexicon))
}

/// A condition on the loop variable.
#[derive(Debug, Clone, PartialEq)]
enum Cond {
    /// `X is ADJ` / `X is not ADJ`: an adjective the noun can carry.
    Copula { adj: String, negated: bool },
    /// Anything else, with pronouns already replaced.
    Other(String),
}

/// `for each N ...`, `go through every N and ...`, `check every N, if ...`.
fn for_each(body: &str, lexicon: &Lexicon) -> Option<Vec<String>> {
    let head = regex!(
        r"^(?:for (?:every|each|all)(?: of)?(?: the)?|(?:go|run|walk|iterate|loop) (?:through|over)(?: every| each| all| all the| all of the| the)?|(?:check|scan|review) (?:every|each|all|all the|all of the))\s+([a-z]+)(?:\s+that\s+(?:is|are)\s+(not\s+)?([a-z]+))?\s*(?:[,:;]\s*|\s+)(.+)$"
    );
    let caps = head.captures(body)?;
    let noun = lexicon.singular(&caps[1]).unwrap_or_else(|| caps[1].to_string());
    let header = caps.get(3).map(|adj| Cond::Copula { adj: adj.as_str().to_string(), negated: caps.get(2).is_some() });
    let rest = regex!(r"^(?:and then |and |then )?(.+)$").captures(&caps[4])?[1].to_string();

    let mut out = vec![];
    for segment in segments(&rest, lexicon) {
        out.push(segment_sentence(&segment, &noun, header.as_ref(), lexicon)?);
    }
    (!out.is_empty()).then_some(out)
}

/// `every time A VERB it (and if C VERB it)` -> `If A then Assistant VERBs the N.` ...
fn every_time(body: &str, lexicon: &Lexicon) -> Option<Vec<String>> {
    let caps = regex!(r"^(every time|each time|whenever|any time|when)\s+(.+)$").captures(body)?;
    // `when` is also a question word: only `when a N ...` is an event.
    if &caps[1] == "when" && !regex!(r"^(?:a|an|the|every|some|any) ").is_match(&caps[2]) {
        return None;
    }
    let rest = format!("if {}", &caps[2]);
    if rest.contains(" then ") {
        return None;
    }
    let mut out = vec![];
    let mut var: Option<String> = None;
    for segment in segments(&rest, lexicon) {
        let after_if = segment.strip_prefix("if ")?;
        let Some((cond, conseq)) = split_condition(after_if, lexicon) else {
            out.push(declarative_rule(after_if, lexicon)?);
            continue;
        };
        if var.is_none() {
            var = Some(bind_pronouns(&cond, &conseq)?);
        }
        let var = var.as_deref().unwrap_or_default();
        let cond = agree(&with_var(&cond, var), var, lexicon);
        let conseq = assistant_does(&conseq, var, lexicon)?;
        out.push(format!("If {cond} then {conseq}."));
    }
    (!out.is_empty()).then_some(out)
}

/// `keep VERBing Ns as long as there are ADJ ones and never VERB a ADJ N` ->
/// `Assistant VERBs every ADJ N. Assistant VERBs no ADJ N.`
fn keep_doing(body: &str, lexicon: &Lexicon) -> Option<Vec<String>> {
    let caps = regex!(
        r"^keep ([a-z]+ing) (?:the )?([a-z]+)(?: (?:as long as|while) there (?:is|are) ([a-z]+) ones)?(?:,? and (.+))?$"
    )
    .captures(body)?;
    let verb = ing_lemma(&caps[1], lexicon)?;
    let noun = lexicon.singular(&caps[2]).unwrap_or_else(|| caps[2].to_string());
    let adj = caps.get(3).map(|m| format!("{} ", m.as_str())).unwrap_or_default();
    let mut out = vec![format!("Assistant {} every {adj}{noun}.", third_person(&verb))];
    if let Some(tail) = caps.get(4) {
        out.push(format!("{}.", assistant_does(tail.as_str(), &format!("the {noun}"), lexicon)?));
    }
    Some(out)
}

/// `if COND VERB ...` with no `then`: the imperative is the consequence. With
/// no imperative, a clause of its own is: `if john owns a dog he feeds it`.
fn bare_conditional(body: &str, lexicon: &Lexicon) -> Option<Vec<String>> {
    let after_if = regex!(r"^if\s+(.+)$").captures(body)?[1].to_string();
    if after_if.contains(" then ") {
        return None;
    }
    let rule = match split_condition(&after_if, lexicon) {
        Some((cond, conseq)) => {
            let var = bind_pronouns(&cond, &conseq)?;
            let cond = agree(&with_var(&cond, &var), &var, lexicon);
            format!("If {cond} then {}.", assistant_does(&conseq, &var, lexicon)?)
        }
        None => declarative_rule(&after_if, lexicon)?,
    };
    Some(vec![rule])
}

/// A conditional whose consequence is a clause of its own: `John owns a dog he
/// feeds it` -> `If John owns a dog then John feeds the dog.` Pronouns in the
/// consequence point back into the condition: `he/she` at its only name, `it`
/// at its last noun phrase.
fn declarative_rule(after_if: &str, lexicon: &Lexicon) -> Option<String> {
    let (cond, conseq) = split_declarative(after_if, lexicon)?;
    let conseq = resolve_pronouns(&conseq, &cond, lexicon)?;
    Some(format!("If {cond} then {conseq}."))
}

/// `John owns a dog he feeds it` -> (`John owns a dog`, `he feeds it`). The
/// consequence starts at a name, a subject pronoun or a determiner phrase
/// that carries its own verb, after a condition that already has one.
fn split_declarative(text: &str, lexicon: &Lexicon) -> Option<(String, String)> {
    if regex!(r"\b(?:unless|otherwise|only if|except|but|and|or)\b").is_match(text) {
        return None;
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    for i in 2..words.len() {
        let cond = &words[..i];
        if !cond.iter().skip(1).any(|w| is_verbish(w, lexicon)) || BLOCKED_BEFORE_VERB.contains(&words[i - 1]) {
            continue;
        }
        let word = words[i];
        let verb_at = if word.starts_with(|c: char| c.is_uppercase()) || matches!(word, "he" | "she" | "it" | "they") {
            i + 1
        } else if matches!(word, "the" | "a" | "an" | "every" | "no" | "some") {
            let adjective = words.get(i + 1).is_some_and(|w| lexicon.is_adjective(w))
                && words.get(i + 2).is_some_and(|w| !is_verbish(w, lexicon));
            if adjective { i + 3 } else { i + 2 }
        } else {
            continue;
        };
        if words.get(verb_at).is_some_and(|v| is_verbish(v, lexicon) && !lexicon.is_base_verb(v)) {
            return Some((cond.join(" "), words[i..].join(" ")));
        }
    }
    None
}

/// `he feeds it` after `John owns a dog` -> `John feeds the dog`. `None` when
/// a pronoun has no single antecedent: a half-resolved clause is worse than
/// handing the sentence to the LLM.
fn resolve_pronouns(conseq: &str, cond: &str, lexicon: &Lexicon) -> Option<String> {
    let names: Vec<&str> = cond
        .split_whitespace()
        .filter(|w| w.starts_with(|c: char| c.is_uppercase()) && !matches!(*w, "User" | "Assistant"))
        .collect();
    let person = (names.len() == 1).then(|| names[0].trim_end_matches("'s"));
    let thing = last_np(cond, lexicon);
    let mut out: Vec<String> = vec![];
    let words: Vec<&str> = conseq.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        let next_is_noun = words.get(i + 1).is_some_and(|n| lexicon.is_noun(n) || lexicon.is_adjective(n));
        out.push(match *word {
            "he" | "she" | "him" => person?.to_string(),
            "his" => format!("{}'s", person?),
            "her" if next_is_noun => format!("{}'s", person?),
            "her" => person?.to_string(),
            "it" => thing.clone()?,
            "its" => format!("{}'s", thing.clone()?),
            "they" | "them" | "their" => return None,
            other => other.to_string(),
        });
    }
    Some(out.join(" "))
}

/// The last `det (adjective)? noun` of a condition as a definite: `a customer
/// owns a red card` -> `the card`.
fn last_np(cond: &str, lexicon: &Lexicon) -> Option<String> {
    let words: Vec<&str> = cond.split_whitespace().collect();
    let mut found = None;
    for i in 0..words.len() {
        if !matches!(words[i], "a" | "an" | "the" | "every" | "no" | "some") {
            continue;
        }
        let mut j = i + 1;
        while words.get(j).is_some_and(|w| lexicon.is_adjective(w))
            && words.get(j + 1).is_some_and(|w| lexicon.is_noun(w) || !lexicon.is_known(w))
        {
            j += 1;
        }
        if let Some(noun) = words.get(j).filter(|w| lexicon.is_noun(w) || !lexicon.is_known(w)) {
            found = Some(format!("the {noun}"));
        }
    }
    found
}

/// Outside a `For every N X` sentence the pronouns point at the condition's
/// first noun phrase, as a definite: `a build fails retry it` -> `the build`.
/// No noun phrase and no pronouns: nothing to bind, any name will do. No noun
/// phrase but pronouns: the sentence is not understood.
fn bind_pronouns(cond: &str, conseq: &str) -> Option<String> {
    let pronouns = regex!(r"\b(?:they|them|it|its|their)\b");
    match first_np(cond) {
        Some(np) => Some(np),
        None if pronouns.is_match(cond) || pronouns.is_match(conseq) => None,
        None => Some(String::new()),
    }
}

// ---- segments ----

/// Split the part after a loop header into independent pieces: `ping it and
/// if the ping fails restart it` -> [`ping it`, `if the ping fails restart
/// it`]. A conditional piece keeps its `and VERB` (VP coordination inside the
/// consequence); a comma before `if` or before an imperative always splits.
fn segments(rest: &str, lexicon: &Lexicon) -> Vec<String> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let mut out: Vec<String> = vec![];
    let mut current: Vec<&str> = vec![];
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        let bare = word.trim_end_matches([',', ';']);
        let comma = bare.len() != word.len();
        if matches!(bare, "and" | "then") && !current.is_empty() {
            let mut next = i + 1;
            if bare == "and" && words.get(next) == Some(&"then") {
                next += 1;
            }
            if words.get(next).is_some_and(|w| opens_segment(w, &current, lexicon)) {
                out.push(current.join(" "));
                current.clear();
                i = next;
                continue;
            }
            current.push(bare);
        } else {
            current.push(bare);
            if comma && words.get(i + 1).is_some_and(|w| opens_segment(w, &current, lexicon)) {
                out.push(current.join(" "));
                current.clear();
            }
        }
        i += 1;
    }
    if !current.is_empty() {
        out.push(current.join(" "));
    }
    out
}

/// `if` always opens a segment; an imperative verb opens one unless the
/// current piece is a conditional still waiting for its consequence.
fn opens_segment(word: &str, current: &[&str], lexicon: &Lexicon) -> bool {
    if word == "if" {
        return true;
    }
    let conditional = current.first() == Some(&"if");
    !conditional && (matches!(word, "never" | "otherwise") || lexicon.is_base_verb(word))
}

// ---- one segment -> one sentence ----

fn segment_sentence(segment: &str, noun: &str, header: Option<&Cond>, lexicon: &Lexicon) -> Option<String> {
    let (cond, conseq) = if let Some(after_if) = segment.strip_prefix("if ") {
        let (c, q) = split_condition(after_if, lexicon)?;
        (Some(c), q)
    } else if let Some((q, c)) = segment.split_once(" if ") {
        (Some(c.to_string()), q.to_string())
    } else {
        (None, segment.to_string())
    };
    let cond = cond.map(|c| classify_condition(&c, lexicon));

    if let Some(sentence) = compressed(&conseq, noun, cond.as_ref(), header, lexicon) {
        return Some(sentence);
    }

    // The general shape: `For every N X if COND then Assistant VERBs ... X ...`.
    let var = "X";
    let mut conds: Vec<String> = vec![];
    for c in header.into_iter().chain(cond.as_ref()) {
        conds.push(match c {
            Cond::Copula { adj, negated } => format!("{var} is {}{adj}", if *negated { "not " } else { "" }),
            Cond::Other(text) => agree(&with_var(text, var), var, lexicon),
        });
    }
    let conseq = assistant_does(&conseq, var, lexicon)?;
    Some(if conds.is_empty() {
        format!("For every {noun} {var} {conseq}.")
    } else {
        format!("For every {noun} {var} if {} then {conseq}.", conds.join(" and "))
    })
}

/// The compressed shape the corpus prefers when the consequence acts on the
/// loop variable itself: `if its empty delete it` -> `Assistant deletes every
/// empty file.`, `reject the expired ones` -> `Assistant rejects every
/// expired card.`. Only copula conditions compress (they become modifiers).
fn compressed(conseq: &str, noun: &str, cond: Option<&Cond>, header: Option<&Cond>, lexicon: &Lexicon) -> Option<String> {
    let words: Vec<&str> = conseq.split_whitespace().collect();
    let verb = words.first().filter(|v| lexicon.is_base_verb(v))?;
    let mut pre: Vec<&str> = vec![];
    let mut post: Vec<String> = vec![];
    match words.as_slice() {
        [_, pron] if is_object_pronoun(pron) => {}
        [_, "the", adj, "ones"] | [_, "all", "the", adj, "ones"] | [_, "every", adj, "one"] => pre.push(adj),
        _ => return None,
    }
    // A header relative clause stays a relative clause; an `if` copula becomes a modifier.
    match header {
        Some(Cond::Copula { adj, negated }) => post.push(format!("that is {}{adj}", if *negated { "not " } else { "" })),
        Some(Cond::Other(_)) => return None,
        None => {}
    }
    match cond {
        Some(Cond::Copula { adj, negated: false }) => pre.push(adj),
        Some(Cond::Copula { adj, negated: true }) => post.push(format!("that is not {adj}")),
        Some(Cond::Other(_)) => return None,
        None => {}
    }
    let mods = pre.iter().map(|a| format!("{a} ")).collect::<String>();
    let tail = post.iter().map(|p| format!(" {p}")).collect::<String>();
    Some(format!("Assistant {} every {mods}{noun}{tail}.", third_person(verb)))
}

fn is_object_pronoun(word: &str) -> bool {
    matches!(word, "it" | "them" | "him" | "her")
}

/// `its empty` / `they are not inactive` -> a copula condition; anything else
/// stays text.
fn classify_condition(cond: &str, lexicon: &Lexicon) -> Cond {
    let re = regex!(r"^(?:they|them|it|he|she) (?:is|are|was|were) (not )?([a-z]+)$|^its (not )?([a-z]+)$");
    if let Some(caps) = re.captures(cond) {
        let adj = caps.get(2).or(caps.get(4)).map(|m| m.as_str()).unwrap_or("");
        if lexicon.is_adjective(adj) || !lexicon.is_known(adj) {
            let negated = caps.get(1).or(caps.get(3)).is_some();
            return Cond::Copula { adj: adj.to_string(), negated };
        }
    }
    Cond::Other(cond.to_string())
}

// ---- condition / consequence boundary ----

/// Words that cannot precede the first word of an imperative consequence.
const BLOCKED_BEFORE_VERB: &[&str] = &[
    "is", "are", "was", "were", "not", "very", "a", "an", "the", "every", "no", "some", "to", "and", "or", "if", "that",
    "than", "any", "each", "all", "more", "most", "least", "at", "in", "on", "of", "with", "for", "from", "by",
];

/// `they are inactive disable their account` -> (`they are inactive`,
/// `disable their account`). An explicit `then` wins; otherwise the
/// consequence starts at the first base-form verb that follows a complete
/// condition (subject plus a verbish word) and is not itself part of a noun
/// phrase or verb chain.
fn split_condition(text: &str, lexicon: &Lexicon) -> Option<(String, String)> {
    // Nested exceptions are beyond a string rewrite; the LLM gets the original.
    if regex!(r"\b(?:unless|otherwise|only if|except|but)\b").is_match(text) {
        return None;
    }
    if let Some((cond, conseq)) = text.split_once(" then ") {
        return Some((cond.to_string(), conseq.to_string()));
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    for i in 2..words.len() {
        let word = words[i];
        let prev = words[i - 1];
        let opener = matches!(word, "never" | "do") || lexicon.is_base_verb(word);
        if !opener || BLOCKED_BEFORE_VERB.contains(&prev) || prev.ends_with("'s") {
            continue;
        }
        if words.get(i + 1).is_some_and(|next| lexicon.is_base_verb(next) && word != "do") {
            continue;
        }
        let cond = &words[..i];
        let complete = cond.iter().skip(1).any(|w| is_verbish(w, lexicon))
            || cond.first() == Some(&"its");
        if complete {
            return Some((cond.join(" "), words[i..].join(" ")));
        }
    }
    None
}

// ---- pronouns, agreement, imperatives ----

/// `they/them/it` -> `var`, `their/its` -> `var's`, `any N` -> `a N`, `its
/// ADJ` -> `var is ADJ`, `that N` -> `the N`. Gendered pronouns are left
/// alone: they point at people, not at the loop noun.
fn with_var(text: &str, var: &str) -> String {
    let s = regex!(r"^its (?:(not) )?([a-z]+)$")
        .replace(text, |caps: &regex::Captures| {
            let not = caps.get(1).map(|_| "not ").unwrap_or("");
            format!("it is {not}{}", &caps[2])
        })
        .into_owned();
    let s = regex!(r"\b(they|them|it)\b").replace_all(&s, var).into_owned();
    let s = regex!(r"\b(their|its)\b").replace_all(&s, format!("{var}'s")).into_owned();
    let s = regex!(r"\b(?:that|this|these|those) ([a-z]+)\b").replace_all(&s, "the $1").into_owned();
    regex!(r"\bany ([a-z]+)\b")
        .replace_all(&s, |caps: &regex::Captures| format!("{} {}", article(&caps[1]), &caps[1]))
        .into_owned()
}

/// The variable is singular: `X are` -> `X is`, `X own` -> `X owns`.
pub(crate) fn agree(text: &str, var: &str, lexicon: &Lexicon) -> String {
    if var.is_empty() {
        return text.to_string();
    }
    let escaped = regex::escape(var);
    let re = Regex::new(&format!(r"\b{escaped} ([a-z]+)\b")).expect("agreement regex");
    re.replace_all(text, |caps: &regex::Captures| {
        let verb = &caps[1];
        let fixed = match verb {
            "are" | "were" | "was" | "am" => "is".to_string(),
            "have" => "has".to_string(),
            "do" => "does".to_string(),
            v if lexicon.is_base_verb(v) && !v.ends_with('s') => third_person(v),
            v => v.to_string(),
        };
        format!("{var} {fixed}")
    })
    .into_owned()
}

/// An imperative becomes what Assistant does: `give them a receipt` ->
/// `Assistant gives a receipt to X`, `never process a cancelled order` ->
/// `Assistant processes no cancelled order`, `mark it broken` -> `Assistant
/// marks X as broken`.
fn assistant_does(conseq: &str, var: &str, lexicon: &Lexicon) -> Option<String> {
    let words: Vec<&str> = conseq.split_whitespace().collect();
    let (negated, verb, rest): (bool, &str, Vec<&str>) = match words.as_slice() {
        ["never", verb, rest @ ..] | ["do", "not", verb, rest @ ..] => (true, verb, rest.to_vec()),
        [verb, rest @ ..] => (false, verb, rest.to_vec()),
        [] => return None,
    };
    if !lexicon.is_base_verb(verb) {
        return None;
    }
    let mut rest = with_var(&rest.join(" "), var);
    if negated {
        return Some(match regex!(r"^(?:a|an) (.+)$").captures(&rest) {
            Some(caps) => format!("Assistant {} no {}", third_person(verb), &caps[1]),
            None => format!("Assistant does not {verb} {rest}"),
        });
    }
    let dative = Regex::new(&format!(r"^{} ((?:a|an|the|every|some|\d+) .+)$", regex::escape(var))).expect("dative regex");
    if let Some(caps) = dative.captures(&rest).filter(|_| is_dative(verb)) {
        rest = format!("{} to {var}", &caps[1]);
    }
    let labels = matches!(verb, "mark" | "flag" | "label" | "consider" | "declare");
    if let Some(caps) = regex!(r"^(.+?) ([a-z]+)$").captures(&rest).filter(|c| labels && lexicon.is_adjective(&c[2])) {
        rest = format!("{} as {}", &caps[1], &caps[2]);
    }
    // VP coordination inside the consequence: `delete X and archive X`.
    let rest = regex!(r"\band ([a-z]+)\b")
        .replace_all(&rest, |caps: &regex::Captures| {
            if lexicon.is_base_verb(&caps[1]) { format!("and {}", third_person(&caps[1])) } else { caps[0].to_string() }
        })
        .into_owned();
    let rest = rest.trim();
    Some(if rest.is_empty() {
        format!("Assistant {}", third_person(verb))
    } else {
        format!("Assistant {} {rest}", third_person(verb))
    })
}

fn is_dative(verb: &str) -> bool {
    matches!(verb, "give" | "send" | "show" | "offer" | "hand" | "mail" | "email" | "grant" | "assign" | "award" | "pay" | "lend")
}

/// First noun phrase of a condition as a definite: `a build fails` -> `the build`.
fn first_np(cond: &str) -> Option<String> {
    regex!(r"\b(?:a|an|the|any|every|each|some) ([a-z]+)\b").captures(cond).map(|c| format!("the {}", &c[1]))
}

/// `processing` -> `process`, `running` -> `run`, `deleting` -> `delete`.
fn ing_lemma(word: &str, lexicon: &Lexicon) -> Option<String> {
    let stem = word.strip_suffix("ing")?;
    let mut candidates = vec![stem.to_string(), format!("{stem}e")];
    let bytes = stem.as_bytes();
    if bytes.len() >= 2 && bytes[bytes.len() - 1] == bytes[bytes.len() - 2] {
        candidates.push(stem[..stem.len() - 1].to_string());
    }
    candidates.into_iter().find(|c| lexicon.is_base_verb(c))
}

fn article(noun: &str) -> &'static str {
    if noun.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex() -> Lexicon {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let seed = manifest.parent().unwrap().parent().unwrap().join("data/seed");
        Lexicon::load_seed_dir(&seed).expect("seed lexicon")
    }

    fn rw(body: &str) -> Vec<String> {
        loop_rewrite(body, &lex()).unwrap_or_else(|| panic!("no loop rewrite for {body:?}"))
    }

    #[test]
    fn for_every_with_condition_keeps_the_variable() {
        assert_eq!(
            rw("for every user if they are inactive disable their account"),
            vec!["For every user X if X is inactive then Assistant disables X's account."]
        );
        assert_eq!(
            rw("for every user if they are inactive then disable their account"),
            vec!["For every user X if X is inactive then Assistant disables X's account."]
        );
    }

    #[test]
    fn consequence_first_and_dative_shift() {
        assert_eq!(
            rw("for each customer give them a receipt if they buys something"),
            vec!["For every customer X if X buys something then Assistant gives a receipt to X."]
        );
    }

    #[test]
    fn loop_bodies_compress_onto_the_quantifier() {
        assert_eq!(rw("go through every card and reject the expired ones"), vec!["Assistant rejects every expired card."]);
        assert_eq!(
            rw("check every file, if its empty delete it, if it is not empty archive it"),
            vec!["Assistant deletes every empty file.", "Assistant archives every file that is not empty."]
        );
        assert_eq!(rw("for each server ping it"), vec!["Assistant pings every server."]);
    }

    #[test]
    fn header_relative_clause_and_follow_up_rule() {
        assert_eq!(
            rw("for each server that is down ping it and if the ping fails restart it"),
            vec![
                "Assistant pings every server that is down.",
                "For every server X if X is down and the ping fails then Assistant restarts X."
            ]
        );
        assert_eq!(
            rw("for every user if they are inactive disable their account and email them"),
            vec!["For every user X if X is inactive then Assistant disables X's account and emails X."]
        );
    }

    #[test]
    fn bare_conditional_gets_its_then() {
        assert_eq!(
            rw("if any task owns more than 3 failures stop retrying that task"),
            vec!["If a task owns more than 3 failures then Assistant stops retrying the task."]
        );
        assert_eq!(rw("if the file is empty delete it"), vec!["If the file is empty then Assistant deletes the file."]);
        // Finished conditionals are not touched.
        assert_eq!(loop_rewrite("if john owns a dog then john is happy", &lex()), None);
    }

    #[test]
    fn declarative_consequences_resolve_their_pronouns() {
        assert_eq!(rw("if John owns a dog he feeds it"), vec!["If John owns a dog then John feeds the dog."]);
        assert_eq!(
            rw("if a customer owns a card the machine accepts it"),
            vec!["If a customer owns a card then the machine accepts the card."]
        );
        assert_eq!(
            rw("if John does not wait the machine rejects his card"),
            vec!["If John does not wait then the machine rejects John's card."]
        );
        assert_eq!(rw("when a dog is hungry it eats"), vec!["If a dog is hungry then the dog eats."]);
        assert_eq!(rw("if there is a red card every clerk checks it"), vec!["If there is a red card then every clerk checks the card."]);
        assert_eq!(rw("if the weather is rainy Bob stays at home"), vec!["If the weather is rainy then Bob stays at home."]);
        // Two names or no antecedent: the LLM gets it.
        let lex = lex();
        assert_eq!(loop_rewrite("if John likes Mary he is happy", &lex), None);
        assert_eq!(loop_rewrite("if John waits they leave", &lex), None);
        assert_eq!(loop_rewrite("if john gives the dog a bone", &lex), None);
    }

    #[test]
    fn event_loops_become_rules() {
        assert_eq!(
            rw("every time a build fails retry it and if it fails again mark it broken"),
            vec![
                "If a build fails then Assistant retries the build.",
                "If the build fails again then Assistant marks the build as broken."
            ]
        );
        assert_eq!(
            rw("whenever a subscriber is dormant disable their profile"),
            vec!["If a subscriber is dormant then Assistant disables the subscriber's profile."]
        );
    }

    #[test]
    fn keep_doing_and_never() {
        assert_eq!(
            rw("keep processing orders as long as there are pending ones and never process a cancelled order"),
            vec!["Assistant processes every pending order.", "Assistant processes no cancelled order."]
        );
    }

    #[test]
    fn non_loops_are_left_alone() {
        let lex = lex();
        assert_eq!(loop_rewrite("john owns a dog", &lex), None);
        assert_eq!(loop_rewrite("for every user", &lex), None);
        assert_eq!(loop_rewrite("save the file", &lex), None);
    }
}
