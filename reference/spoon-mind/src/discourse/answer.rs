//! Facts layer, question side: answer grounded questions from stored facts.
//!
//! Answer rules:
//!   - YesNo: query by pred+bound args; check truth; fallback to CAN hierarchy.
//!     `Is X bigger than Y?` follows the comparison chain (`compare.rs`).
//!   - Who/What/Which/When: collect values at the Query position.
//!   - Where + copula: the subject's current location through the carry
//!     chain (`state.rs`).
//!   - HowMany: count the matching facts, summing rel.count facts, keeping
//!     only values that fit the asked noun ("How many apples ...").
//!
//! Memory never guesses: no fact means `Answer::Unknown`.

use std::collections::HashMap;

use spoon_core::can::Can;
use spoon_core::store::Store;
use spoon_core::types::*;

use spoon_lang::sce::singularize_noun;

use super::facts::{action_id_for, property_subject, resolve_term};
use super::rules::capitalize_first;
use super::{compare, state, Binding, Grounded};

#[derive(Debug)]
pub enum Answer {
    Values(Vec<Value>),
    YesNo(bool, Option<Fact>),
    Count(i64),
    Unknown { reason: String },
}

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
        QuestionKind::Where { .. } if pred.pred == "be" => Ok(answer_where(store, g, pred)),
        QuestionKind::Who { focus }
        | QuestionKind::What { focus }
        | QuestionKind::Which { focus }
        | QuestionKind::Where { focus }
        | QuestionKind::When { focus } => {
            answer_wh(can, store, g, pred, &all_refs, focus)
        }
        QuestionKind::HowMany { focus } => {
            answer_how_many(can, store, g, pred, &all_refs, focus)
        }
    }
}

/// Everything a concept is, following stored concept-level `rel.is_a` facts
/// and the CAN hierarchy: `Dog` -> `Animal` -> `Entity`. Concept-level facts
/// are what the lookup chain writes, and what makes `Is a dog an animal?`
/// answerable without ever meeting a particular dog.
pub fn class_parents(can: &Can, store: &Store, concept: &ConceptId) -> Vec<ConceptId> {
    let mut seen: Vec<ConceptId> = vec![concept.clone()];
    let mut frontier = vec![concept.clone()];
    while let Some(current) = frontier.pop() {
        for p in class_parents_direct(can, store, &current) {
            if !seen.contains(&p) {
                seen.push(p.clone());
                frontier.push(p);
            }
        }
    }
    seen.remove(0);
    seen
}

/// One hop up: what a concept is, in the shortest true answer.
pub fn class_parents_direct(can: &Can, store: &Store, concept: &ConceptId) -> Vec<ConceptId> {
    let mut parents: Vec<ConceptId> = can.concept(concept).map(|c| c.extends.clone()).unwrap_or_default();
    let stored = store
        .query_facts(&ActionId("rel.is_a".into()), &[Some(Value::Name(concept.0.clone())), None])
        .unwrap_or_default();
    for f in stored.iter().filter(|f| f.truth) {
        if let Some(Value::Name(parent)) = f.args.get(1) {
            parents.push(ConceptId(parent.clone()));
        }
    }
    // `Thing` is the root every provisional concept extends; it is true and
    // useless as an answer unless nothing else is known.
    if parents.iter().any(|p| p.0 != "Thing") {
        parents.retain(|p| p.0 != "Thing");
    }
    parents.dedup();
    parents.retain(|p| p != concept);
    parents
}

/// The concept a bare noun names ("dog" -> `Dog`), whether or not the CAN
/// has met it.
pub fn concept_of_noun(can: &Can, noun: &str) -> ConceptId {
    let noun = singularize_noun(noun);
    can.concepts_for_noun(&noun)
        .into_iter()
        .next()
        .map(|c| c.id.clone())
        .unwrap_or_else(|| ConceptId(capitalize_first(&noun)))
}

/// `Where is X?`: X's current location, following whoever carries it.
fn answer_where(store: &Store, g: &Grounded, pred: &Pred) -> Answer {
    let Some(subject) = pred.args.first().and_then(|t| resolve_term(t, &g.bindings)) else {
        return Answer::Unknown { reason: "unresolved subject".into() };
    };
    match state::location_of(store, &subject) {
        Some(place) => Answer::Values(vec![place]),
        None => Answer::Unknown { reason: "no location".into() },
    }
}

fn answer_yes_no(
    can: &Can,
    store: &Store,
    g: &Grounded,
    pred: &Pred,
    all_refs: &[&Referent],
) -> anyhow::Result<Answer> {
    if pred.pred == "be" {
        return answer_be_yesno(can, store, g, pred, all_refs);
    }

    let action_id = action_id_for(can, &pred.pred);
    let Some(action_id) = action_id else {
        return Ok(Answer::Unknown { reason: format!("unknown predicate '{}'", pred.pred) });
    };

    let pattern = build_pattern(pred, &g.bindings);
    let facts = store.query_facts(&action_id, &pattern).unwrap_or_default();
    let facts = filter_by_noun_constraints(can, store, g, pred, all_refs, facts);

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
    // Property form: "Is the wellbeing of Assistant good?" -> rel.wellbeing(Assistant, ?)
    if let Some((noun, owner)) = pred.args.first().and_then(|t| property_subject(t, &g.bindings, all_refs)) {
        let Some(action_id) = action_id_for(can, &noun) else {
            return Ok(Answer::Unknown { reason: format!("nothing known about {noun}") });
        };
        let want = match &pred.attr {
            Some(attr) => Some(Value::Text(attr.clone())),
            None => pred.args.get(1).and_then(|t| resolve_term(t, &g.bindings)),
        };
        let facts = store.query_facts(&action_id, &[Some(owner), None]).unwrap_or_default();
        let Some(want) = want else {
            return Ok(Answer::Unknown { reason: "unresolved object".into() });
        };
        if let Some(f) = facts.iter().find(|f| f.args.get(1) == Some(&want)) {
            return Ok(Answer::YesNo(f.truth, Some(f.clone())));
        }
        if let Some(f) = facts.iter().find(|f| f.truth) {
            return Ok(Answer::YesNo(false, Some(f.clone())));
        }
        return Ok(Answer::Unknown { reason: format!("no {noun} fact") });
    }

    // Comparative form: "Is the elephant bigger than the cat?"
    if let Some(attr) = pred.attr.as_deref().filter(|a| compare::is_comparative(a)) {
        if pred.args.len() >= 2 {
            return Ok(compare::answer(store, pred, attr, &g.bindings));
        }
    }

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
        // Class membership first: "Is a dog an animal?" is about the concepts,
        // and no particular dog needs to exist for it to be true.
        if let (Some(child), Some(parent)) =
            (subject_noun(&pred.args[0], all_refs), subject_noun(&pred.args[1], all_refs))
        {
            let child = concept_of_noun(can, &child);
            let parent = concept_of_noun(can, &parent);
            if child == parent || class_parents(can, store, &child).contains(&parent) {
                return Ok(Answer::YesNo(true, None));
            }
        }
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
            return Ok(Answer::Unknown { reason: "unresolved object of 'be'".into() });
        }
    }

    Ok(Answer::Unknown { reason: "no is_a fact".into() })
}

/// The head noun of the referent a term points at, for questions about a
/// class rather than an individual ("a dog", "the avengers").
fn subject_noun(term: &Term, all_refs: &[&Referent]) -> Option<String> {
    let Term::Var { var } = term else { return None };
    all_refs.iter().find(|r| &r.var == var)?.noun.clone()
}

/// A concept as the phrase that answers "what is it": `Animal` -> "an animal".
fn class_phrase(can: &Can, concept: &ConceptId) -> String {
    let noun = can
        .concept(concept)
        .and_then(|c| c.nouns.first().cloned())
        .unwrap_or_else(|| concept.0.to_lowercase());
    let article = if noun.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
    format!("{article} {noun}")
}

/// Same, but only for `a dog` and bare plurals: a class, not an individual.
fn indefinite_noun(term: &Term, all_refs: &[&Referent]) -> Option<String> {
    let Term::Var { var } = term else { return None };
    let r = all_refs.iter().find(|r| &r.var == var)?;
    matches!(r.quant, Quant::Indef | Quant::Every).then(|| r.noun.clone())?
}

fn answer_wh(
    can: &Can,
    store: &Store,
    g: &Grounded,
    pred: &Pred,
    all_refs: &[&Referent],
    focus: &str,
) -> anyhow::Result<Answer> {
    if pred.pred == "be" {
        // "What is the name of Assistant?" -> rel.name(Assistant, ?). The
        // possessed noun may sit on either side of the copula.
        if let Some((noun, owner)) =
            pred.args.iter().find_map(|t| property_subject(t, &g.bindings, all_refs))
        {
            let Some(action_id) = action_id_for(can, &noun) else {
                return Ok(Answer::Unknown { reason: format!("nothing known about {noun}") });
            };
            let facts = store.query_facts(&action_id, &[Some(owner), None]).unwrap_or_default();
            let values: Vec<Value> =
                facts.iter().filter(|f| f.truth).filter_map(|f| f.args.get(1).cloned()).collect();
            return Ok(if values.is_empty() {
                Answer::Unknown { reason: format!("no {noun} fact") }
            } else {
                Answer::Values(values)
            });
        }

    }

    let action_id = action_id_for(can, &pred.pred)
        .or_else(|| Some(ActionId(format!("rel.{}", pred.pred))));
    let Some(action_id) = action_id else {
        return Ok(Answer::Unknown { reason: format!("unknown predicate '{}'", pred.pred) });
    };

    let mut pattern = build_pattern(pred, &g.bindings);
    let pos = focus_position(pred, focus, &mut pattern);
    let facts = store.query_facts(&action_id, &pattern).unwrap_or_default();
    let facts = filter_by_noun_constraints(can, store, g, pred, all_refs, facts);

    let mut values: Vec<Value> = facts
        .iter()
        .filter(|f| f.truth)
        .filter_map(|f| f.args.get(pos).cloned())
        .collect();

    values.sort_by(|a, b| a.render().cmp(&b.render()));
    values.dedup_by(|a, b| a == b);

    if !values.is_empty() {
        return Ok(Answer::Values(values));
    }

    // Last resort for "What is a dog?": what the concept Dog is a kind of.
    // The wh slot is the argument with no noun, so the other one names the
    // class. Indefinite only: "the time" asks about a particular thing, and
    // answering "an entity" would be true and useless.
    if pred.pred == "be" {
        if let Some(noun) = pred.args.iter().find_map(|t| indefinite_noun(t, all_refs)) {
            let parents = class_parents_direct(can, store, &concept_of_noun(can, &noun));
            if !parents.is_empty() {
                return Ok(Answer::Values(parents.iter().map(|c| Value::Text(class_phrase(can, c))).collect()));
            }
        }
    }
    Ok(Answer::Unknown { reason: "no matching fact".into() })
}

/// Drop facts whose argument does not fit the noun of an open referent:
/// "Who owns a cat?" must not match own(John, dog_1). A value fits a noun
/// when it has an is_a fact to that noun's concept (or a subconcept).
fn filter_by_noun_constraints(
    can: &Can,
    store: &Store,
    g: &Grounded,
    pred: &Pred,
    all_refs: &[&Referent],
    mut facts: Vec<Fact>,
) -> Vec<Fact> {
    for (pos, term) in pred.args.iter().enumerate() {
        let Term::Var { var } = term else { continue };
        if !matches!(g.bindings.get(var.as_str()), Some(Binding::Unbound)) {
            continue;
        }
        let Some(noun) = all_refs.iter().find(|r| &r.var == var).and_then(|r| r.noun.as_deref()) else {
            continue;
        };
        facts.retain(|f| f.args.get(pos).is_some_and(|v| value_fits_noun(can, store, v, noun)));
    }
    facts
}

/// Nouns that name anything at all ("How many objects does Mary carry?").
fn is_generic_noun(noun: &str) -> bool {
    matches!(singularize_noun(noun).as_str(), "thing" | "object" | "item" | "entity" | "one")
}

/// Does `value` have an is_a fact to `noun`'s concept (or a subconcept)?
fn value_fits_noun(can: &Can, store: &Store, value: &Value, noun: &str) -> bool {
    let noun = singularize_noun(noun);
    let mut targets: Vec<ConceptId> = can.concepts_for_noun(&noun).iter().map(|c| c.id.clone()).collect();
    if targets.is_empty() {
        targets.push(ConceptId(capitalize_first(&noun)));
    }
    store
        .query_facts(&ActionId("rel.is_a".into()), &[Some(value.clone()), None])
        .unwrap_or_default()
        .iter()
        .filter(|f| f.truth)
        .any(|f| match f.args.get(1) {
            Some(Value::Name(n)) => {
                let c = ConceptId(n.clone());
                targets.iter().any(|tc| &c == tc || can.is_a(&c, tc))
            }
            _ => false,
        })
}

fn answer_how_many(
    can: &Can,
    store: &Store,
    g: &Grounded,
    pred: &Pred,
    all_refs: &[&Referent],
    focus: &str,
) -> anyhow::Result<Answer> {
    // Build query pattern: focus position = None, rest = bound.
    let mut pattern = build_pattern(pred, &g.bindings);
    let pos = focus_position(pred, focus, &mut pattern);

    let action_id = action_id_for(can, &pred.pred).unwrap_or_else(|| ActionId(format!("rel.{}", pred.pred)));

    let mut facts = store.query_facts(&action_id, &pattern).unwrap_or_default();

    // "How many apples does Mary carry?" counts apples only; "objects" counts all.
    let focus_noun = all_refs.iter().find(|r| r.var == focus).and_then(|r| r.noun.as_deref());
    if let Some(noun) = focus_noun.filter(|n| !is_generic_noun(n)) {
        facts.retain(|f| f.args.get(pos).is_some_and(|v| value_fits_noun(can, store, v, noun)));
    }

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

/// The argument position the wh-referent answers. The parser leaves the
/// wh-object of a transitive verb out of the predicate ("What does Mary
/// carry?" -> carry(Mary)); the missing argument is the answer slot, so an
/// open position is appended to the pattern for it.
fn focus_position(pred: &Pred, focus: &str, pattern: &mut Vec<Option<Value>>) -> usize {
    let in_args = pred.args.iter().position(|t| matches!(t, Term::Var { var } if var == focus));
    match in_args {
        Some(pos) => pos,
        None if pred.attr.is_none() => {
            pattern.push(None);
            pattern.len() - 1
        }
        None => 0,
    }
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
