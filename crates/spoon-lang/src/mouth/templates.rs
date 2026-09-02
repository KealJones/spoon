//! Deterministic template realizer. Converts a `ResponsePlan` to plain English
//! with no LLM. Variant selection is seeded by a hash of the move content, so
//! the same plan always produces the same text (offline-safe, test-stable).

use spoon_core::types::can::Effect;
use spoon_core::types::response::{Move, ResponsePlan, Tone};
use spoon_core::types::value::Value;

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 14695981039346656037u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

fn seed(m: &Move) -> u64 {
    fnv1a(&format!("{m:?}"))
}

fn pick(choices: &[&str], s: u64) -> String {
    choices[(s as usize) % choices.len()].to_string()
}

fn pick_own(choices: Vec<String>, s: u64) -> String {
    let idx = (s as usize) % choices.len();
    choices.into_iter().nth(idx).unwrap_or_default()
}

fn fmt_effect(e: &Effect) -> &'static str {
    match e {
        Effect::Pure => "pure",
        Effect::Read => "read",
        Effect::Write => "write",
        Effect::Network => "network",
        Effect::Shell => "shell",
    }
}

fn join_values(values: &[Value]) -> String {
    match values.len() {
        0 => "nothing".into(),
        1 => values[0].render(),
        2 => format!("{} and {}", values[0].render(), values[1].render()),
        _ => {
            let (last, rest) = values.split_last().unwrap();
            let parts: Vec<String> = rest.iter().map(Value::render).collect();
            format!("{}, and {}", parts.join(", "), last.render())
        }
    }
}

/// Convert a `ResponsePlan` into a plain-English string, deterministically.
pub fn realize(plan: &ResponsePlan) -> String {
    plan.moves.iter().map(|m| realize_move(m, plan.tone)).collect::<Vec<_>>().join(" ")
}

fn realize_move(m: &Move, tone: Tone) -> String {
    let s = seed(m);
    match m {
        Move::Greet { returning } => {
            if *returning {
                pick(&["welcome back.", "hey, you're back.", "good to see you again."], s)
            } else {
                pick(&["hey. what's up?", "yo. what do you need?", "hi. what can i do for you?"], s)
            }
        }
        Move::Farewell => pick(&["later.", "see ya.", "take care."], s),
        Move::Thanks => pick(&["thanks.", "appreciate it.", "cheers."], s),
        Move::Ack { summary } => pick_own(
            vec![
                format!("got it: {summary}"),
                format!("noted, {summary}."),
                format!("ok, {summary}."),
            ],
            s,
        ),
        Move::Empathize { feeling, about } => {
            let prefix = if tone == Tone::Warm { "hey, " } else { "" };
            format!("{prefix}that sounds {feeling}. {about} is a lot.")
        }
        Move::Reflect { summary } => pick_own(
            vec![
                format!("so, {summary}."),
                format!("right, so {summary}."),
                format!("ok so {summary}."),
            ],
            s,
        ),
        Move::Answer { values, source, .. } => {
            let val_str = join_values(values);
            let base = format!("{val_str}.");
            match source {
                Some(src) => format!("{base} (source: {src})"),
                None => base,
            }
        }
        Move::YesNo { answer, because, .. } => {
            let head = if *answer {
                pick_own(vec!["yes.".into(), "yes, right.".into(), "yes, correct.".into()], s)
            } else {
                pick_own(vec!["no.".into(), "no, not quite.".into(), "no, nope.".into()], s)
            };
            match because {
                Some(b) => format!("{head} {b}"),
                None => head,
            }
        }
        Move::Result { value, steps, .. } => {
            let v = value.render();
            if *steps > 1 { format!("{v} (took {steps} steps)") } else { v }
        }
        Move::Opinion { stance, reasons, .. } => {
            let prefix = pick(&["honestly?", "real talk:", "straight up:"], s);
            if reasons.is_empty() {
                format!("{prefix} {stance}.")
            } else {
                format!("{prefix} {stance}. {}", reasons.join(". "))
            }
        }
        Move::Advise { situation, options, leaning } => {
            let mut out = format!("re: {situation}");
            for opt in options {
                let pros = if opt.pros.is_empty() { "ok".into() } else { opt.pros.join(", ") };
                let cons =
                    if opt.cons.is_empty() { "none obvious".into() } else { opt.cons.join(", ") };
                out.push_str(&format!("\n- {}: {}; downside: {}", opt.option, pros, cons));
            }
            if let Some(lean) = leaning {
                out.push_str(&format!("\ni'd lean toward {lean}."));
            }
            out
        }
        Move::Recall { episodes, .. } => {
            let ep = match episodes.len() {
                0 => "nothing relevant".into(),
                1 => episodes[0].clone(),
                _ => episodes.join(", "),
            };
            format!("you mentioned {ep}...")
        }
        Move::Explain { text } => text.clone(),
        Move::Info { text, source } => format!("{text} (source: {source})"),
        Move::Clarify { question, options, .. } => {
            if options.is_empty() {
                question.clone()
            } else {
                let opts: Vec<String> = options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| format!("{}) {o}", (b'a' + i as u8) as char))
                    .collect();
                format!("{question} {}", opts.join(" "))
            }
        }
        Move::Elicit { input_name, ty, .. } => {
            format!("what {input_name} should i use? (needs a {ty})")
        }
        Move::AskPermission { description, effect, .. } => {
            format!("this will {} ({}). ok to run?", description, fmt_effect(effect))
        }
        Move::AskExamples { capability, signature } => {
            format!(
                "i don't have a way to {capability} yet. give me 1-3 examples like \
                `input -> output` and i'll build it. signature: {signature}"
            )
        }
        Move::UnknownWord { word, guess } => {
            let base = format!("what does '{word}' mean?");
            match guess {
                Some(g) => format!("{base} i'm guessing {g}?"),
                None => base,
            }
        }
        Move::Refuse { reason } => reason.clone(),
        Move::Learned { what } => format!("learned: {what}"),
        Move::CannotDo { what, reason } => format!("can't {what}: {reason}"),
        Move::Error { message } => format!("something broke: {message}"),
    }
}
