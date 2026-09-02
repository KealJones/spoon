//! Facts layer: assert grounded clauses as facts, answer questions over them.
//!
//! Assertion rules:
//!   - For each Pred, find or create a Relation action (id = "rel.{verb}").
//!   - "be" copula is handled specially: attr -> rel.is, NP -> rel.is_a.
//!   - New entities with nouns get an is_a fact linking them to their concept.
//!   - Count entities get a rel.count fact.
//!   - Universal bindings delegate to rules.rs; the clause returns Universal.
//!   - Contradictions (same pred+args, opposite truth) are surfaced to caller.
//!
//! Answer rules:
//!   - YesNo: query by pred+bound args; check truth; fallback to CAN hierarchy.
//!   - Who/What/Which/Where/When: collect values at the Query position.
//!   - HowMany: sum rel.count facts for entities matching the query.

use std::collections::HashMap;

use anyhow::Context;

use spoon_core::can::Can;
use spoon_core::store::Store;
use spoon_core::types::*;

use super::rules::{
    capitalize_first, ensure_concept_in_can, forward_chain, universal_to_rule,
};
use super::{Binding, Grounded};

// ---------- public types ----------

pub struct FactWriter<'a> {
    pub can: &'a mut Can,
    pub store: &'a Store,
}

#[derive(Debug)]
pub enum AssertOutcome {
    Stored {
        facts: Vec<Fact>,
        new_relations: Vec<ActionId>,
        new_concepts: Vec<ConceptId>,
    },
    Contradiction {
        existing: Fact,
        incoming: Fact,
    },
    Universal {
        rule_id: String,
    },
}

#[derive(Debug)]
pub enum Answer {
    Values(Vec<Value>),
    YesNo(bool, Option<Fact>),
    Count(i64),
    Unknown { reason: String },
}

// ---------- helpers for assertion ----------

/// Ensure a Relation action exists for `rel_id` (e.g. "rel.own"). Creates a
/// provisional one with `Impl::Primitive` if absent.
fn ensure_relation(
    w: &mut FactWriter<'_>,
    rel_id: &str,
    verbs: &[&str],
    arity: usize,
    sce: &str,
) -> ActionId {
    let action_id = ActionId(rel_id.to_string());
    if w.can.action(&action_id).is_some() {
        return action_id;
    }
    let inputs: Vec<Input> = (0..arity)
        .map(|i| Input::required(&format!("arg{}", i), Type::Any))
        .collect();
    let action = Action {
        id: action_id.clone(),
        inputs,
        output: Type::Bool,
        effect: Effect::Pure,
        imp: Impl::Primitive,
        role: Role::Relation,
        verbs: verbs.iter().map(|s| s.to_string()).collect(),
        phrasings: vec![],
        description: format!("Relation: {}", rel_id),
        tier: Tier::Provisional,
        provenance: Provenance::User {
            utterance: sce.to_string(),
        },
        stats: Stats::default(),
    };
    w.can.add_action(action.clone());
    let _ = w.store.save_action(&action);
    action_id
}

/// Find or create a Relation action for a plain verb.
fn ensure_verb_relation(w: &mut FactWriter<'_>, verb: &str, arity: usize, sce: &str) -> ActionId {
    // Check if a Relation action already exists for this verb.
    let existing = w.can.actions_for_verb(verb);
    if let Some(a) = existing.iter().find(|a| a.role == Role::Relation) {
        return a.id.clone();
    }
    ensure_relation(w, &format!("rel.{}", verb), &[verb], arity, sce)
}

/// Resolve a Term to a Value using the binding map.
fn resolve_term(term: &Term, bindings: &HashMap<String, Binding>) -> Option<Value> {
    match term {
        Term::Var { var } => match bindings.get(var.as_str()) {
            Some(Binding::Entity(v)) => Some(v.clone()),
            Some(Binding::Literal(v)) => Some(v.clone()),
            _ => None,
        },
        Term::Value { value } => Some(value.clone()),
        Term::Sub { clause } => Some(Value::Text(clause.sce.clone())),
        Term::Arith { .. } => None,
    }
}

/// For the `be` copula NP argument: if the arg's referent is Indef (typing),
/// return the concept name value instead of the minted entity name.
fn resolve_be_np_arg(
    term: &Term,
    bindings: &HashMap<String, Binding>,
    clause_refs: &[Referent],
    can: &Can,
) -> Option<Value> {
    if let Term::Var { var } = term {
        let referent = clause_refs.iter().find(|r| &r.var == var);
        if let Some(r) = referent {
            if matches!(r.quant, Quant::Indef | Quant::Count(_)) {
                if let Some(noun) = &r.noun {
                    let concept = can
                        .concepts_for_noun(noun)
                        .into_iter()
                        .next()
                        .map(|c| c.id.0.clone())
                        .unwrap_or_else(|| capitalize_first(noun));
                    return Some(Value::Name(concept));
                }
            }
        }
    }
    resolve_term(term, bindings)
}

/// Build a Fact (id=0 placeholder) from a Pred and resolved args.
fn make_fact(
    pred_id: ActionId,
    args: Vec<Value>,
    negated: bool,
    modal: &Option<Modal>,
    source: &str,
    episode_id: Option<i64>,
) -> Fact {
    Fact {
        id: 0,
        pred: pred_id,
        args,
        truth: !negated,
        modal: modal.as_ref().map(|m| match m {
            Modal::Must => "must".to_string(),
            Modal::Should => "should".to_string(),
            Modal::May => "may".to_string(),
            Modal::Can => "can".to_string(),
        }),
        asserted_at: now_ms(),
        invalidated_at: None,
        source: source.to_string(),
        episode_id,
    }
}

// ---------- assert_grounded ----------

pub fn assert_grounded(
    w: &mut FactWriter<'_>,
    g: &Grounded,
    source: &str,
    episode_id: Option<i64>,
) -> anyhow::Result<AssertOutcome> {
    let sce = &g.clause.sce;

    // Universal quantifiers -> delegate to rules.
    let has_universal = g.bindings.values().any(|b| matches!(b, Binding::Universal { .. }));
    if has_universal {
        let rule = universal_to_rule(w.can, w.store, &g.clause)?;
        // Eagerly forward-chain so existing facts benefit immediately.
        forward_chain(w.can, w.store, &[], 1000);
        return Ok(AssertOutcome::Universal { rule_id: rule.id });
    }

    // Act::Rule clauses (explicit if-then) also become stored rules.
    if matches!(g.clause.act, Act::Rule) {
        use crate::discourse::rules::add_rule;
        let rule = add_rule(w.store, &g.clause)?;
        forward_chain(w.can, w.store, &[], 1000);
        return Ok(AssertOutcome::Universal { rule_id: rule.id });
    }

    let mut stored_facts: Vec<Fact> = Vec::new();
    let mut new_relations: Vec<ActionId> = Vec::new();
    let mut new_concepts: Vec<ConceptId> = Vec::new();

    // ---- Process new entities: ensure concept + is_a fact + count fact ----
    for entity in &g.new_entities {
        if let Value::Name(ent_name) = &entity.id {
            if let Some(noun) = &entity.noun {
                let concept_id = ensure_concept_in_can(w.can, w.store, noun);
                let is_a_id = ensure_relation(w, "rel.is_a", &["is_a"], 2, sce);
                let is_a_fact = make_fact(
                    is_a_id,
                    vec![Value::Name(ent_name.clone()), Value::Name(concept_id.0.clone())],
                    false,
                    &None,
                    source,
                    episode_id,
                );
                let inserted_id = w.store.insert_fact(&is_a_fact)?;
                stored_facts.push(Fact { id: inserted_id, ..is_a_fact });
                new_concepts.push(concept_id);
            }

            // count mod -> rel.count fact.
            for m in &entity.mods {
                if let Some(count_str) = m.strip_prefix("count=") {
                    if let Ok(n) = count_str.parse::<i64>() {
                        let count_id = ensure_relation(w, "rel.count", &["count"], 2, sce);
                        let count_fact = make_fact(
                            count_id,
                            vec![Value::Name(ent_name.clone()), Value::Int(n)],
                            false,
                            &None,
                            source,
                            episode_id,
                        );
                        let inserted_id = w.store.insert_fact(&count_fact)?;
                        stored_facts.push(Fact { id: inserted_id, ..count_fact });
                    }
                }
            }
        }
    }

    // ---- Process each predicate in the clause ----
    let all_refs: Vec<&Referent> = g
        .clause
        .referents
        .iter()
        .chain(g.clause.then_referents.iter())
        .collect();

    for pred in &g.clause.conditions {
        let args_and_id = if pred.pred == "be" {
            process_be_pred(w, pred, &g.bindings, &all_refs, source, episode_id)
        } else {
            process_regular_pred(w, pred, &g.bindings, sce, source, episode_id)
        };

        let Some((action_id, fact)) = args_and_id else {
            continue;
        };

        // Check for contradiction.
        let pattern: Vec<Option<Value>> = fact.args.iter().map(|a| Some(a.clone())).collect();
        let existing = w
            .store
            .query_facts(&action_id, &pattern)
            .unwrap_or_default();
        for ex in &existing {
            if ex.truth != fact.truth {
                return Ok(AssertOutcome::Contradiction {
                    existing: ex.clone(),
                    incoming: fact,
                });
            }
        }

        // Insert fact.
        let id = w.store.insert_fact(&fact).context("insert fact")?;
        let was_new = !existing
            .iter()
            .any(|e| e.truth == fact.truth && e.pred == fact.pred && e.args == fact.args);
        if was_new {
            new_relations.push(action_id);
            stored_facts.push(Fact { id, ..fact });
        }
    }

    Ok(AssertOutcome::Stored {
        facts: stored_facts,
        new_relations,
        new_concepts,
    })
}

fn process_be_pred(
    w: &mut FactWriter<'_>,
    pred: &Pred,
    bindings: &HashMap<String, Binding>,
    all_refs: &[&Referent],
    source: &str,
    episode_id: Option<i64>,
) -> Option<(ActionId, Fact)> {
    let sce = "";
    if let Some(attr) = &pred.attr {
        // Attribute form: be(x1) attr="adj" -> rel.is(x1, Text(adj))
        let x1 = resolve_term(pred.args.first()?, bindings)?;
        let action_id = ensure_relation(w, "rel.is", &["is"], 2, sce);
        let fact = make_fact(
            action_id.clone(),
            vec![x1, Value::Text(attr.clone())],
            pred.negated,
            &pred.modal,
            source,
            episode_id,
        );
        Some((action_id, fact))
    } else if pred.args.len() >= 2 {
        // NP form: be(x1, x2) -> rel.is_a(x1, ConceptId)
        let x1 = resolve_term(&pred.args[0], bindings)?;
        let x2_val = resolve_be_np_arg(
            &pred.args[1],
            bindings,
            &all_refs.iter().map(|r| (*r).clone()).collect::<Vec<_>>(),
            w.can,
        )?;
        let action_id = ensure_relation(w, "rel.is_a", &["is_a"], 2, sce);
        let fact = make_fact(
            action_id.clone(),
            vec![x1, x2_val],
            pred.negated,
            &pred.modal,
            source,
            episode_id,
        );
        Some((action_id, fact))
    } else {
        None
    }
}

fn process_regular_pred(
    w: &mut FactWriter<'_>,
    pred: &Pred,
    bindings: &HashMap<String, Binding>,
    sce: &str,
    source: &str,
    episode_id: Option<i64>,
) -> Option<(ActionId, Fact)> {
    let mut args: Vec<Value> = Vec::new();
    for term in &pred.args {
        let v = resolve_term(term, bindings)?;
        args.push(v);
    }
    // Adjuncts: add as extra args.
    for (_prep, term) in &pred.adjuncts {
        if let Some(v) = resolve_term(term, bindings) {
            args.push(v);
        }
    }
    // Attribute.
    if let Some(attr) = &pred.attr {
        args.push(Value::Text(attr.clone()));
    }

    let action_id = ensure_verb_relation(w, &pred.pred, args.len(), sce);
    let fact = make_fact(
        action_id.clone(),
        args,
        pred.negated,
        &pred.modal,
        source,
        episode_id,
    );
    Some((action_id, fact))
}

// ---------- supersede ----------

/// Invalidate an existing fact and insert the incoming replacement.
pub fn supersede(store: &Store, existing_id: i64, incoming: &Fact) -> anyhow::Result<i64> {
    store.invalidate_fact(existing_id, now_ms())?;
    store.insert_fact(incoming)
}

// ---------- answer_grounded ----------

pub fn answer_grounded(
    can: &Can,
    store: &Store,
    g: &Grounded,
    kind: &QuestionKind,
) -> anyhow::Result<Answer> {
    let all_refs: Vec<&Referent> = g
        .clause
        .referents
        .iter()
        .chain(g.clause.then_referents.iter())
        .collect();

    // Find the first meaningful predicate.
    let pred = match g.clause.conditions.first() {
        Some(p) => p,
        None => return Ok(Answer::Unknown { reason: "no predicate".into() }),
    };

    match kind {
        QuestionKind::YesNo | QuestionKind::Should => {
            answer_yes_no(can, store, g, pred, &all_refs)
        }
        QuestionKind::Who { focus }
        | QuestionKind::What { focus }
        | QuestionKind::Which { focus }
        | QuestionKind::Where { focus }
        | QuestionKind::When { focus } => {
            answer_wh(can, store, g, pred, &all_refs, focus, None)
        }
        QuestionKind::HowMany { focus } => {
            answer_how_many(store, g, pred, focus)
        }
    }
}

fn answer_yes_no(
    can: &Can,
    store: &Store,
    g: &Grounded,
    pred: &Pred,
    _all_refs: &[&Referent],
) -> anyhow::Result<Answer> {
    if pred.pred == "be" {
        return answer_be_yesno(can, store, g, pred, &[]);
    }

    let action_id = action_id_for(can, &pred.pred);
    let Some(action_id) = action_id else {
        return Ok(Answer::Unknown { reason: format!("unknown predicate '{}'", pred.pred) });
    };

    let pattern = build_pattern(pred, &g.bindings);
    let facts = store.query_facts(&action_id, &pattern).unwrap_or_default();

    if let Some(f) = facts.iter().find(|f| f.truth) {
        return Ok(Answer::YesNo(true, Some(f.clone())));
    }
    if let Some(f) = facts.iter().find(|f| !f.truth) {
        return Ok(Answer::YesNo(false, Some(f.clone())));
    }

    // No direct fact: check for Should-type questions.
    if matches!(g.clause.act, Act::Question { kind: QuestionKind::Should }) {
        return Ok(Answer::Unknown { reason: "suggestion".into() });
    }

    Ok(Answer::Unknown { reason: "no fact".into() })
}

fn answer_be_yesno(
    can: &Can,
    store: &Store,
    g: &Grounded,
    pred: &Pred,
    all_refs: &[&Referent],
) -> anyhow::Result<Answer> {
    // Attribute form: "Is X brown?" -> query rel.is(x1, Text("brown"))
    if let Some(attr) = &pred.attr {
        let Some(x1) = pred.args.first().and_then(|t| resolve_term(t, &g.bindings)) else {
            return Ok(Answer::Unknown { reason: "unresolved subject".into() });
        };
        let action_id = ActionId("rel.is".into());
        let pattern = vec![Some(x1), Some(Value::Text(attr.clone()))];
        let facts = store.query_facts(&action_id, &pattern).unwrap_or_default();
        if let Some(f) = facts.iter().find(|f| f.truth) {
            return Ok(Answer::YesNo(true, Some(f.clone())));
        }
        if let Some(f) = facts.iter().find(|f| !f.truth) {
            return Ok(Answer::YesNo(false, Some(f.clone())));
        }
        return Ok(Answer::Unknown { reason: "no attribute fact".into() });
    }

    // NP form: "Is X a dog?" -> check rel.is_a
    if pred.args.len() >= 2 {
        let Some(x1) = resolve_term(&pred.args[0], &g.bindings) else {
            return Ok(Answer::Unknown { reason: "unresolved subject".into() });
        };

        // Determine the target concept.
        let target_concept = if let Term::Var { var: x2_var } = &pred.args[1] {
            let ref2 = all_refs.iter().find(|r| &r.var == x2_var);
            ref2.and_then(|r| r.noun.as_deref()).and_then(|noun| {
                can.concepts_for_noun(noun)
                    .into_iter()
                    .next()
                    .map(|c| c.id.clone())
                    .or_else(|| Some(ConceptId(capitalize_first(noun))))
            })
        } else if let Term::Value { value: Value::Name(n) } = &pred.args[1] {
            Some(ConceptId(n.clone()))
        } else {
            None
        };

        let action_id = ActionId("rel.is_a".into());

        if let Some(tc) = &target_concept {
            // Direct fact lookup.
            let pattern = vec![Some(x1.clone()), Some(Value::Name(tc.0.clone()))];
            let facts = store.query_facts(&action_id, &pattern).unwrap_or_default();
            if let Some(f) = facts.iter().find(|f| f.truth) {
                return Ok(Answer::YesNo(true, Some(f.clone())));
            }
            if let Some(f) = facts.iter().find(|f| !f.truth) {
                return Ok(Answer::YesNo(false, Some(f.clone())));
            }

            // CAN hierarchy fallback: query all is_a facts for x1, then check closure.
            let direct_pattern = vec![Some(x1.clone()), None];
            let direct_facts = store.query_facts(&action_id, &direct_pattern).unwrap_or_default();
            for f in direct_facts.iter().filter(|f| f.truth) {
                if let Some(Value::Name(concept_name)) = f.args.get(1) {
                    let c = ConceptId(concept_name.clone());
                    if can.is_a(&c, tc) {
                        return Ok(Answer::YesNo(true, Some(f.clone())));
                    }
                }
            }
        } else {
            // Unknown concept: just query rel.is_a(x1, None) and return what's there.
            let pattern = vec![Some(x1.clone()), None];
            let facts = store.query_facts(&action_id, &pattern).unwrap_or_default();
            if !facts.is_empty() {
                return Ok(Answer::YesNo(true, Some(facts[0].clone())));
            }
        }
    }

    Ok(Answer::Unknown { reason: "no is_a fact".into() })
}

fn answer_wh(
    can: &Can,
    store: &Store,
    g: &Grounded,
    pred: &Pred,
    _all_refs: &[&Referent],
    focus: &str,
    noun_filter: Option<&str>,
) -> anyhow::Result<Answer> {
    if pred.pred == "be" {
        // be with NP -> we want who/what is x
        // For now fall through to generic handling.
    }

    let action_id = action_id_for(can, &pred.pred)
        .or_else(|| Some(ActionId(format!("rel.{}", pred.pred))));
    let Some(action_id) = action_id else {
        return Ok(Answer::Unknown { reason: format!("unknown predicate '{}'", pred.pred) });
    };

    let pattern = build_pattern(pred, &g.bindings);
    let facts = store.query_facts(&action_id, &pattern).unwrap_or_default();

    // Find which arg position corresponds to the focus variable.
    let focus_pos = pred.args.iter().position(|t| {
        if let Term::Var { var } = t {
            var == focus
                && matches!(
                    g.bindings.get(var.as_str()),
                    Some(Binding::Query) | Some(Binding::Unbound)
                )
        } else {
            false
        }
    });

    let pos = focus_pos.unwrap_or(0);
    let mut values: Vec<Value> = facts
        .iter()
        .filter(|f| f.truth)
        .filter_map(|f| f.args.get(pos).cloned())
        .collect();

    // Filter by noun concept for Which questions.
    if let Some(noun) = noun_filter {
        let target_concepts: Vec<ConceptId> = can
            .concepts_for_noun(noun)
            .iter()
            .map(|c| c.id.clone())
            .collect();
        if !target_concepts.is_empty() {
            let is_a_id = ActionId("rel.is_a".into());
            values.retain(|v| {
                let p = vec![Some(v.clone()), None];
                store.query_facts(&is_a_id, &p)
                    .unwrap_or_default()
                    .iter()
                    .filter(|f| f.truth)
                    .any(|f| {
                        f.args.get(1).and_then(|a| {
                            if let Value::Name(n) = a {
                                Some(ConceptId(n.clone()))
                            } else {
                                None
                            }
                        })
                        .map(|c| target_concepts.iter().any(|tc| can.is_a(&c, tc) || &c == tc))
                        .unwrap_or(false)
                    })
            });
        }
    }

    values.sort_by(|a, b| a.render().cmp(&b.render()));
    values.dedup_by(|a, b| a == b);

    if values.is_empty() {
        Ok(Answer::Unknown { reason: "no matching fact".into() })
    } else {
        Ok(Answer::Values(values))
    }
}

fn answer_how_many(
    store: &Store,
    g: &Grounded,
    pred: &Pred,
    focus: &str,
) -> anyhow::Result<Answer> {
    // Find the focus var position in the pred.
    let focus_pos = pred.args.iter().position(|t| {
        if let Term::Var { var } = t {
            var == focus
        } else {
            false
        }
    });

    // Build query pattern: focus position = None, rest = bound.
    let pattern = build_pattern(pred, &g.bindings);

    let action_id = ActionId(format!("rel.{}", pred.pred));

    let facts = store.query_facts(&action_id, &pattern).unwrap_or_default();

    let pos = focus_pos.unwrap_or(pred.args.len().saturating_sub(1));
    let count_id = ActionId("rel.count".into());
    let mut total: i64 = 0;
    let mut found_count = false;

    for fact in facts.iter().filter(|f| f.truth) {
        if let Some(entity_val) = fact.args.get(pos) {
            let cp = vec![Some(entity_val.clone()), None];
            let count_facts = store.query_facts(&count_id, &cp).unwrap_or_default();
            for cf in count_facts.iter().filter(|f| f.truth) {
                if let Some(Value::Int(n)) = cf.args.get(1) {
                    total += n;
                    found_count = true;
                }
            }
            if !found_count {
                total += 1; // Count the entity itself if no count fact.
            }
        }
    }

    if facts.iter().any(|f| f.truth) {
        Ok(Answer::Count(total))
    } else {
        Ok(Answer::Unknown { reason: "no matching facts".into() })
    }
}

// ---------- describe ----------

/// Describe an entity as a list of SCE-style strings, e.g. "John owns dog_1".
pub fn describe(can: &Can, store: &Store, entity: &Value) -> Vec<String> {
    let facts = store.facts_about(entity).unwrap_or_default();
    facts
        .iter()
        .map(|f| {
            let pred_name = can
                .action(&f.pred)
                .and_then(|a| a.verbs.first().cloned())
                .unwrap_or_else(|| f.pred.0.clone());
            let args_str: Vec<String> = f.args.iter().map(|v| v.render()).collect();
            let truth_pfx = if f.truth { "" } else { "not " };
            format!("{}{}({})", truth_pfx, pred_name, args_str.join(", "))
        })
        .collect()
}

// ---------- utility ----------

/// Find the ActionId for a verb by checking Relation actions in CAN.
fn action_id_for(can: &Can, verb: &str) -> Option<ActionId> {
    let direct = ActionId(format!("rel.{}", verb));
    if can.action(&direct).is_some() {
        return Some(direct);
    }
    can.actions_for_verb(verb)
        .iter()
        .find(|a| a.role == Role::Relation)
        .map(|a| a.id.clone())
}

/// Build a query pattern from a Pred and bindings: Entity/Literal -> Some(v),
/// Query/Unbound/Universal -> None.
fn build_pattern(pred: &Pred, bindings: &HashMap<String, Binding>) -> Vec<Option<Value>> {
    let mut pattern: Vec<Option<Value>> = pred
        .args
        .iter()
        .map(|t| match t {
            Term::Var { var } => match bindings.get(var.as_str()) {
                Some(Binding::Entity(v)) => Some(v.clone()),
                Some(Binding::Literal(v)) => Some(v.clone()),
                _ => None,
            },
            Term::Value { value } => Some(value.clone()),
            _ => None,
        })
        .collect();

    if let Some(attr) = &pred.attr {
        pattern.push(Some(Value::Text(attr.clone())));
    }
    pattern
}
