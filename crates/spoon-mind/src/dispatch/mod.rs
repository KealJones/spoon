//! Dispatch layer: routes grounded clauses to moves, plans, or teacher asks.
//! Deterministic - no LLM in the interior.

pub mod arith;
mod command;
mod dialog;
mod opinion;
mod selfmodel;

use spoon_core::can::Can;
use spoon_core::kernel::Kernel;
use spoon_core::store::Store;
use spoon_core::types::{
    Act, ActionId, Fact, Intent, Move, QuestionKind, Role, Signal, Value,
};

use crate::discourse::{answer_grounded, assert_grounded, Answer, DiscourseState, FactWriter, Grounded};

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
    /// A Command resolved to a known action; the brain plans and executes it.
    Plan { intent: Intent, moves_before: Vec<Move> },
    /// A Command whose verb matches no action.
    UnknownCapability { verb: String, signals: Vec<Signal>, sce: String, fallback: Vec<Move> },
    /// Something the teacher could supply.
    NeedsTeacher { ask: TeacherAsk, fallback: Vec<Move> },
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
            Dispatched::Plan { intent, mut moves_before } => {
                moves_before.splice(0..0, accumulated_moves);
                return Ok(Dispatched::Plan { intent, moves_before });
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

    // 2. Opinion questions.
    if opinion::is_opinion_question(g) {
        return opinion::handle_opinion(ctx, g, state);
    }

    // 3. Advice questions.
    if opinion::is_advice_question(g, kind) {
        return opinion::handle_advice(ctx, g, state);
    }

    // 4. Standard fact lookup.
    let answer = answer_grounded(ctx.can, ctx.store, g, kind)?;
    let dispatched = match answer {
        Answer::Values(vs) => {
            if vs.is_empty() {
                Dispatched::Moves(vec![Move::Answer { question: sce, values: vec![], source: None }])
            } else {
                Dispatched::Moves(vec![Move::Answer { question: sce, values: vs, source: Some("memory".into()) }])
            }
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
        Answer::Unknown { .. } => {
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
                    moves.push(Move::Answer { question: sce, values: vec![], source: None });
                }
                Dispatched::Moves(moves)
            } else {
                Dispatched::Moves(vec![Move::Answer { question: sce, values: vec![], source: None }])
            }
        }
    };
    Ok(dispatched)
}

// ---- Helpers --------------------------------------------------------------

/// Render a Fact as a compact SCE-ish string.
pub(crate) fn render_fact(ctx: &DispatchCtx<'_>, fact: &Fact) -> String {
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
