//! Facts layer, assertion side: store grounded clauses as facts. Questions
//! over them are answered in `answer.rs`.
//!
//! Assertion rules:
//!   - For each Pred, find or create a Relation action (id = "rel.{verb}").
//!   - "be" copula is handled specially: attr -> rel.is, NP -> rel.is_a,
//!     `is ADJ than` -> rel.ADJ-than(x, y) (see `compare.rs`).
//!   - New entities with nouns get an is_a fact linking them to their concept.
//!   - Count entities get a rel.count fact.
//!   - Universal bindings delegate to rules.rs; the clause returns Universal.
//!   - Contradictions (same pred+args, opposite truth) are surfaced to caller.
//!   - Event verbs (moves to, picks-up, drops, gives) also update the world
//!     state (see `state.rs`).

use std::collections::HashMap;

use anyhow::Context;

use spoon_core::can::Can;
use spoon_core::store::Store;
use spoon_core::types::*;

use super::rules::{
    capitalize_first, ensure_concept_in_can, forward_chain, universal_to_rule,
};
use super::{compare, state, Binding, Grounded};

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

// ---------- helpers for assertion ----------

/// Ensure a Relation action exists for `rel_id` (e.g. "rel.own"). Creates a
/// provisional one with `Impl::Primitive` if absent.
pub(super) fn ensure_relation(
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
pub(super) fn resolve_term(term: &Term, bindings: &HashMap<String, Binding>) -> Option<Value> {
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
pub(super) fn make_fact(
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

        // An event also moves the world: location and possession state.
        if pred.pred != "be" {
            stored_facts.extend(state::derive(w, pred, &g.bindings, source, episode_id)?);
        }
    }

    Ok(AssertOutcome::Stored {
        facts: stored_facts,
        new_relations,
        new_concepts,
    })
}

/// "The name of Assistant is Spoon" / "The mood of Assistant is calm": the
/// subject is a possessed noun. Returns (property noun, owner value) so the
/// fact becomes `rel.<noun>(owner, value)` instead of an is_a on a minted
/// entity that nobody can query back.
pub(super) fn property_subject(
    term: &Term,
    bindings: &HashMap<String, Binding>,
    all_refs: &[&Referent],
) -> Option<(String, Value)> {
    let Term::Var { var } = term else { return None };
    let r = all_refs.iter().find(|r| &r.var == var)?;
    let owner_var = r.owner.as_deref()?;
    let noun = r.noun.clone()?;
    let owner = resolve_term(&Term::Var { var: owner_var.to_string() }, bindings)?;
    Some((noun, owner))
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
    if let Some((noun, owner)) = pred.args.first().and_then(|t| property_subject(t, bindings, all_refs)) {
        let value = match &pred.attr {
            Some(attr) => Value::Text(attr.clone()),
            None => resolve_term(pred.args.get(1)?, bindings)?,
        };
        let action_id = ensure_verb_relation(w, &noun, 2, sce);
        let fact = make_fact(action_id.clone(), vec![owner, value], pred.negated, &pred.modal, source, episode_id);
        return Some((action_id, fact));
    }
    if let Some(attr) = pred.attr.as_deref().filter(|a| compare::is_comparative(a)) {
        if pred.args.len() >= 2 {
            return compare::fact_for(w, pred, attr, bindings, source, episode_id);
        }
    }
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
        let v = match term {
            // "User thinks that dogs are great." keeps the user's words.
            Term::Sub { clause } => Value::Text(super::realize::sub_clause_text(sce, clause)),
            _ => resolve_term(term, bindings)?,
        };
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

// ---------- concept-level is_a ----------

/// Store `rel.is_a(child, parent)` between two *concepts* (not entities), so
/// "Is a dog an animal?" can be answered without ever meeting a dog. Both
/// concepts are created provisionally if they are new. Returns `None` when
/// the fact was already there.
pub fn assert_class_is_a(
    w: &mut FactWriter<'_>,
    child_noun: &str,
    parent_noun: &str,
    source: &str,
) -> anyhow::Result<Option<Fact>> {
    let child = ensure_concept_in_can(w.can, w.store, child_noun);
    let parent = ensure_concept_in_can(w.can, w.store, parent_noun);
    if child == parent {
        return Ok(None);
    }
    let pred = ensure_relation(w, "rel.is_a", &["is_a"], 2, "");
    let args = vec![Value::Name(child.0.clone()), Value::Name(parent.0.clone())];
    let pattern: Vec<Option<Value>> = args.iter().map(|a| Some(a.clone())).collect();
    if !w.store.query_facts(&pred, &pattern).unwrap_or_default().is_empty() {
        return Ok(None);
    }
    let fact = make_fact(pred, args, false, &None, source, None);
    let id = w.store.insert_fact(&fact)?;
    Ok(Some(Fact { id, ..fact }))
}

// ---------- supersede ----------

/// Invalidate an existing fact and insert the incoming replacement.
pub fn supersede(store: &Store, existing_id: i64, incoming: &Fact) -> anyhow::Result<i64> {
    store.invalidate_fact(existing_id, now_ms())?;
    store.insert_fact(incoming)
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
pub(super) fn action_id_for(can: &Can, verb: &str) -> Option<ActionId> {
    let direct = ActionId(format!("rel.{}", verb));
    if can.action(&direct).is_some() {
        return Some(direct);
    }
    can.actions_for_verb(verb)
        .iter()
        .find(|a| a.role == Role::Relation)
        .map(|a| a.id.clone())
}
