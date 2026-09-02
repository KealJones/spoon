//! Property questions that memory cannot answer become computations.
//!
//! "What is the double of 100?" arrives as a wh-question over the possessed
//! noun `double` with owner `100`. When no `rel.double` fact exists, an
//! action whose verb (or id tail) is `double` and whose one required input
//! accepts `100` is planned and executed exactly like `Assistant, double
//! 100!`. Yes/no questions ("Is the double of 3 6?") compute and compare.
//! No LLM involved.

use spoon_core::kernel::Ctx;
use spoon_core::types::{
    ActionId, Goal, Impl, Intent, Quant, QuestionKind, Referent, Role, Signal, Term, Tier, Type, Value,
};

use crate::discourse::{Binding, Grounded};

use super::{DispatchCtx, Dispatched};

/// How the brain should present the value a plan produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Present {
    /// A command: `Move::Result`.
    Result,
    /// A wh-question: `Move::Answer` with source "computed".
    Answer { question: String },
    /// A yes/no question: compare the value with `want`. `subject` is the
    /// noun phrase asked about ("the double of 3") for the explanation.
    YesNo { question: String, subject: String, want: Value },
}

/// Nouns that name a kernel action under a different verb. Ownerless
/// time-component nouns ("What is the hour?") mean "of now": the DateTime
/// input is filled with the current instant in `signals_for`.
const NOUN_ALIASES: &[(&str, &str)] = &[
    ("time", "time.now"),
    ("date", "time.today"),
    ("hour", "time.hour"),
    ("minute", "time.minute"),
    ("year", "time.year"),
    ("month", "time.month"),
    ("weekday", "time.weekday"),
];

/// The possessed (or bare definite) noun a `be` question asks about.
struct PropertyRef {
    noun: String,
    owner: Option<Value>,
    owner_var: Option<String>,
    /// Yes/no questions: the value the user proposed.
    want: Option<Value>,
}

/// Try to answer a property question by running a capability.
pub fn compute(ctx: &mut DispatchCtx<'_>, g: &Grounded, kind: &QuestionKind) -> Option<Dispatched> {
    let pred = g.clause.conditions.first()?;
    if pred.pred != "be" || matches!(kind, QuestionKind::HowMany { .. }) {
        return None;
    }
    let prop = property_ref(g, pred.args.as_slice(), pred.attr.as_deref())?;
    let (action, signals) = capability_for(ctx, &prop)?;
    let sce = g.clause.sce.clone();
    let present = match kind {
        QuestionKind::YesNo | QuestionKind::Should => Present::YesNo {
            question: sce.clone(),
            subject: subject_phrase(&prop),
            want: prop.want?,
        },
        _ => Present::Answer { question: sce.clone() },
    };
    let intent = Intent { goal: Goal::Action { action: action.clone() }, signals, routes: vec![action], sce };
    Some(Dispatched::Plan { intent, moves_before: vec![], present })
}

fn property_ref(g: &Grounded, args: &[Term], attr: Option<&str>) -> Option<PropertyRef> {
    let referent = |t: &Term| match t {
        Term::Var { var } => g.clause.referent(var),
        _ => None,
    };
    let is_property = |r: &Referent| r.noun.is_some() && (r.owner.is_some() || r.quant == Quant::Def);
    let pos = args.iter().position(|t| referent(t).is_some_and(is_property))?;
    let r = referent(&args[pos])?;
    let noun = r.noun.clone()?;

    let (owner, owner_var) = match &r.owner {
        Some(v) => match g.bindings.get(v.as_str()) {
            Some(Binding::Entity(val)) | Some(Binding::Literal(val)) => (Some(val.clone()), Some(v.clone())),
            _ => return None,
        },
        None => (None, None),
    };

    // Yes/no: the proposed value is the attribute or the other argument.
    let want = match attr {
        Some(a) => Some(Value::Text(a.to_string())),
        None => args.iter().enumerate().find(|(i, _)| *i != pos).and_then(|(_, t)| match t {
            Term::Var { var } => match g.bindings.get(var.as_str()) {
                Some(Binding::Entity(v)) | Some(Binding::Literal(v)) => Some(v.clone()),
                _ => None,
            },
            Term::Value { value } => Some(value.clone()),
            _ => None,
        }),
    };
    Some(PropertyRef { noun, owner, owner_var, want })
}

/// "the double of 3", "the reverse of \"abc\"", "the time".
fn subject_phrase(prop: &PropertyRef) -> String {
    match &prop.owner {
        Some(Value::Text(s)) => format!("the {} of \"{s}\"", prop.noun),
        Some(v) => format!("the {} of {}", prop.noun, v.render()),
        None => format!("the {}", prop.noun),
    }
}

/// The runnable action for `prop.noun` whose inputs fit the owner, with the
/// signals that fill them. Kernel actions first, then learned ones, by id.
fn capability_for(ctx: &mut DispatchCtx<'_>, prop: &PropertyRef) -> Option<(ActionId, Vec<Signal>)> {
    let mut ids: Vec<ActionId> = Vec::new();
    if let Some((_, id)) = NOUN_ALIASES.iter().find(|(n, _)| *n == prop.noun) {
        ids.push(ActionId((*id).into()));
    }
    ids.extend(ctx.can.actions_for_verb(&prop.noun).iter().map(|a| a.id.clone()));
    ids.extend(ctx.can.actions().filter(|a| id_tail(&a.id) == prop.noun).map(|a| a.id.clone()));

    let mut candidates: Vec<(u8, ActionId)> = ids
        .into_iter()
        .filter_map(|id| ctx.can.action(&id).map(|a| (a.id.clone(), a.role, a.tier, matches!(a.imp, Impl::Program { .. }))))
        .filter(|(id, role, tier, is_program)| {
            !matches!(role, Role::Relation | Role::Dialog)
                && *tier != Tier::Deprecated
                && (ctx.kernel.has(id) || *is_program)
        })
        .map(|(id, _, tier, _)| ((tier != Tier::Kernel) as u8, id))
        .collect();
    candidates.sort();
    candidates.dedup();

    for (_, id) in candidates {
        if let Some(signals) = signals_for(ctx, &id, prop) {
            return Some((id, signals));
        }
    }
    None
}

fn id_tail(id: &ActionId) -> String {
    id.0.rsplit('.').next().unwrap_or(&id.0).replace('_', "-")
}

/// Signals that fill every required input of `id` from the owner (or from
/// "now" for an ownerless DateTime input). `None` when the action does not fit.
fn signals_for(ctx: &mut DispatchCtx<'_>, id: &ActionId, prop: &PropertyRef) -> Option<Vec<Signal>> {
    let required: Vec<Type> =
        ctx.can.action(id)?.inputs.iter().filter(|i| i.required).map(|i| i.ty.clone()).collect();
    match (&prop.owner, required.as_slice()) {
        (None, []) => Some(vec![]),
        (None, [Type::DateTime]) => {
            let now = call_kernel(ctx, &ActionId("time.now".into()))?;
            Some(vec![Signal { ty: Type::DateTime, value: now, name_hint: None, var: None }])
        }
        (Some(owner), [ty]) if ty.accepts_value(owner) => Some(vec![Signal {
            ty: ty.clone(),
            value: owner.clone(),
            name_hint: None,
            var: prop.owner_var.clone(),
        }]),
        _ => None,
    }
}

fn call_kernel(ctx: &mut DispatchCtx<'_>, id: &ActionId) -> Option<Value> {
    let mut kctx = Ctx::new(ctx.can, ctx.kernel, ctx.host);
    ctx.kernel.call(&mut kctx, id, &[]).ok()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use spoon_core::types::{Act, Clause, Pred};

    use super::*;

    /// True when `g` is a question the property path could take.
    fn is_property_question(g: &Grounded) -> bool {
        matches!(g.clause.act, Act::Question { .. })
            && g.clause.conditions.first().is_some_and(|p| {
                p.pred == "be" && property_ref(g, &p.args, p.attr.as_deref()).is_some()
            })
    }

    fn wh_property(noun: &str, owner: Value) -> Grounded {
        let clause = Clause {
            act: Act::Question { kind: QuestionKind::What { focus: "x3".into() } },
            referents: vec![
                Referent { var: "x2".into(), noun: None, quant: Quant::Literal(owner.clone()), mods: vec![], owner: None, span: None },
                Referent { var: "x3".into(), noun: Some(noun.into()), quant: Quant::Def, mods: vec![], owner: Some("x2".into()), span: None },
                Referent { var: "x1".into(), noun: None, quant: Quant::Wh, mods: vec![], owner: None, span: None },
            ],
            conditions: vec![Pred {
                pred: "be".into(),
                args: vec![Term::Var { var: "x1".into() }, Term::Var { var: "x3".into() }],
                negated: false,
                modal: None,
                adjuncts: vec![],
                attr: None,
            }],
            then: vec![],
            then_referents: vec![],
            sce: format!("What is the {noun} of {}?", owner.render()),
        };
        let mut bindings = HashMap::new();
        bindings.insert("x2".to_string(), Binding::Literal(owner));
        bindings.insert("x1".to_string(), Binding::Query);
        bindings.insert("x3".to_string(), Binding::Unbound);
        Grounded { clause, bindings, new_entities: vec![] }
    }

    #[test]
    fn finds_the_possessed_noun_and_its_owner() {
        let g = wh_property("reverse", Value::text("abc"));
        assert!(is_property_question(&g));
        let p = property_ref(&g, &g.clause.conditions[0].args, None).unwrap();
        assert_eq!(p.noun, "reverse");
        assert_eq!(p.owner, Some(Value::text("abc")));
        assert_eq!(p.want, None);
        assert_eq!(subject_phrase(&p), "the reverse of \"abc\"");
    }

    #[test]
    fn unresolved_owner_is_not_a_property_question() {
        let mut g = wh_property("double", Value::Int(1));
        g.bindings.insert("x2".to_string(), Binding::Unbound);
        assert!(!is_property_question(&g));
    }
}
