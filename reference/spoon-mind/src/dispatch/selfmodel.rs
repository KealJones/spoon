//! Questions about Assistant: wellbeing, identity, capabilities.
//! No LLM involved.

use spoon_core::types::{ActionId, Move, Quant, QuestionKind, Role, Value};

use crate::discourse::{answer_grounded, Answer, DiscourseState, Grounded};

use super::{DispatchCtx, Dispatched};

/// Returns true if the question involves Assistant as a key referent.
pub fn is_about_assistant(g: &Grounded) -> bool {
    // Check referents for Named("Assistant").
    g.clause.referents.iter().any(|r| matches!(&r.quant, Quant::Named(n) if n == "Assistant"))
}

/// Dispatch a question about Assistant.
pub fn handle_assistant_question(
    ctx: &mut DispatchCtx<'_>,
    g: &Grounded,
    kind: &QuestionKind,
    _state: &DiscourseState,
) -> anyhow::Result<Dispatched> {
    let sce = g.clause.sce.clone();
    let pred = g.clause.conditions.first();

    // Capabilities question.
    if is_capability_question(g, pred) {
        return handle_capabilities(ctx);
    }

    // Opinion/preference question addressed to Assistant.
    // (handled by opinion.rs, but route capability-Can questions here)
    if let Some(p) = pred {
        if p.pred == "can" || p.modal.is_some() {
            if let Some(verb) = capability_verb_from_pred(p) {
                let has_action = !ctx.can.actions_for_verb(&verb).is_empty();
                return Ok(Dispatched::Moves(vec![Move::YesNo {
                    question: sce,
                    answer: has_action,
                    because: if has_action {
                        Some(format!("i have an action for '{}'", verb))
                    } else {
                        Some(format!("no action for '{}' yet", verb))
                    },
                }]));
            }
        }
    }

    // Wellbeing / mood / status / feeling questions.
    if is_wellbeing_question(g, pred) {
        return handle_wellbeing(ctx, &sce);
    }

    // Identity questions (who/what/is-a).
    // Try standard answer_grounded first.
    match answer_grounded(ctx.can, ctx.store, g, kind)? {
        Answer::Values(vs) if !vs.is_empty() => {
            return Ok(Dispatched::Moves(vec![Move::Answer {
                question: sce,
                values: vs,
                source: Some("memory".into()),
            }]));
        }
        Answer::YesNo(b, fact) => {
            let because = fact.map(|f| {
                let verb = ctx.can.action(&f.pred)
                    .and_then(|a| a.verbs.first().cloned())
                    .unwrap_or_else(|| f.pred.0.clone());
                format!("{} {} {}", f.args.iter().map(|v| v.render()).collect::<Vec<_>>().join(", "), if f.truth { "is" } else { "is not" }, verb)
            });
            return Ok(Dispatched::Moves(vec![Move::YesNo { question: sce, answer: b, because }]));
        }
        _ => {}
    }

    // Identity fallbacks.
    let identity_text = identity_fallback(pred);
    Ok(Dispatched::Moves(vec![Move::Explain { text: identity_text }]))
}

fn is_wellbeing_question(g: &Grounded, pred: Option<&spoon_core::types::Pred>) -> bool {
    let Some(p) = pred else { return false; };
    let wellbeing_nouns = ["wellbeing", "mood", "status", "feeling", "health", "happiness"];
    // Check referent nouns.
    let has_wellbeing_noun = g.clause.referents.iter().any(|r| {
        r.noun.as_deref().map(|n| wellbeing_nouns.contains(&n)).unwrap_or(false)
    });
    if has_wellbeing_noun { return true; }
    // Also match "Is Assistant fine/ok/happy/good?" style.
    let wellbeing_attrs = ["fine", "ok", "good", "happy", "well", "alive", "alright", "doing"];
    if p.pred == "be" {
        if let Some(attr) = &p.attr {
            if wellbeing_attrs.iter().any(|a| attr.contains(a)) { return true; }
        }
    }
    false
}

fn handle_wellbeing(ctx: &mut DispatchCtx<'_>, sce: &str) -> anyhow::Result<Dispatched> {
    // Try to answer from stored facts.
    let preds_to_try = ["rel.wellbeing", "rel.mood", "rel.status", "rel.feeling"];
    for pred_id in preds_to_try {
        let facts = ctx.store.query_facts(
            &ActionId(pred_id.into()),
            &[Some(Value::Name("Assistant".into())), None],
        ).unwrap_or_default();
        if let Some(f) = facts.first() {
            if let Some(v) = f.args.get(1) {
                return Ok(Dispatched::Moves(vec![Move::Answer {
                    question: sce.to_string(),
                    values: vec![v.clone()],
                    source: Some("memory".into()),
                }]));
            }
        }
    }
    // Also try rel.is facts about Assistant with wellbeing-ish args.
    let is_facts = ctx.store.query_facts(
        &ActionId("rel.is".into()),
        &[Some(Value::Name("Assistant".into())), None],
    ).unwrap_or_default();
    if let Some(f) = is_facts.first() {
        if let Some(v) = f.args.get(1) {
            return Ok(Dispatched::Moves(vec![Move::Answer {
                question: sce.to_string(),
                values: vec![v.clone()],
                source: Some("memory".into()),
            }]));
        }
    }
    // Fallback.
    Ok(Dispatched::Moves(vec![Move::Explain { text: "doing fine, thanks for asking".into() }]))
}

fn is_capability_question(g: &Grounded, pred: Option<&spoon_core::types::Pred>) -> bool {
    let Some(p) = pred else { return false; };
    // "What can Assistant do?" - pred might be "can" or "do".
    let capability_nouns = ["capability", "capabilities", "limit", "limits", "ability", "abilities"];
    let has_cap_noun = g.clause.referents.iter().any(|r| {
        r.noun.as_deref().map(|n| capability_nouns.contains(&n)).unwrap_or(false)
    });
    if has_cap_noun { return true; }
    // "What can Assistant do?" -> pred "can" or modal "can" with verb "do".
    if p.pred == "can" || p.modal.as_ref().map(|m| matches!(m, spoon_core::types::Modal::Can)).unwrap_or(false) {
        return true;
    }
    // Question focus is "do" verb.
    if p.pred == "do" {
        return true;
    }
    false
}

fn capability_verb_from_pred(pred: &spoon_core::types::Pred) -> Option<String> {
    // "Can Assistant calculate?" -> pred "can" args [Assistant, Term::Var{verb?}]
    // Or modal "can" on a different verb.
    if pred.modal.is_some() && pred.pred != "be" {
        return Some(pred.pred.clone());
    }
    None
}

fn handle_capabilities(ctx: &mut DispatchCtx<'_>) -> anyhow::Result<Dispatched> {
    // List up to 8 verbs grouped by module.
    let actions: Vec<&spoon_core::types::Action> = ctx.can.actions().collect();

    let mut math_verbs = vec![];
    let mut text_verbs = vec![];
    let mut list_verbs = vec![];
    let mut time_verbs = vec![];
    let mut fs_verbs = vec![];
    let mut http_verbs = vec![];
    let mut learned_verbs = vec![];

    for a in &actions {
        if a.role == Role::Dialog || a.tier == spoon_core::types::Tier::Deprecated {
            continue;
        }
        let id = &a.id.0;
        let verb = a.verbs.first().cloned().unwrap_or_else(|| id.clone());
        if id.starts_with("math.") { math_verbs.push(verb); }
        else if id.starts_with("text.") { text_verbs.push(verb); }
        else if id.starts_with("list.") { list_verbs.push(verb); }
        else if id.starts_with("time.") { time_verbs.push(verb); }
        else if id.starts_with("fs.") { fs_verbs.push(verb); }
        else if id.starts_with("http.") { http_verbs.push(verb); }
        else if a.tier == spoon_core::types::Tier::Provisional || a.tier == spoon_core::types::Tier::Consolidated {
            learned_verbs.push(verb);
        }
    }

    let mut lines = vec![
        "here's what i can do:".to_string(),
        format!("- math: {}", take_up_to(&math_verbs, 4, "calculate, add, subtract, multiply, divide")),
        "- remember facts and answer questions about them".to_string(),
        "- learn new capabilities from examples".to_string(),
    ];
    if !text_verbs.is_empty() {
        lines.push(format!("- text: {}", take_up_to(&text_verbs, 3, "split, join, length")));
    }
    if !list_verbs.is_empty() {
        lines.push(format!("- lists: {}", take_up_to(&list_verbs, 3, "map, filter, count")));
    }
    if !time_verbs.is_empty() {
        lines.push(format!("- time: {}", take_up_to(&time_verbs, 2, "now, format")));
    }
    if !learned_verbs.is_empty() {
        lines.push(format!("- learned: {}", take_up_to(&learned_verbs, 3, "")));
    }

    Ok(Dispatched::Moves(vec![Move::Explain { text: lines.join("\n") }]))
}

fn take_up_to(verbs: &[String], n: usize, fallback: &str) -> String {
    if verbs.is_empty() {
        return fallback.to_string();
    }
    verbs.iter().take(n).cloned().collect::<Vec<_>>().join(", ")
}

fn identity_fallback(pred: Option<&spoon_core::types::Pred>) -> String {
    if let Some(p) = pred {
        if p.pred == "be" {
            return "i am Spoon, a conversational AI built by Keal".to_string();
        }
    }
    "i am Spoon".to_string()
}
