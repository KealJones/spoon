//! Dialog assert policy: mirrored moves for User dialog acts and small-talk.
//! No LLM involved.

use spoon_core::types::{Move, Quant, Role, Term, Value};

use crate::discourse::{assert_grounded, Answer, AssertOutcome, DiscourseState, FactWriter, Grounded};

use super::{render_fact, DispatchCtx, Dispatched};

/// Dispatch an Assert clause where the verb is a Dialog action with User subject.
/// Returns Some if it was a dialog move, None if it should fall through to store.
pub fn try_dialog_assert(
    ctx: &mut DispatchCtx<'_>,
    g: &Grounded,
    state: &DiscourseState,
) -> Option<Dispatched> {
    let pred = g.clause.conditions.first()?;

    // Subject must be Named("User").
    let subj_var = match pred.args.first()? {
        Term::Var { var } => var.clone(),
        _ => return None,
    };
    let subj_ref = g.clause.referent(&subj_var)?;
    if !matches!(&subj_ref.quant, Quant::Named(n) if n == "User") {
        return None;
    }

    let verb = pred.pred.as_str();

    // Resolve to dialog action.
    if let Some(action_id) = find_dialog_action(ctx, verb) {
        let moves = mirror_dialog_move(ctx, &action_id.0, state);
        return Some(Dispatched::Moves(moves));
    }

    // Handle confused/not-understanding by copula attr or negated "understand".
    if is_confused(pred) {
        let text = last_reply_text(state);
        return Some(Dispatched::Moves(vec![Move::Explain { text }]));
    }

    None
}

/// Resolve a verb (with variants) to a Dialog-role action ID.
fn find_dialog_action(ctx: &DispatchCtx<'_>, verb: &str) -> Option<spoon_core::types::ActionId> {
    // Try direct lookup.
    if let Some(a) = ctx.can.actions_for_verb(verb).iter().find(|a| a.role == Role::Dialog) {
        return Some(a.id.clone());
    }
    // Try common aliases not in the phrasing table.
    let alias: &str = match verb {
        "says-goodbye-to" | "say-goodbye-to" | "farewell" => "say-goodbye",
        "thank" => "thank",
        "encourage" => "empathize",
        _ => verb,
    };
    if alias != verb {
        if let Some(a) = ctx.can.actions_for_verb(alias).iter().find(|a| a.role == Role::Dialog) {
            return Some(a.id.clone());
        }
    }
    // Strip trailing preposition: say-goodbye-to -> say-goodbye.
    if let Some(pos) = verb.rfind('-') {
        let stem = &verb[..pos];
        if let Some(a) = ctx.can.actions_for_verb(stem).iter().find(|a| a.role == Role::Dialog) {
            return Some(a.id.clone());
        }
    }
    None
}

fn is_confused(pred: &spoon_core::types::Pred) -> bool {
    // "User is confused" -> be copula with attr "confused"
    if pred.pred == "be" {
        if let Some(attr) = &pred.attr {
            if attr == "confused" { return true; }
        }
    }
    // "User does not understand" -> negated understand
    if pred.pred == "understand" && pred.negated {
        return true;
    }
    false
}

fn last_reply_text(state: &DiscourseState) -> String {
    state.last_result.as_ref().map(|v| v.render())
        .or_else(|| state.last_clauses.first().map(|c| c.sce.clone()))
        .unwrap_or_else(|| "my previous reply".to_string())
}

/// Produce the mirrored Move(s) for a dialog action ID.
fn mirror_dialog_move(ctx: &DispatchCtx<'_>, action_id: &str, state: &DiscourseState) -> Vec<Move> {
    match action_id {
        "dialog.greet" => vec![Move::Greet { returning: ctx.returning_user }],
        "dialog.thanks" => vec![Move::Ack { summary: "you're welcome".into() }],
        "dialog.farewell" => vec![Move::Farewell],
        "dialog.ack" => {
            let summary = last_clause_summary(state);
            vec![Move::Ack { summary }]
        }
        "dialog.empathize" | "dialog.reflect" => {
            let summary = last_clause_summary(state);
            vec![Move::Thanks, Move::Reflect { summary }]
        }
        "dialog.refuse" => {
            vec![Move::Ack { summary: "understood, dropping it".into() }]
        }
        "dialog.explain" | "dialog.clarify" => {
            let text = last_reply_text(state);
            vec![Move::Ack { summary: text }]
        }
        _ => vec![Move::Ack { summary: "ok".into() }],
    }
}

fn last_clause_summary(state: &DiscourseState) -> String {
    state.last_clauses.first()
        .map(|c| compact_sce(&c.sce))
        .unwrap_or_else(|| "ok".to_string())
}

/// Compact an SCE sentence for use as a summary (strip period, lowercase first char unless a Name).
pub fn compact_sce(sce: &str) -> String {
    let s = sce.trim_end_matches('.');
    if s.is_empty() { return "ok".to_string(); }
    // Lowercase first char only if not a proper name (uppercase + more).
    let mut chars = s.chars();
    match chars.next() {
        None => s.to_string(),
        Some(first) => {
            if first.is_uppercase() && chars.next().map(|c| c.is_lowercase()).unwrap_or(false) {
                first.to_lowercase().to_string() + &s[first.len_utf8()..]
            } else {
                s.to_string()
            }
        }
    }
}

/// Assert the clause and apply small-talk policy.
pub fn dispatch_store_assert(
    ctx: &mut DispatchCtx<'_>,
    g: &Grounded,
    _state: &DiscourseState,
) -> anyhow::Result<Dispatched> {
    let mut fw = FactWriter { can: ctx.can, store: ctx.store };
    let outcome = assert_grounded(&mut fw, g, "user", ctx.episode_id)?;

    match outcome {
        AssertOutcome::Stored { facts, .. } => {
            let pred = g.clause.conditions.first();
            let moves = small_talk_policy(g, pred, &facts);
            Ok(Dispatched::Moves(moves))
        }
        AssertOutcome::Contradiction { existing, incoming } => {
            let existing_rendered = render_fact(ctx, &existing);
            let incoming_rendered = render_fact(ctx, &incoming);
            Ok(Dispatched::Moves(vec![Move::Clarify {
                question: format!(
                    "earlier you said {}; now {}. which is right?",
                    existing_rendered, incoming_rendered
                ),
                options: vec![existing_rendered, incoming_rendered],
                slot_type: None,
            }]))
        }
        AssertOutcome::Universal { rule_id } => {
            Ok(Dispatched::Moves(vec![Move::Learned { what: g.clause.sce.clone() }]))
        }
    }
}

/// Apply small-talk policy: check if subject is User-related and produce empathy/reflection.
fn small_talk_policy(
    g: &Grounded,
    pred: Option<&spoon_core::types::Pred>,
    _facts: &[spoon_core::types::Fact],
) -> Vec<Move> {
    let Some(pred) = pred else {
        return vec![Move::Ack { summary: compact_sce(&g.clause.sce) }];
    };

    let subj_is_user = pred.args.first().and_then(|t| {
        if let Term::Var { var } = t {
            g.clause.referent(var)
        } else {
            None
        }
    }).map(|r| matches!(&r.quant, Quant::Named(n) if n == "User")).unwrap_or(false);

    // Also check possessive subject: "User's X" has owner=User.
    let subj_owner_is_user = pred.args.first().and_then(|t| {
        if let Term::Var { var } = t {
            g.clause.referent(var)
        } else {
            None
        }
    }).and_then(|r| r.owner.as_ref()).and_then(|owner_var| g.clause.referent(owner_var))
        .map(|r| matches!(&r.quant, Quant::Named(n) if n == "User"))
        .unwrap_or(false);

    if !subj_is_user && !subj_owner_is_user {
        return vec![Move::Ack { summary: compact_sce(&g.clause.sce) }];
    }

    // Classify the predicate.
    let verb = pred.pred.as_str();
    let attr = pred.attr.as_deref().unwrap_or("");

    // Feeling / state verbs.
    let feeling = classify_feeling(verb, attr, pred.negated, pred.modal.is_some());
    if let Some(feeling) = feeling {
        let about = pred.adjuncts.first()
            .map(|(_, t)| match t {
                Term::Value { value } => value.render(),
                Term::Var { var } => g.clause.referent(var)
                    .and_then(|r| r.noun.clone())
                    .unwrap_or_else(|| "this".to_string()),
                _ => "this".to_string(),
            })
            .or_else(|| {
                pred.args.get(1).and_then(|t| {
                    if let Term::Var { var } = t {
                        g.clause.referent(var).and_then(|r| r.noun.clone())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_else(|| "this".to_string());
        return vec![Move::Empathize { feeling, about }];
    }

    // Preference verbs.
    if matches!(verb, "like" | "dislike" | "love" | "hate" | "prefer" | "want") {
        let summary = compact_sce(&g.clause.sce);
        return vec![Move::Ack { summary }];
    }

    // Belief verbs -> Reflect.
    if matches!(verb, "believe" | "think" | "know" | "say") {
        // Check for embedded clause (Sub).
        let summary = pred.args.iter().find_map(|t| {
            if let Term::Sub { clause } = t {
                Some(compact_sce(&clause.sce))
            } else {
                None
            }
        }).unwrap_or_else(|| compact_sce(&g.clause.sce));
        return vec![Move::Reflect { summary }];
    }

    vec![Move::Ack { summary: compact_sce(&g.clause.sce) }]
}

fn classify_feeling(verb: &str, attr: &str, negated: bool, has_modal: bool) -> Option<String> {
    // "is tired/bored/hungry/stressed/happy/sad/in-pain/exhausted"
    let feeling_attrs = ["tired", "bored", "hungry", "stressed", "happy", "sad", "in-pain", "exhausted", "anxious", "overwhelmed", "lonely"];
    if verb == "be" || verb == "is" {
        for f in feeling_attrs {
            if attr.contains(f) {
                return Some(f.to_string());
            }
        }
    }
    // "cannot sleep" - negated modal + sleep verb
    if (verb == "sleep" && (negated || has_modal)) {
        return Some("unable to sleep".to_string());
    }
    // "is in pain"
    if verb == "be" && attr.contains("pain") {
        return Some("in pain".to_string());
    }
    // "has a bad day"
    if verb == "have" && attr.contains("bad") {
        return Some("a bad day".to_string());
    }
    // "wants to give up"
    if verb == "want" && attr.contains("give") {
        return Some("overwhelmed".to_string());
    }
    // Direct verb feelings.
    match verb {
        "tire" | "exhaust" => Some("tired".to_string()),
        "bore" => Some("bored".to_string()),
        "stress" | "overwhelm" => Some("stressed".to_string()),
        _ => None,
    }
}
