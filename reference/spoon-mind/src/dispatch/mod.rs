//! Dispatch layer: routes grounded clauses to moves, plans, or teacher asks.
//! Deterministic - no LLM in the interior.

pub mod arith;
mod command;
mod dialog;
mod opinion;
mod property;
mod selfmodel;

use spoon_core::can::Can;
use spoon_core::kernel::Kernel;
use spoon_core::store::Store;
use spoon_core::types::{Act, ConceptId, Fact, Intent, Move, QuestionKind, Signal, Value};

use crate::discourse::{
    answer_grounded, assert_grounded, display_value, realize_fact_with, Answer, Article, DiscourseState, FactWriter,
    Grounded,
};

pub use command::ask_examples_move;
pub use property::Present;

// ---- Public types ---------------------------------------------------------

pub struct DispatchCtx<'a> {
    pub can: &'a mut Can,
    pub store: &'a Store,
    pub kernel: &'a Kernel,
    pub host: &'a dyn spoon_core::kernel::Host,
    pub session_id: &'a str,
    pub episode_id: Option<i64>,
    /// Does this session have prior episodes (for Greet{returning})?
    pub returning_user: bool,
}

#[derive(Debug)]
pub enum TeacherAsk {
    Stance { topic: String },
    Advice { situation: String, keywords: Vec<String> },
    Concept { noun: String, context: String },
}

#[derive(Debug)]
pub enum Dispatched {
    /// Fully handled. Moves in the order they should be said.
    Moves(Vec<Move>),
    /// A Command (or a property question with no stored fact) resolved to a
    /// known action; the brain plans and executes it, then presents the value
    /// as `present` says.
    Plan { intent: Intent, moves_before: Vec<Move>, present: Present },
    /// A Command whose verb matches no action.
    UnknownCapability { verb: String, signals: Vec<Signal>, sce: String, fallback: Vec<Move> },
    /// Something the teacher could supply.
    NeedsTeacher { ask: TeacherAsk, fallback: Vec<Move> },
}

/// A literal that spells an absolute http(s) URL is a URL, however it was
/// written. `Assistant, http-get-json "https://x/y"!` puts the address in a
/// quoted string, which parses to `Value::Text`, and the planner matches
/// inputs by type, so `http.get_json` would ask the user for a url it already
/// has. The coercion belongs at signal binding rather than in
/// `Type::accepts_value` because a signal is the one place a value and the
/// type an action expects meet; loosening the runtime check alone would leave
/// the planner blind, and loosening `Type::Url` itself would let any text
/// through everywhere.
pub(crate) fn signal_value(v: &Value) -> Value {
    match v {
        Value::Text(s) if is_absolute_http_url(s) => Value::Url(s.clone()),
        other => other.clone(),
    }
}

fn is_absolute_http_url(s: &str) -> bool {
    let after_scheme = ["https://", "http://"]
        .iter()
        .find(|p| s.len() > p.len() && s[..p.len()].eq_ignore_ascii_case(p))
        .map(|p| &s[p.len()..]);
    matches!(after_scheme, Some(rest) if !rest.starts_with('/') && !rest.chars().any(char::is_whitespace))
}

// ---- Public API -----------------------------------------------------------

/// Dispatch a single grounded clause.
pub fn dispatch(ctx: &mut DispatchCtx<'_>, g: &Grounded, state: &DiscourseState) -> anyhow::Result<Dispatched> {
    match &g.clause.act {
        Act::Assert => dispatch_assert(ctx, g, state),
        Act::Command => command::dispatch_command(ctx, g, state),
        Act::Question { kind } => {
            let kind = kind.clone();
            dispatch_question(ctx, g, &kind, state)
        }
        Act::Rule => {
            let mut fw = FactWriter { can: ctx.can, store: ctx.store };
            let _ = assert_grounded(&mut fw, g, "user", ctx.episode_id)?;
            Ok(Dispatched::Moves(vec![Move::Learned { what: g.clause.sce.clone() }]))
        }
    }
}

/// Dispatch all grounded clauses of a turn in order, merging Moves.
/// The first non-Moves outcome wins for control flow, but earlier Moves are kept.
pub fn dispatch_turn(
    ctx: &mut DispatchCtx<'_>,
    gs: &[Grounded],
    state: &DiscourseState,
) -> anyhow::Result<Dispatched> {
    let mut accumulated_moves: Vec<Move> = vec![];

    for g in gs {
        let result = dispatch(ctx, g, state)?;
        match result {
            Dispatched::Moves(mut mvs) => {
                accumulated_moves.append(&mut mvs);
            }
            Dispatched::Plan { intent, mut moves_before, present } => {
                moves_before.splice(0..0, accumulated_moves);
                return Ok(Dispatched::Plan { intent, moves_before, present });
            }
            Dispatched::UnknownCapability { verb, signals, sce, mut fallback } => {
                fallback.splice(0..0, accumulated_moves);
                return Ok(Dispatched::UnknownCapability { verb, signals, sce, fallback });
            }
            Dispatched::NeedsTeacher { ask, mut fallback } => {
                fallback.splice(0..0, accumulated_moves);
                return Ok(Dispatched::NeedsTeacher { ask, fallback });
            }
        }
    }

    Ok(Dispatched::Moves(accumulated_moves))
}

/// SCE sentences the brain asserts at first boot (source "seed").
pub fn self_model_seed() -> Vec<String> {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("../../../../data/seed/self_model.json"))
            .expect("self_model.json must be valid JSON");
    json.as_array()
        .expect("self_model.json must be an array")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect()
}

// ---- Internal routing -----------------------------------------------------

fn dispatch_assert(ctx: &mut DispatchCtx<'_>, g: &Grounded, state: &DiscourseState) -> anyhow::Result<Dispatched> {
    // 1. Dialog verb with User subject.
    if let Some(d) = dialog::try_dialog_assert(ctx, g, state) {
        return Ok(d);
    }
    // 2. Store and apply small-talk policy.
    dialog::dispatch_store_assert(ctx, g, state)
}

fn dispatch_question(
    ctx: &mut DispatchCtx<'_>,
    g: &Grounded,
    kind: &QuestionKind,
    state: &DiscourseState,
) -> anyhow::Result<Dispatched> {
    let sce = g.clause.sce.clone();

    // 1. Self-model questions.
    if selfmodel::is_about_assistant(g) {
        // Capabilities and identity handled in selfmodel.
        if !opinion::is_opinion_question(g) {
            return selfmodel::handle_assistant_question(ctx, g, kind, state);
        }
    }

    // 2. Opinion questions: Spoon's own views, then views the user reported
    // ("What does User think about dogs?").
    if opinion::is_opinion_question(g) {
        return opinion::handle_opinion(ctx, g, state);
    }
    if let Some(d) = opinion::handle_reported_view(ctx, g, kind) {
        return Ok(d);
    }

    // 3. Advice questions.
    if opinion::is_advice_question(g, kind) {
        return opinion::handle_advice(ctx, g, state);
    }

    // 4. Standard fact lookup.
    let answer = answer_grounded(ctx.can, ctx.store, g, kind)?;
    let dispatched = match answer {
        Answer::Values(vs) if !vs.is_empty() => {
            let values = vs.iter().map(|v| display_value(ctx.can, ctx.store, v)).collect();
            Dispatched::Moves(vec![Move::Answer { question: sce, values, source: Some("memory".into()) }])
        }
        Answer::YesNo(b, fact) => {
            let because = fact.map(|f| render_fact(ctx, &f));
            Dispatched::Moves(vec![Move::YesNo { question: sce, answer: b, because }])
        }
        Answer::Count(n) => {
            Dispatched::Moves(vec![Move::Answer {
                question: sce,
                values: vec![Value::Int(n)],
                source: Some("memory".into()),
            }])
        }
        Answer::Values(_) | Answer::Unknown { .. } => {
            // 5. No fact: a property question may name a capability
            // ("What is the double of 100?" runs the learned `double`).
            if let Some(d) = property::compute(ctx, g, kind) {
                return Ok(d);
            }
            // Check for arithmetic term in conditions.
            let has_arith = g.clause.conditions.iter().any(|p| {
                p.args.iter().any(|t| matches!(t, spoon_core::types::Term::Arith { .. }))
            });
            if has_arith {
                // Evaluate via arith module.
                let mut moves = vec![];
                for pred in &g.clause.conditions {
                    for term in &pred.args {
                        if let spoon_core::types::Term::Arith { expr } = term {
                            match arith::eval(ctx, expr) {
                                Ok(value) => moves.push(Move::Answer {
                                    question: sce.clone(),
                                    values: vec![value],
                                    source: None,
                                }),
                                Err(e) => moves.push(Move::Error { message: e.to_string() }),
                            }
                        }
                    }
                }
                if moves.is_empty() {
                    moves.push(unknown_move(g, kind));
                }
                Dispatched::Moves(moves)
            } else {
                Dispatched::Moves(vec![unknown_move(g, kind)])
            }
        }
    };
    Ok(dispatched)
}

// ---- Helpers --------------------------------------------------------------

/// An honest unknown, phrased from the question itself:
/// "Is John happy?" -> "I don't know whether John is happy."
/// "Who owns a cat?" -> "I don't know who owns a cat."
/// "What is the color of the dog?" -> "I don't know the color of the dog."
/// "Where is Mary?" -> "I don't know where Mary is."
fn unknown_move(g: &Grounded, kind: &QuestionKind) -> Move {
    let body = g.clause.sce.trim().trim_end_matches(|c: char| c == '?' || c == '.').trim().to_string();
    let text = match kind {
        QuestionKind::YesNo | QuestionKind::Should => {
            let mut declarative = g.clause.clone();
            declarative.act = Act::Assert;
            let s = spoon_lang::sce::realize(&declarative);
            let s = s.trim().trim_end_matches('.');
            if s.is_empty() {
                format!("I don't know: {body}?")
            } else {
                format!("I don't know whether {}.", lowercase_determiner(s))
            }
        }
        QuestionKind::Where { .. } | QuestionKind::When { .. } => match embedded_copula_question(&body) {
            Some(embedded) => format!("I don't know {embedded}."),
            None => format!("I don't know {}.", lowercase_determiner(&body)),
        },
        _ => match body.strip_prefix("What is ").filter(|np| np.starts_with("the ")) {
            Some(np) => format!("I don't know {np}."),
            None => format!("I don't know {}.", lowercase_determiner(&body)),
        },
    };
    Move::Explain { text }
}

/// "Where is Mary" -> "where Mary is": an embedded question puts the copula
/// after its subject. Anything that is not `wh + copula + subject` is left
/// to the caller.
fn embedded_copula_question(body: &str) -> Option<String> {
    let mut words = body.split_whitespace();
    let wh = words.next()?.to_lowercase();
    let copula = words.next()?;
    if !matches!(copula, "is" | "are" | "was" | "were") {
        return None;
    }
    let subject = words.collect::<Vec<_>>().join(" ");
    if subject.is_empty() {
        return None;
    }
    Some(format!("{wh} {} {copula}", lowercase_determiner(&subject)))
}

/// Lowercase a sentence-initial function word so it can sit mid-sentence;
/// names keep their capital.
fn lowercase_determiner(s: &str) -> String {
    const FUNCTION_WORDS: [&str; 13] =
        ["the", "a", "an", "it", "there", "every", "no", "some", "who", "what", "which", "where", "when"];
    let first = s.split_whitespace().next().unwrap_or("");
    if FUNCTION_WORDS.contains(&first.to_lowercase().as_str()) {
        let mut cs = s.chars();
        match cs.next() {
            Some(c) => c.to_lowercase().collect::<String>() + cs.as_str(),
            None => String::new(),
        }
    } else {
        s.to_string()
    }
}

/// Render a Fact as English with definite noun phrases ("the elephant is
/// bigger than the cat"), falling back to a compact SCE-ish string.
pub(crate) fn render_fact(ctx: &DispatchCtx<'_>, fact: &Fact) -> String {
    if let Some(s) = realize_fact_with(ctx.can, ctx.store, fact, Article::Definite) {
        return s;
    }
    // A typing fact says nothing when describing a thing, but it is the whole
    // reason when it answers "Is Blorp a hero?".
    if fact.pred.0 == "rel.is_a" {
        if let [subject, class] = fact.args.as_slice() {
            let noun = match class {
                Value::Name(n) => ctx
                    .can
                    .concept(&ConceptId(n.clone()))
                    .and_then(|c| c.nouns.first().cloned())
                    .unwrap_or_else(|| n.to_lowercase()),
                other => other.render(),
            };
            let article = if noun.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
            let not = if fact.truth { "" } else { "not " };
            return format!("{} is {not}{article} {noun}", subject.render());
        }
    }
    let verb = ctx.can.action(&fact.pred)
        .and_then(|a| a.verbs.first().cloned())
        .unwrap_or_else(|| fact.pred.0.clone());
    let args: Vec<String> = fact.args.iter().map(|v| v.render()).collect();
    let truth_pfx = if fact.truth { "" } else { "not " };
    format!("{}{} {}", truth_pfx, args.first().cloned().unwrap_or_default(), {
        let mut rest = args.clone();
        if !rest.is_empty() { rest.remove(0); }
        format!("{} {}", verb, rest.join(", "))
    }.trim_start())
}
