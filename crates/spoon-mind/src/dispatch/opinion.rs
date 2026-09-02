//! Opinion and advice retrieval from stances.
//! No LLM involved - queries the store.

use spoon_core::types::{ActionId, AdviceOption, ConceptId, Move, Pred, Quant, QuestionKind, Term, Value};
use spoon_lang::sce::singularize_noun;

use crate::discourse::{concept_name, realize_fact, Binding, DiscourseState, Grounded};

use super::{DispatchCtx, Dispatched, TeacherAsk};

const OPINION_VERBS: &[&str] = &["think", "believe", "prefer", "like", "love", "hate", "recommend", "opine"];
const COMPARISON_ATTRS: &[&str] = &[
    "better", "worse", "best", "worst", "hard", "easy", "ok", "good", "bad",
    "important", "dangerous", "toxic", "overrated", "underrated",
];

/// True if this question is an opinion question addressed to Assistant.
pub fn is_opinion_question(g: &Grounded) -> bool {
    let Some(pred) = g.clause.conditions.first() else { return false; };
    let subject_is_assistant = pred.args.first().and_then(|t| {
        if let Term::Var { var } = t { g.clause.referent(var) } else { None }
    }).map(|r| matches!(&r.quant, Quant::Named(n) if n == "Assistant"))
        .unwrap_or(false);

    if subject_is_assistant && OPINION_VERBS.contains(&pred.pred.as_str()) {
        return true;
    }
    // Comparison: "Is X better than Y?" -> subject might not be Assistant but no explicit subj.
    if pred.pred == "be" {
        if let Some(attr) = &pred.attr {
            if COMPARISON_ATTRS.iter().any(|a| attr.contains(a)) {
                return true;
            }
        }
    }
    false
}

/// True if this is an advice question.
pub fn is_advice_question(g: &Grounded, kind: &QuestionKind) -> bool {
    let is_advice_kind = matches!(kind, QuestionKind::Should | QuestionKind::What { .. } | QuestionKind::Who { .. });
    is_advice_kind && advice_signals(g)
}

fn advice_signals(g: &Grounded) -> bool {
    let Some(pred) = g.clause.conditions.first() else { return false; };
    // "What should User do?" -> subject User, kind Should or What + "should"
    let subj_is_user = pred.args.first().and_then(|t| {
        if let Term::Var { var } = t { g.clause.referent(var) } else { None }
    }).map(|r| matches!(&r.quant, Quant::Named(n) if n == "User"))
        .unwrap_or(false);
    let is_user_question = subj_is_user || g.clause.referents.iter().any(|r| matches!(&r.quant, Quant::Named(n) if n == "User"));
    let advice_preds = ["recommend", "advise", "help", "decide"];
    let has_advice_pred = advice_preds.contains(&pred.pred.as_str())
        || pred.modal.is_some();
    is_user_question && has_advice_pred
}

/// Handle an opinion question (What does Assistant think about X?).
pub fn handle_opinion(
    ctx: &mut DispatchCtx<'_>,
    g: &Grounded,
    _state: &DiscourseState,
) -> anyhow::Result<Dispatched> {
    let raw_keywords = extract_topic_keywords(g);
    // Also try space-normalized form (hyphens -> spaces) so "pineapple-pizza" matches "pineapple pizza".
    let mut keywords: Vec<String> = raw_keywords.clone();
    for kw in &raw_keywords {
        let normalized = kw.replace('-', " ");
        if normalized != *kw { keywords.push(normalized); }
    }
    let stances = ctx.store.stances(&keywords).unwrap_or_default();

    if let Some(stance) = stances.first() {
        return Ok(Dispatched::Moves(vec![Move::Opinion {
            topic: stance.topic.clone(),
            stance: stance.stance.clone(),
            reasons: stance.reasons.clone(),
            confidence: stance.confidence,
        }]));
    }

    let topic = keywords.join(" ").trim().to_string();
    let topic_display = if topic.is_empty() { "that".to_string() } else { surface_topic(&g.clause.sce, &raw_keywords) };
    let known = facts_mentioning(ctx, &raw_keywords, 2);
    let text = if known.is_empty() {
        format!("i don't have a view on {topic_display} yet. tell me yours and i'll think about it")
    } else {
        format!("i don't have a view on {topic_display} yet, but i know {}. what do you think?", known.join(" and "))
    };
    Ok(Dispatched::NeedsTeacher {
        ask: TeacherAsk::Stance { topic: topic.clone() },
        fallback: vec![Move::Explain { text }],
    })
}

/// "What does User think about dogs?": a view someone other than Assistant
/// stated earlier ("User thinks that dogs are great."), read back from the
/// `rel.think` (believe, ...) facts about them. `None` when the question is
/// not of this shape.
pub fn handle_reported_view(ctx: &mut DispatchCtx<'_>, g: &Grounded, kind: &QuestionKind) -> Option<Dispatched> {
    if !matches!(kind, QuestionKind::What { .. } | QuestionKind::Who { .. } | QuestionKind::Which { .. }) {
        return None;
    }
    let pred = g.clause.conditions.first()?;
    if !OPINION_VERBS.contains(&pred.pred.as_str()) {
        return None;
    }
    let Term::Var { var } = pred.args.first()? else { return None };
    let subject_name = match &g.clause.referent(var)?.quant {
        Quant::Named(n) if n != "Assistant" => n.clone(),
        _ => return None,
    };
    let subject = match g.bindings.get(var.as_str())? {
        Binding::Entity(v) | Binding::Literal(v) => v.clone(),
        _ => return None,
    };

    let keywords = about_keywords(g, pred);
    let rel = ActionId(format!("rel.{}", pred.pred));
    let views: Vec<Value> = ctx
        .store
        .query_facts(&rel, &[Some(subject), None])
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.truth)
        .filter_map(|f| f.args.get(1).cloned())
        .filter(|v| keywords.is_empty() || v.as_str().is_some_and(|s| mentions_any(s, &keywords)))
        .collect();

    let topic = surface_topic(&g.clause.sce, &keywords);
    Some(Dispatched::Moves(vec![if views.is_empty() {
        let who = if subject_name == "User" { "you think".to_string() } else { format!("{subject_name} thinks") };
        let about = if topic.is_empty() { String::new() } else { format!(" about {topic}") };
        Move::Explain { text: format!("i don't know what {who}{about} yet") }
    } else {
        Move::Answer { question: g.clause.sce.clone(), values: views, source: Some("memory".into()) }
    }]))
}

/// What a view is about: the `about` adjunct (or the object), not the thinker.
fn about_keywords(g: &Grounded, pred: &Pred) -> Vec<String> {
    let mut out = vec![];
    let about = pred.adjuncts.iter().filter(|(p, _)| p == "about").map(|(_, t)| t);
    for t in about.chain(pred.args.iter().skip(1)) {
        let Term::Var { var } = t else { continue };
        let Some(r) = g.clause.referent(var) else { continue };
        if let Some(n) = &r.noun {
            out.push(n.clone());
        }
        if let Quant::Named(n) = &r.quant {
            out.push(n.to_lowercase());
        }
    }
    out
}

/// The topic as the user said it: keyword lemmas ("dog") mapped back to the
/// words of the sentence ("dogs"), hyphens opened ("pineapple pizza").
fn surface_topic(sce: &str, keywords: &[String]) -> String {
    let words: Vec<String> = sce
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    keywords
        .iter()
        .map(|kw| {
            words
                .iter()
                .find(|w| *w == kw || singularize_noun(w) == *kw)
                .unwrap_or(kw)
                .replace('-', " ")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Does `text` contain any keyword, in singular or plural form?
fn mentions_any(text: &str, keywords: &[String]) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .any(|w| keywords.iter().any(|kw| w == *kw || singularize_noun(&w) == *kw))
}

/// Up to `limit` stored facts about the topic, as English: the topic as a
/// proper name (John) and every entity of the topic's concept (dog_1, ...).
fn facts_mentioning(ctx: &DispatchCtx<'_>, keywords: &[String], limit: usize) -> Vec<String> {
    let is_a = ActionId("rel.is_a".into());
    let mut entities: Vec<Value> = Vec::new();
    for kw in keywords {
        entities.push(Value::Name(concept_name(kw)));
        let mut concepts: Vec<ConceptId> = ctx.can.concepts_for_noun(kw).iter().map(|c| c.id.clone()).collect();
        if concepts.is_empty() {
            concepts.push(ConceptId(concept_name(kw)));
        }
        for c in concepts {
            for f in ctx.store.query_facts(&is_a, &[None, Some(Value::Name(c.0))]).unwrap_or_default() {
                if let (true, Some(e)) = (f.truth, f.args.first()) {
                    entities.push(e.clone());
                }
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    for e in &entities {
        let facts = ctx.store.facts_about(e).unwrap_or_default();
        for s in facts.iter().filter_map(|f| realize_fact(ctx.can, ctx.store, f)) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out.truncate(limit);
    out
}

/// Handle an advice question (What should User do?).
pub fn handle_advice(
    ctx: &mut DispatchCtx<'_>,
    g: &Grounded,
    state: &DiscourseState,
) -> anyhow::Result<Dispatched> {
    let sce = g.clause.sce.clone();

    // Gather situation from last_clauses (User asserts in this and previous turn).
    let situation = state.last_clauses.iter()
        .filter(|c| {
            c.referents.iter().any(|r| matches!(&r.quant, Quant::Named(n) if n == "User"))
        })
        .map(|c| c.sce.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    let keywords = extract_topic_keywords(g);
    let advice_keywords: Vec<String> = keywords.iter()
        .map(|k| format!("advice:{}", k))
        .chain(keywords.clone().into_iter())
        .collect();
    let stances = ctx.store.stances(&advice_keywords).unwrap_or_default();

    if let Some(stance) = stances.first() {
        let options = stance.reasons.iter().map(|r| AdviceOption {
            option: r.clone(),
            pros: vec![],
            cons: vec![],
        }).collect();
        return Ok(Dispatched::Moves(vec![Move::Advise {
            situation: if situation.is_empty() { sce.clone() } else { situation },
            options,
            leaning: Some(stance.stance.clone()),
        }]));
    }

    // Fallback: check for feeling context in last_clauses.
    let has_feeling = state.last_clauses.iter().any(|c| {
        c.conditions.iter().any(|p| {
            p.pred == "be" && p.attr.as_deref().map(is_feeling_attr).unwrap_or(false)
                || matches!(p.pred.as_str(), "stress" | "worry" | "fight" | "struggle")
        })
    });

    let situation_display = if situation.is_empty() { sce.clone() } else { situation.clone() };
    let mut fallback = vec![Move::Reflect { summary: situation_display }];
    if has_feeling {
        fallback.insert(0, Move::Empathize { feeling: "stressed".to_string(), about: "this situation".to_string() });
    }
    fallback.push(Move::Clarify {
        question: "what have you tried so far?".into(),
        options: vec![],
        slot_type: None,
    });

    Ok(Dispatched::NeedsTeacher {
        ask: TeacherAsk::Advice { situation: situation.clone(), keywords },
        fallback,
    })
}

fn is_feeling_attr(attr: &str) -> bool {
    let feelings = ["tired", "stressed", "angry", "sad", "overwhelmed", "bored", "lonely", "upset"];
    feelings.iter().any(|f| attr.contains(f))
}

/// Extract topic keywords from the question referents and pred.
pub fn extract_topic_keywords(g: &Grounded) -> Vec<String> {
    let mut keywords = vec![];
    for r in &g.clause.referents {
        if let Some(noun) = &r.noun {
            if !["assistant", "user", "statement", "question"].contains(&noun.as_str()) {
                keywords.push(noun.clone());
            }
        }
        for m in &r.mods {
            keywords.push(m.clone());
        }
        if let Quant::Named(n) = &r.quant {
            if !["Assistant", "User"].contains(&n.as_str()) {
                keywords.push(n.to_lowercase());
            }
        }
    }
    // Also add adjunct objects.
    if let Some(pred) = g.clause.conditions.first() {
        if let Some(attr) = &pred.attr {
            if !COMPARISON_ATTRS.contains(&attr.as_str()) {
                keywords.push(attr.clone());
            }
        }
        for (prep, term) in &pred.adjuncts {
            if prep == "about" {
                if let Term::Var { var } = term {
                    if let Some(r) = g.clause.referent(var) {
                        if let Some(noun) = &r.noun {
                            keywords.push(noun.clone());
                        }
                        if let Quant::Named(n) = &r.quant {
                            keywords.push(n.to_lowercase());
                        }
                    }
                }
            }
        }
    }
    keywords.sort();
    keywords.dedup();
    keywords
}
