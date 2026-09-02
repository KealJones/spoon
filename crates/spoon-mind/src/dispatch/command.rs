//! Command clause -> Intent or UnknownCapability.
//! Resolves verb to a CAN action and maps bindings to Signals.
//! No LLM involved.

use spoon_core::types::{
    ActionId, Goal, Intent, Move, Quant, Role, Signal, Term, Type, Value,
};

use crate::discourse::Binding;

use crate::discourse::DiscourseState;

use super::{arith, DispatchCtx, Dispatched};

/// Dispatch a Command clause.
pub fn dispatch_command(
    ctx: &mut DispatchCtx<'_>,
    g: &crate::discourse::Grounded,
    state: &DiscourseState,
) -> anyhow::Result<Dispatched> {
    let pred = match g.clause.conditions.first() {
        Some(p) => p,
        None => return Ok(Dispatched::Moves(vec![Move::Error { message: "empty command".into() }])),
    };

    let verb = pred.pred.as_str();
    let sce = g.clause.sce.clone();

    // Dialog commands addressed to Assistant as commands.
    let dialog_result = try_dialog_command(ctx, verb, state);
    if let Some(d) = dialog_result {
        return Ok(d);
    }

    // If any arg is Term::Arith and verb is a calc verb -> evaluate.
    let has_arith = pred.args.iter().any(|t| matches!(t, Term::Arith { .. }));
    if has_arith && is_calc_verb(verb) {
        return eval_arith_command(ctx, pred, &sce);
    }

    // Resolve verb to an action in the CAN.
    let action = resolve_verb(ctx, verb);

    if let Some(action_id) = action {
        let signals = build_signals(ctx, g, pred);
        Ok(Dispatched::Plan {
            intent: Intent { goal: Goal::Action { action: action_id.clone() }, signals, routes: vec![action_id], sce },
            moves_before: vec![],
        })
    } else {
        // Unknown verb.
        let signals = build_signals(ctx, g, pred);
        let sig_summary: String = signals
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}: {}", s.name_hint.as_deref().unwrap_or(&format!("arg{}", i)), s.ty))
            .collect::<Vec<_>>()
            .join(", ");
        let signature = if sig_summary.is_empty() {
            format!("() -> ?")
        } else {
            format!("({}) -> ?", sig_summary)
        };
        Ok(Dispatched::UnknownCapability {
            verb: verb.to_string(),
            signals,
            sce,
            fallback: vec![
                Move::CannotDo { what: verb.to_string(), reason: "i don't have that capability yet".into() },
                Move::AskExamples { capability: verb.to_string(), signature },
            ],
        })
    }
}

/// Evaluate all Term::Arith in the predicate args and return Results.
fn eval_arith_command(
    ctx: &mut DispatchCtx<'_>,
    pred: &spoon_core::types::Pred,
    sce: &str,
) -> anyhow::Result<Dispatched> {
    let _ = sce;
    let mut moves = vec![];
    for term in &pred.args {
        if let Term::Arith { expr } = term {
            match arith::eval(ctx, expr) {
                Ok(value) => moves.push(Move::Result { action: ActionId("math.eval".into()), value, steps: 1 }),
                Err(e) => moves.push(Move::Error { message: e.to_string() }),
            }
        }
    }
    if moves.is_empty() {
        moves.push(Move::Error { message: "no arithmetic expression found".into() });
    }
    Ok(Dispatched::Moves(moves))
}

/// Check if verb is a calculation verb.
fn is_calc_verb(verb: &str) -> bool {
    matches!(verb, "calculate" | "compute" | "evaluate" | "solve")
}

/// Try to resolve a verb to a CAN action, trying hyphen variants.
pub fn resolve_verb(ctx: &DispatchCtx<'_>, verb: &str) -> Option<ActionId> {
    // Direct lookup.
    let actions = ctx.can.actions_for_verb(verb);
    if let Some(a) = actions.iter().find(|a| a.role != Role::Dialog) {
        return Some(a.id.clone());
    }
    // Also accept any action with this verb.
    if let Some(a) = actions.first() {
        return Some(a.id.clone());
    }
    // Try removing trailing preposition from phrasal verb (look-for -> look).
    if let Some(pos) = verb.rfind('-') {
        let stem = &verb[..pos];
        let acts = ctx.can.actions_for_verb(stem);
        if let Some(a) = acts.first() {
            return Some(a.id.clone());
        }
    }
    None
}

/// Build Signals from the predicate bindings.
pub fn build_signals(ctx: &DispatchCtx<'_>, g: &crate::discourse::Grounded, pred: &spoon_core::types::Pred) -> Vec<Signal> {
    let mut signals = vec![];

    for (i, term) in pred.args.iter().enumerate() {
        if let Some(sig) = term_to_signal(ctx, g, term, None) {
            // Skip the "Assistant" signal (first arg if Named Assistant in a command).
            if i == 0 {
                if let Value::Name(n) = &sig.value {
                    if n == "Assistant" {
                        continue;
                    }
                }
            }
            signals.push(sig);
        }
    }
    for (prep, term) in &pred.adjuncts {
        if let Some(mut sig) = term_to_signal(ctx, g, term, Some(prep.clone())) {
            sig.name_hint = Some(prep.clone());
            signals.push(sig);
        }
    }
    signals
}

fn term_to_signal(
    ctx: &DispatchCtx<'_>,
    g: &crate::discourse::Grounded,
    term: &Term,
    name_hint: Option<String>,
) -> Option<Signal> {
    match term {
        Term::Var { var } => match g.bindings.get(var.as_str())? {
            Binding::Entity(v) => {
                // Determine type: check if entity has a concept.
                let ty = if let Value::Name(n) = v {
                    // Check CAN for concept.
                    let concept_id = spoon_core::types::ConceptId(n.clone());
                    if ctx.can.concept(&concept_id).is_some() {
                        Type::Concept(concept_id)
                    } else {
                        Type::Name
                    }
                } else {
                    v.type_of()
                };
                Some(Signal { ty, value: v.clone(), name_hint, var: Some(var.clone()) })
            }
            Binding::Literal(v) => {
                Some(Signal { ty: v.type_of(), value: v.clone(), name_hint, var: Some(var.clone()) })
            }
            _ => None,
        },
        Term::Value { value } => {
            Some(Signal { ty: value.type_of(), value: value.clone(), name_hint, var: None })
        }
        Term::Arith { .. } => None,
        Term::Sub { clause } => {
            let v = Value::Text(clause.sce.clone());
            Some(Signal { ty: Type::Text, value: v, name_hint, var: None })
        }
    }
}

/// Handle dialog-style commands addressed to Assistant.
fn try_dialog_command(ctx: &mut DispatchCtx<'_>, verb: &str, state: &DiscourseState) -> Option<Dispatched> {
    let last_text = state.last_result.as_ref().map(|v| v.render())
        .or_else(|| state.last_clauses.first().map(|c| c.sce.clone()))
        .unwrap_or_else(|| "the previous statement".to_string());

    match verb {
        "explain" | "repeat" | "clarify" | "elaborate" => {
            Some(Dispatched::Moves(vec![Move::Explain { text: last_text }]))
        }
        "tell" => {
            // "tell User a fun fact"
            let facts = ctx.store.facts_about(&Value::Name("seed".into())).unwrap_or_default();
            let text = facts.first()
                .map(|f| f.args.iter().map(|a| a.render()).collect::<Vec<_>>().join(" "))
                .unwrap_or_else(|| "here's a fun fact: i store things you tell me and remember them".to_string());
            Some(Dispatched::Moves(vec![Move::Info { text, source: "memory".into() }]))
        }
        "describe" => {
            let facts = ctx.store.facts_about(&Value::Name("Assistant".into())).unwrap_or_default();
            let lines: Vec<String> = facts.iter().take(5)
                .map(|f| format!("{}: {}", f.pred.0, f.args.iter().map(|a| a.render()).collect::<Vec<_>>().join(", ")))
                .collect();
            let text = if lines.is_empty() {
                "i am Spoon, a conversational AI".to_string()
            } else {
                lines.join("; ")
            };
            Some(Dispatched::Moves(vec![Move::Explain { text }]))
        }
        "stop" | "stop-asking" => {
            Some(Dispatched::Moves(vec![Move::Ack { summary: "ok, stopping".into() }]))
        }
        "help" => {
            // "help User decide" -> advice path (no situation context here)
            let situation = state.last_clauses.iter().map(|c| c.sce.as_str()).collect::<Vec<_>>().join(" ");
            Some(Dispatched::NeedsTeacher {
                ask: super::TeacherAsk::Advice { situation: situation.clone(), keywords: vec![] },
                fallback: vec![
                    Move::Reflect { summary: if situation.is_empty() { "let me understand the situation".to_string() } else { situation } },
                    Move::Clarify { question: "what have you tried so far?".into(), options: vec![], slot_type: None },
                ],
            })
        }
        _ => None,
    }
}
