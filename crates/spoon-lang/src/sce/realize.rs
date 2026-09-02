//! Clause -> SCE string realizer.
//!
//! Produces a sentence that re-parses to an equal Clause (modulo var names).
//! Handles all Act variants, all Quant values, modals, negation, copula,
//! embedded sub-clauses, arithmetic, rules, and possessives.

use spoon_core::types::clause::{
    Act, ArithExpr, Clause, Modal, Pred, Quant, QuestionKind, Referent, Term,
};
use spoon_core::types::value::Value;

use super::arith::render_arith;
use super::lemma::conjugate_3sg;

/// Realize a Clause into a valid SCE sentence.
pub fn realize(clause: &Clause) -> String {
    match &clause.act {
        Act::Assert => realize_assert(clause),
        Act::Command => realize_command(clause),
        Act::Question { kind } => realize_question(clause, kind),
        Act::Rule => realize_rule(clause),
    }
}

// ---------------------------------------------------------------------------
// Assert
// ---------------------------------------------------------------------------

fn realize_assert(clause: &Clause) -> String {
    if clause.conditions.is_empty() {
        // "There is NP"
        if let Some(r) = clause.referents.first() {
            return format!("There is {}.", realize_ref_np(clause, r, &clause.referents));
        }
        return String::new();
    }
    let pred = &clause.conditions[0];
    // "It is possible/false that S"
    if pred.pred == "be" && pred.args.len() >= 2 {
        if let Some(ref_var) = pred.args.first() {
            if let Term::Var { var: v } = ref_var {
                if clause.referents.iter().any(|r| &r.var == v && matches!(r.quant, Quant::Named(ref n) if n == "It")) {
                    if let Some(attr) = &pred.attr {
                        if let Some(Term::Sub { clause: sub }) = pred.args.get(1) {
                            return format!("It is {} that {}", attr, realize(sub).trim_end_matches('.'));
                        }
                    }
                }
            }
        }
    }
    // Single-predicate or multi-predicate declarative
    // Find subject (first arg of first condition)
    let subj_var = match pred.args.first() {
        Some(Term::Var { var: v }) => v.clone(),
        _ => String::new(),
    };
    // Check if subject is "There" existential (referent has no pred from subj)
    // Determine if this looks like a "There is" sentence
    let is_existential = clause.conditions.is_empty()
        || (clause.referents.len() == 1 && clause.conditions.is_empty());

    let subj_str = realize_var_np(clause, &subj_var, &clause.referents);
    let mut parts = vec![];
    for (i, cond) in clause.conditions.iter().enumerate() {
        let s = realize_pred(clause, cond, &clause.referents, i == 0);
        parts.push(s);
    }
    let body = parts.join(" and ");
    format!("{} {}.", subj_str, body)
}

fn realize_pred(clause: &Clause, pred: &Pred, refs: &[Referent], is_first: bool) -> String {
    let subj_var = match pred.args.first() {
        Some(Term::Var { var: v }) => v.as_str(),
        _ => "",
    };
    let modal_str = realize_modal(&pred.modal, pred.negated);
    let use_base = pred.modal.is_some() || pred.negated;

    match pred.pred.as_str() {
        "be" => realize_be_pred(clause, pred, refs, subj_var, &modal_str, use_base),
        _ => realize_verb_pred(clause, pred, refs, subj_var, &modal_str, use_base, is_first),
    }
}

fn realize_be_pred(clause: &Clause, pred: &Pred, refs: &[Referent], _subj_var: &str, modal_str: &str, _use_base: bool) -> String {
    let negated = pred.negated;
    let be = if modal_str.is_empty() {
        if negated { "is not".to_string() } else { "is".to_string() }
    } else {
        format!("{} be", modal_str)
    };
    let adj_str = if let Some(ref attr) = pred.attr {
        // Comparative: "smarter-than"
        if attr.ends_with("-than") {
            let base_adj = &attr[..attr.len() - 5]; // strip "-than"
            if let Some(cmp_term) = pred.args.get(1) {
                let cmp_str = realize_term(clause, cmp_term, refs);
                return format!("{} {} than {}", be, base_adj, cmp_str);
            }
        }
        attr.replace('-', " ")
    } else {
        String::new()
    };

    if pred.args.len() >= 2 && pred.attr.is_none() {
        // is-a or sub-clause
        let obj = realize_term(clause, &pred.args[1], refs);
        let pp = realize_adjuncts(&pred.adjuncts, clause, refs);
        format!("{} {}{}", be, obj, pp)
    } else if !adj_str.is_empty() {
        let pp = realize_adjuncts(&pred.adjuncts, clause, refs);
        format!("{} {}{}", be, adj_str, pp)
    } else {
        be
    }
}

fn realize_verb_pred(clause: &Clause, pred: &Pred, refs: &[Referent], subj_var: &str, modal_str: &str, use_base: bool, _is_first: bool) -> String {
    let verb = if use_base {
        pred.pred.clone()
    } else {
        conjugate_3sg(&pred.pred)
    };

    let neg_prefix = if pred.negated && pred.modal.is_none() {
        "does not ".to_string()
    } else {
        String::new()
    };

    let mod_prefix = if !modal_str.is_empty() {
        format!("{} ", modal_str)
    } else {
        String::new()
    };

    // Build args (skip first which is subject)
    let mut obj_parts = vec![];
    for arg in pred.args.iter().skip(1) {
        obj_parts.push(realize_term(clause, arg, refs));
    }
    let obj_str = obj_parts.join(" ");
    let pp = realize_adjuncts(&pred.adjuncts, clause, refs);
    let obj_full = if obj_str.is_empty() {
        pp
    } else if pp.is_empty() {
        format!(" {}", obj_str)
    } else {
        format!(" {}{}", obj_str, pp)
    };

    format!("{}{}{}{}", mod_prefix, neg_prefix, verb, obj_full)
}

fn realize_modal(modal: &Option<Modal>, negated: bool) -> String {
    match modal {
        None => String::new(),
        Some(Modal::Can) => if negated { "cannot".to_string() } else { "can".to_string() },
        Some(Modal::Should) => "should".to_string(),
        Some(Modal::Must) => "must".to_string(),
        Some(Modal::May) => "may".to_string(),
    }
}

fn realize_adjuncts(adjuncts: &[(String, Term)], clause: &Clause, refs: &[Referent]) -> String {
    if adjuncts.is_empty() { return String::new(); }
    let parts: Vec<String> = adjuncts.iter().map(|(prep, term)| {
        format!(" {} {}", prep, realize_term(clause, term, refs))
    }).collect();
    parts.join("")
}

fn realize_term(clause: &Clause, term: &Term, refs: &[Referent]) -> String {
    match term {
        Term::Var { var: v } => realize_var_np(clause, v, refs),
        Term::Value { value } => realize_value(value),
        Term::Sub { clause: sub } => format!("that {}", realize(sub).trim_end_matches('.').trim_start_matches("that ")),
        Term::Arith { expr } => render_arith(expr),
    }
}

fn realize_value(val: &Value) -> String {
    match val {
        Value::Int(n) => n.to_string(),
        Value::Float(f) => if f.fract() == 0.0 { format!("{}", *f as i64) } else { f.to_string() },
        Value::Text(s) => format!("\"{}\"", s),
        Value::Path(s) | Value::Url(s) => s.clone(),
        Value::DateTime(ms) => chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*ms)
            .map(|d| d.to_rfc3339()).unwrap_or_else(|| ms.to_string()),
        _ => val.render(),
    }
}

/// Realize a var reference: look up the referent and produce its NP string.
fn realize_var_np(clause: &Clause, var: &str, refs: &[Referent]) -> String {
    let r = refs.iter()
        .chain(clause.then_referents.iter())
        .find(|r| r.var == var);
    match r {
        Some(r) => realize_ref_np(clause, r, refs),
        None => var.to_string(),
    }
}

fn realize_ref_np(clause: &Clause, r: &Referent, refs: &[Referent]) -> String {
    // Possessive: owner set
    if let Some(ref owner_var) = r.owner {
        let owner_str = realize_var_np(clause, owner_var, refs);
        let noun = r.noun.as_deref().unwrap_or("");
        let mods = r.mods.join(" ");
        let noun_str = if mods.is_empty() { noun.to_string() } else { format!("{} {}", mods, noun) };
        return format!("{}'s {}", owner_str, noun_str);
    }
    match &r.quant {
        Quant::Named(name) => name.clone(),
        Quant::Literal(val) => realize_value(val),
        Quant::Wh => {
            // In question context this is a wh-word; handled above
            r.noun.as_deref().unwrap_or("thing").to_string()
        }
        _ => {
            let det = match &r.quant {
                Quant::Indef => "a".to_string(),
                Quant::Def => "the".to_string(),
                Quant::Every => "every".to_string(),
                Quant::No => "no".to_string(),
                Quant::AtLeast(n) => format!("at least {}", n),
                Quant::AtMost(n) => format!("at most {}", n),
                Quant::Exactly(n) => format!("exactly {}", n),
                Quant::Count(n) => n.to_string(),
                _ => String::new(),
            };
            let mods = r.mods.join(" ");
            let noun = r.noun.as_deref().unwrap_or("");
            let np_body = if mods.is_empty() { noun.to_string() } else { format!("{} {}", mods, noun) };
            // Fix "a" vs "an"
            let det = fix_an(det, &np_body);
            if det.is_empty() { np_body } else { format!("{} {}", det, np_body) }
        }
    }
}

fn fix_an(det: String, noun_phrase: &str) -> String {
    if det != "a" { return det; }
    let first_char = noun_phrase.chars().next().map(|c| c.to_lowercase().next().unwrap_or(c));
    if matches!(first_char, Some('a') | Some('e') | Some('i') | Some('o') | Some('u')) {
        "an".to_string()
    } else {
        det
    }
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

fn realize_command(clause: &Clause) -> String {
    let mut parts = vec![];
    for pred in &clause.conditions {
        let do_not = if pred.negated && pred.modal.is_none() { "do not " } else { "" };
        let modal_str = realize_modal(&pred.modal, pred.negated);
        let mod_prefix = if !modal_str.is_empty() { format!("{} ", modal_str) } else { String::new() };
        let verb = &pred.pred;
        let mut obj_parts = vec![];
        for arg in pred.args.iter().skip(1) {
            obj_parts.push(realize_term(clause, arg, &clause.referents));
        }
        let obj_str = obj_parts.join(" ");
        let pp = realize_adjuncts(&pred.adjuncts, clause, &clause.referents);
        let obj_full = if obj_str.is_empty() { pp } else { format!(" {}{}", obj_str, pp) };
        parts.push(format!("{}{}{}{}", mod_prefix, do_not, verb, obj_full));
    }
    format!("Assistant, {}!", parts.join(" and "))
}

// ---------------------------------------------------------------------------
// Question
// ---------------------------------------------------------------------------

fn realize_question(clause: &Clause, kind: &QuestionKind) -> String {
    match kind {
        QuestionKind::YesNo => realize_yesno(clause),
        QuestionKind::Should => realize_modal_q(clause, "Should"),
        QuestionKind::Who { focus } => realize_who(clause, focus),
        QuestionKind::What { focus } => realize_what(clause, focus),
        QuestionKind::Which { focus } => realize_which(clause, focus),
        QuestionKind::HowMany { focus } => realize_howmany(clause, focus),
        QuestionKind::Where { focus } => realize_where(clause, focus),
        QuestionKind::When { focus } => realize_when(clause, focus),
    }
}

fn realize_yesno(clause: &Clause) -> String {
    if clause.conditions.is_empty() { return "Is it?".to_string(); }
    let pred = &clause.conditions[0];
    let subj_var = match pred.args.first() {
        Some(Term::Var { var: v }) => v.as_str(),
        _ => "",
    };
    let subj_str = realize_var_np(clause, subj_var, &clause.referents);
    if pred.pred == "be" {
        let neg = if pred.negated { " not" } else { "" };
        if let Some(ref attr) = pred.attr {
            if attr.ends_with("-than") {
                let base_adj = &attr[..attr.len() - 5];
                if let Some(cmp) = pred.args.get(1) {
                    return format!("Is {}{} {} than {}?", subj_str, neg, base_adj, realize_term(clause, cmp, &clause.referents));
                }
            }
            return format!("Is {}{}{}?", subj_str, neg, if attr.is_empty() { String::new() } else { format!(" {}", attr.replace('-', " ")) });
        }
        if let Some(obj) = pred.args.get(1) {
            return format!("Is {}{} {}?", subj_str, neg, realize_term(clause, obj, &clause.referents));
        }
        return format!("Is {}{}?", subj_str, neg);
    }
    // Does NP VP?
    let modal = &pred.modal;
    let (aux, base_verb) = if let Some(m) = modal {
        let ms = match m { Modal::Can => "Can", Modal::Should => "Should", Modal::Must => "Must", Modal::May => "May" };
        (ms.to_string(), pred.pred.clone())
    } else if pred.negated {
        ("Does".to_string(), pred.pred.clone())
    } else {
        ("Does".to_string(), pred.pred.clone())
    };
    let neg_part = if pred.negated && modal.is_none() { " not" } else { "" };
    let mut obj_parts: Vec<String> = pred.args.iter().skip(1).map(|a| realize_term(clause, a, &clause.referents)).collect();
    let obj_str = obj_parts.join(" ");
    let pp = realize_adjuncts(&pred.adjuncts, clause, &clause.referents);
    let obj_full = if obj_str.is_empty() { pp } else { format!(" {}{}", obj_str, pp) };
    format!("{} {}{}{}{}?", aux, subj_str, neg_part, format!(" {}", base_verb), obj_full)
}

fn realize_modal_q(clause: &Clause, aux: &str) -> String {
    let pred = clause.conditions.first();
    let subj_var = pred.and_then(|p| p.args.first()).and_then(|a| if let Term::Var { var: v } = a { Some(v.as_str()) } else { None }).unwrap_or("");
    let subj_str = realize_var_np(clause, subj_var, &clause.referents);
    if let Some(pred) = pred {
        let verb = &pred.pred;
        let neg = if pred.negated { " not" } else { "" };
        let mut obj_parts: Vec<String> = pred.args.iter().skip(1).map(|a| realize_term(clause, a, &clause.referents)).collect();
        let obj_str = obj_parts.join(" ");
        let pp = realize_adjuncts(&pred.adjuncts, clause, &clause.referents);
        let obj_full = if obj_str.is_empty() { pp } else { format!(" {}{}", obj_str, pp) };
        format!("{} {}{}{}{}?", aux, subj_str, neg, format!(" {}", verb), obj_full)
    } else {
        format!("{} {}?", aux, subj_str)
    }
}

fn realize_who(clause: &Clause, focus: &str) -> String {
    if clause.conditions.is_empty() { return "Who?".to_string(); }
    let pred = &clause.conditions[0];
    if pred.pred == "be" {
        let obj = pred.args.get(1).map(|a| realize_term(clause, a, &clause.referents)).unwrap_or_default();
        if obj.is_empty() { return "Who?".to_string(); }
        return format!("Who is {}?", obj);
    }
    let verb = conjugate_3sg(&pred.pred);
    let mut obj_parts: Vec<String> = pred.args.iter().skip(1).map(|a| realize_term(clause, a, &clause.referents)).collect();
    let obj_str = obj_parts.join(" ");
    let pp = realize_adjuncts(&pred.adjuncts, clause, &clause.referents);
    let obj_full = if obj_str.is_empty() { pp } else { format!(" {}{}", obj_str, pp) };
    format!("Who {}{}?", verb, obj_full)
}

fn realize_what(clause: &Clause, focus: &str) -> String {
    if clause.conditions.is_empty() { return "What?".to_string(); }
    let pred = &clause.conditions[0];
    if pred.pred == "be" {
        let np_var = if let Some(Term::Var { var: v }) = pred.args.get(1) { v.as_str() } else { "" };
        if !np_var.is_empty() {
            let np_str = realize_var_np(clause, np_var, &clause.referents);
            return format!("What is {}?", np_str);
        }
    }
    // Find subject (not the wh-var)
    let subj_var = pred.args.first().and_then(|a| if let Term::Var { var: v } = a { Some(v.as_str()) } else { None }).unwrap_or("");
    let subj_ref = clause.referents.iter().find(|r| r.var == subj_var);
    let is_wh_subj = subj_ref.map_or(false, |r| matches!(r.quant, Quant::Wh));
    if is_wh_subj {
        let verb = conjugate_3sg(&pred.pred);
        let mut obj_parts: Vec<String> = pred.args.iter().skip(1).map(|a| realize_term(clause, a, &clause.referents)).collect();
        format!("What {}{}?", verb, if obj_parts.is_empty() { String::new() } else { format!(" {}", obj_parts.join(" ")) })
    } else {
        let subj_str = realize_var_np(clause, subj_var, &clause.referents);
        let verb = &pred.pred;
        let mut obj_parts: Vec<String> = pred.args.iter().skip(1).map(|a| realize_term(clause, a, &clause.referents)).collect();
        format!("What does {} {}{}?", subj_str, verb, if obj_parts.is_empty() { String::new() } else { format!(" {}", obj_parts.join(" ")) })
    }
}

fn realize_which(clause: &Clause, focus: &str) -> String {
    let focus_ref = clause.referents.iter().find(|r| r.var == focus);
    let noun = focus_ref.and_then(|r| r.noun.as_deref()).unwrap_or("thing");
    if clause.conditions.is_empty() { return format!("Which {}?", noun); }
    let pred = &clause.conditions[0];
    let subj_var = pred.args.first().and_then(|a| if let Term::Var { var: v } = a { Some(v.as_str()) } else { None }).unwrap_or("");
    let verb_str = if let Some(ref m) = pred.modal {
        let ms = match m { Modal::Can => "can", Modal::Should => "should", Modal::Must => "must", Modal::May => "may" };
        format!("{} {}", ms, pred.pred)
    } else {
        conjugate_3sg(&pred.pred)
    };
    let mut obj_parts: Vec<String> = pred.args.iter().skip(1).map(|a| realize_term(clause, a, &clause.referents)).collect();
    let obj_str = if obj_parts.is_empty() { String::new() } else { format!(" {}", obj_parts.join(" ")) };
    format!("Which {} {}{}?", noun, verb_str, obj_str)
}

fn realize_howmany(clause: &Clause, focus: &str) -> String {
    let focus_ref = clause.referents.iter().find(|r| r.var == focus);
    let noun = focus_ref.and_then(|r| r.noun.as_deref()).unwrap_or("thing");
    if clause.conditions.is_empty() { return format!("How many {}?", noun); }
    let pred = &clause.conditions[0];
    let subj_var = pred.args.first().and_then(|a| if let Term::Var { var: v } = a { Some(v.as_str()) } else { None }).unwrap_or("");
    let focus_ref2 = clause.referents.iter().find(|r| r.var == focus);
    let is_wh_subj = focus_ref2.map_or(false, |r| matches!(r.quant, Quant::Wh));
    if is_wh_subj {
        let verb = conjugate_3sg(&pred.pred);
        let mut obj_parts: Vec<String> = pred.args.iter().skip(1).map(|a| realize_term(clause, a, &clause.referents)).collect();
        format!("How many {} {}{}?", noun, verb, if obj_parts.is_empty() { String::new() } else { format!(" {}", obj_parts.join(" ")) })
    } else {
        let subj_str = realize_var_np(clause, subj_var, &clause.referents);
        let verb = &pred.pred;
        let mut obj_parts: Vec<String> = pred.args.iter().skip(1).map(|a| realize_term(clause, a, &clause.referents)).collect();
        format!("How many {} does {} {}{}?", noun, subj_str, verb, if obj_parts.is_empty() { String::new() } else { format!(" {}", obj_parts.join(" ")) })
    }
}

fn realize_where(clause: &Clause, focus: &str) -> String {
    if clause.conditions.is_empty() { return "Where?".to_string(); }
    let pred = &clause.conditions[0];
    let subj_var = pred.args.first().and_then(|a| if let Term::Var { var: v } = a { Some(v.as_str()) } else { None }).unwrap_or("");
    let subj_str = realize_var_np(clause, subj_var, &clause.referents);
    format!("Where is {}?", subj_str)
}

fn realize_when(clause: &Clause, focus: &str) -> String {
    if clause.conditions.is_empty() { return "When?".to_string(); }
    let pred = &clause.conditions[0];
    let subj_var = pred.args.first().and_then(|a| if let Term::Var { var: v } = a { Some(v.as_str()) } else { None }).unwrap_or("");
    let subj_str = realize_var_np(clause, subj_var, &clause.referents);
    let verb = &pred.pred;
    format!("When does {} {}?", subj_str, verb)
}

// ---------------------------------------------------------------------------
// Rule
// ---------------------------------------------------------------------------

fn realize_rule(clause: &Clause) -> String {
    // Realize antecedent as a declarative-like sentence without period
    let ant_str = realize_parts_as_decl(&clause.referents, &clause.conditions, clause);
    let con_str = realize_parts_as_decl(&clause.then_referents, &clause.then, clause);
    format!("If {} then {}.", ant_str, con_str)
}

fn realize_parts_as_decl(refs: &[Referent], preds: &[Pred], clause: &Clause) -> String {
    if preds.is_empty() { return String::new(); }
    let pred = &preds[0];
    let subj_var = match pred.args.first() {
        Some(Term::Var { var: v }) => v.as_str(),
        _ => "",
    };
    // Look up subject in the given refs AND in all clause refs
    let all_refs: Vec<&Referent> = refs.iter().chain(clause.referents.iter()).chain(clause.then_referents.iter()).collect();
    let subj_r = all_refs.iter().find(|r| r.var == subj_var);
    let subj_str = subj_r.map(|r| {
        // We build refs from all combined
        let combined: Vec<Referent> = refs.iter().chain(clause.referents.iter()).chain(clause.then_referents.iter()).cloned().collect();
        realize_ref_np(clause, r, &combined)
    }).unwrap_or_else(|| subj_var.to_string());

    let combined: Vec<Referent> = refs.iter().chain(clause.referents.iter()).chain(clause.then_referents.iter()).cloned().collect();
    let mut vp_parts = vec![];
    for p in preds {
        vp_parts.push(realize_pred(clause, p, &combined, false));
    }
    format!("{} {}", subj_str, vp_parts.join(" and "))
}
