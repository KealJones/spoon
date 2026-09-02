//! Grounding: maps discourse referents to entity values or binding types.
//!
//! Quant rules (from the spec):
//!   Named(n) -> look up or create entity. User/Assistant get their fixed concept.
//!               Single uppercase letters are turn-scoped variables.
//!   Literal(v) -> Binding::Literal(v)
//!   Indef (Assert) -> mint new entity "{noun}_{k}". Count(n) adds "count=n" mod.
//!   Indef (Question/Command) -> bind to most salient matching entity, else Unbound.
//!   Def -> most recently mentioned entity with matching noun or concept ancestry;
//!          if none found, mint a new one (ACE behavior).
//!   Every/No/AtLeast/AtMost/Exactly -> Binding::Universal (becomes a rule).
//!   Wh -> Binding::Query.

use std::collections::HashMap;

use spoon_core::can::Can;
use spoon_core::types::*;

use super::{Binding, DiscourseState, Entity, Grounded};

// ---------- static data ----------

/// Common English first names used to infer Person concept for Named referents.
const FIRST_NAMES: &[&str] = &[
    "alice", "anna", "alex", "ben", "bob", "charlie", "chris", "david", "diana",
    "emma", "eric", "fiona", "frank", "george", "grace", "helen", "henry", "iris",
    "ivan", "jack", "james", "jane", "john", "julia", "kate", "kevin", "laura",
    "lisa", "luke", "mark", "mary", "mike", "nancy", "nick", "olivia", "oscar",
    "paul", "peter", "quinn", "rose", "rex", "sam", "sarah", "sandra", "tim",
    "tom", "uma", "ursula", "vera", "victor", "wendy", "will", "xavier", "xena",
    "yara", "yvonne", "zachary", "zoe",
];

fn is_first_name(name: &str) -> bool {
    FIRST_NAMES.contains(&name.to_lowercase().as_str())
}

/// Single capital letter optionally followed by digits: X, Y, Z, X1, X2 etc.
fn is_turn_variable(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => chars.all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

// ---------- Def resolution ----------

/// Find the most recently mentioned entity whose noun or concept matches
/// the target referent. Checks `referent.mods` for adjective filtering too.
fn find_def<'a>(entities: &'a [Entity], referent: &Referent, can: &Can) -> Option<&'a Entity> {
    let noun_lower = referent.noun.as_deref().unwrap_or("").to_lowercase();

    // Concepts that correspond to the target noun.
    let target_concepts: Vec<ConceptId> = can
        .concepts_for_noun(&noun_lower)
        .iter()
        .map(|c| c.id.clone())
        .collect();

    let candidates: Vec<&Entity> = entities
        .iter()
        .filter(|e| {
            let noun_match = e
                .noun
                .as_deref()
                .map(|n| n.to_lowercase() == noun_lower)
                .unwrap_or(false);

            let concept_match = e.concept.as_ref().map(|ec| {
                target_concepts.iter().any(|tc| can.is_a(ec, tc))
                    // Also accept exact concept id match (e.g. resolving "the Dog" when entity has concept "Dog")
                    || target_concepts.contains(ec)
            }).unwrap_or(false);

            let mods_match = referent.mods.iter().all(|m| e.mods.contains(m));

            (noun_match || concept_match) && mods_match
        })
        .collect();

    candidates.into_iter().max_by_key(|e| (e.last_mentioned, e.mentions))
}

// ---------- main grounding ----------

/// Ground a single referent. Returns the binding and, when a new entity was
/// created (Indef/Count/new-Def), the entity to add to state.
fn ground_referent(
    entities: &[Entity],
    mint_counter: &mut u32,
    turn: i64,
    referent: &Referent,
    can: &Can,
    is_assert: bool,
    has_universal: bool,
) -> (Binding, Option<Entity>) {
    match &referent.quant {
        Quant::Named(name) => {
            let fixed_concept = match name.as_str() {
                "User" => Some(ConceptId("User".into())),
                "Assistant" => Some(ConceptId("Assistant".into())),
                _ => None,
            };
            // Turn-scoped variable (single capital letter + optional digits)
            // treated like a name but ephemeral (will age out at cap-200).
            let id = Value::Name(name.clone());
            // Look up existing entity.
            let existing = entities.iter().find(|e| e.id == id);
            if existing.is_some() {
                return (Binding::Entity(id), None);
            }
            // Create entity for new names (NOT added to new_entities - named
            // things are addressed, not freshly introduced discourse objects).
            let concept = fixed_concept.or_else(|| {
                referent
                    .noun
                    .as_deref()
                    .and_then(|n| can.concepts_for_noun(n).into_iter().next().map(|c| c.id.clone()))
                    .or_else(|| {
                        if is_first_name(name) || is_turn_variable(name) {
                            Some(ConceptId("Person".into()))
                        } else {
                            None
                        }
                    })
            });
            let entity = Entity {
                id: id.clone(),
                concept,
                noun: referent.noun.clone(),
                mods: referent.mods.clone(),
                last_mentioned: turn,
                mentions: 0,
            };
            // Named entities go to state.entities but NOT new_entities.
            // We signal this by returning Some but callers handle it separately.
            (Binding::Entity(id), Some(entity))
        }

        Quant::Literal(v) => (Binding::Literal(v.clone()), None),

        Quant::Indef => {
            if is_assert && !has_universal {
                // Mint a new entity.
                *mint_counter += 1;
                let noun = referent.noun.as_deref().unwrap_or("thing");
                let name = format!("{}_{}", noun, mint_counter);
                let id = Value::Name(name.clone());
                let concept = can
                    .concepts_for_noun(noun)
                    .into_iter()
                    .next()
                    .map(|c| c.id.clone())
                    .or_else(|| Some(ConceptId(capitalize_first(noun))));
                let entity = Entity {
                    id,
                    concept,
                    noun: referent.noun.clone(),
                    mods: referent.mods.clone(),
                    last_mentioned: turn,
                    mentions: 0,
                };
                (Binding::Entity(Value::Name(name)), Some(entity))
            } else {
                // Question or Command: try to bind to salient entity.
                if let Some(noun) = &referent.noun {
                    let fake_ref = Referent {
                        var: referent.var.clone(),
                        noun: Some(noun.clone()),
                        quant: Quant::Def,
                        mods: referent.mods.clone(),
                        owner: None,
                        span: None,
                    };
                    if let Some(e) = find_def(entities, &fake_ref, can) {
                        return (Binding::Entity(e.id.clone()), None);
                    }
                }
                (Binding::Unbound, None)
            }
        }

        Quant::Count(n) => {
            if is_assert && !has_universal {
                *mint_counter += 1;
                let noun = referent.noun.as_deref().unwrap_or("thing");
                let name = format!("{}_{}", noun, mint_counter);
                let id = Value::Name(name.clone());
                let concept = can
                    .concepts_for_noun(noun)
                    .into_iter()
                    .next()
                    .map(|c| c.id.clone())
                    .or_else(|| Some(ConceptId(capitalize_first(noun))));
                let mut mods = referent.mods.clone();
                mods.push(format!("count={}", n));
                let entity = Entity {
                    id,
                    concept,
                    noun: referent.noun.clone(),
                    mods,
                    last_mentioned: turn,
                    mentions: 0,
                };
                (Binding::Entity(Value::Name(name)), Some(entity))
            } else {
                (Binding::Unbound, None)
            }
        }

        Quant::Def => {
            if let Some(existing) = find_def(entities, referent, can) {
                return (Binding::Entity(existing.id.clone()), None);
            }
            // ACE behavior: mint a new entity (Def resolution failure).
            *mint_counter += 1;
            let noun = referent.noun.as_deref().unwrap_or("thing");
            let name = match &referent.owner {
                Some(owner_var) => {
                    // Find owner name from entities.
                    let owner_val = entities.iter().find(|e| {
                        if let Value::Name(_) = &e.id {
                            // Check if this entity's var matches owner_var
                            // We don't have var mapping here; use noun as fallback.
                            false
                        } else {
                            false
                        }
                    });
                    let owner_name = owner_val
                        .and_then(|e| if let Value::Name(n) = &e.id { Some(n.clone()) } else { None })
                        .unwrap_or_else(|| owner_var.clone());
                    format!("{}_{}", owner_name, noun)
                }
                None => format!("{}_{}", noun, mint_counter),
            };
            let id = Value::Name(name.clone());
            let concept = can
                .concepts_for_noun(noun)
                .into_iter()
                .next()
                .map(|c| c.id.clone());
            let entity = Entity {
                id,
                concept,
                noun: referent.noun.clone(),
                mods: referent.mods.clone(),
                last_mentioned: turn,
                mentions: 0,
            };
            (Binding::Entity(Value::Name(name)), Some(entity))
        }

        Quant::Every | Quant::No | Quant::AtLeast(_) | Quant::AtMost(_) | Quant::Exactly(_) => {
            (
                Binding::Universal {
                    noun: referent.noun.clone(),
                    quant: referent.quant.clone(),
                },
                None,
            )
        }

        Quant::Wh => (Binding::Query, None),
    }
}

/// Ground a single clause, updating discourse state with new/existing entities.
pub fn ground(state: &mut DiscourseState, clause: &Clause, can: &Can) -> Grounded {
    let turn = state.turn;
    let is_assert = matches!(clause.act, Act::Assert | Act::Rule);
    let is_q_or_cmd = matches!(clause.act, Act::Question { .. } | Act::Command);

    // Check if any referent has a universal quantifier.
    let has_universal = clause
        .referents
        .iter()
        .chain(clause.then_referents.iter())
        .any(|r| {
            matches!(
                r.quant,
                Quant::Every | Quant::No | Quant::AtLeast(_) | Quant::AtMost(_) | Quant::Exactly(_)
            )
        });

    let all_refs: Vec<&Referent> = clause
        .referents
        .iter()
        .chain(clause.then_referents.iter())
        .collect();

    let mut bindings: HashMap<String, Binding> = HashMap::new();
    // Entities created by Indef/Count/new-Def that are discourse-new.
    let mut new_entities: Vec<Entity> = Vec::new();
    // Named entities created because they were absent from state (not discourse-new).
    let mut implicit_entities: Vec<Entity> = Vec::new();

    let mint_counter = &mut state.mint_counter;

    for referent in &all_refs {
        let (binding, maybe_new) = ground_referent(
            &state.entities,
            mint_counter,
            turn,
            referent,
            can,
            is_assert && !is_q_or_cmd,
            has_universal,
        );

        if let Some(new_ent) = maybe_new {
            match &referent.quant {
                Quant::Named(_) => {
                    // Named entities: add to state but not new_entities.
                    implicit_entities.push(new_ent);
                }
                _ => {
                    // Indef/Count/Def-new: discourse-new entities.
                    state.entities.push(new_ent.clone());
                    new_entities.push(new_ent);
                }
            }
        }
        bindings.insert(referent.var.clone(), binding);
    }

    // Add implicit (named) entities to state.
    state.entities.extend(implicit_entities);

    // Update salience for all bound entities.
    for binding in bindings.values() {
        if let Binding::Entity(Value::Name(name)) = binding {
            if let Some(e) = state
                .entities
                .iter_mut()
                .find(|e| matches!(&e.id, Value::Name(n) if n == name))
            {
                e.last_mentioned = turn;
                e.mentions = e.mentions.saturating_add(1);
            }
        }
    }

    // Cap at 200, dropping the least recently mentioned.
    if state.entities.len() > 200 {
        state
            .entities
            .sort_by(|a, b| b.last_mentioned.cmp(&a.last_mentioned));
        state.entities.truncate(200);
    }

    Grounded {
        clause: clause.clone(),
        bindings,
        new_entities,
    }
}
