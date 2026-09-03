//! World state derived from event assertions.
//!
//! An event sentence (`Mary moves to the garden.`, `Mary picks-up the apple.`)
//! is stored as an event fact by the facts layer; this module derives the
//! state it implies through a small verb table:
//!
//! - motion verb + `to` place: `rel.location(subject, place)`, superseding
//!   the subject's previous location so the history stays in the store;
//! - take verbs: `rel.carry(subject, object)`;
//! - drop verbs: the carry fact is retracted and the object's own location is
//!   pinned to wherever the subject is at that moment;
//! - `give X to Y`: the carry fact moves from the subject to Y.
//!
//! Location resolves at query time through the carry chain (`location_of`):
//! a carried object is wherever its carrier is. A move therefore writes one
//! fact and never fans out to the carried objects, and there is no second
//! copy of the truth to keep in sync. Only a drop materializes the object's
//! own location, because from then on it stops travelling.
//!
//! Everything here is deterministic store logic; no LLM.

use std::collections::HashMap;

use spoon_core::store::Store;
use spoon_core::types::*;

use super::facts::{ensure_relation, make_fact, resolve_term, supersede, FactWriter};
use super::Binding;

pub const LOCATION: &str = "rel.location";
pub const CARRY: &str = "rel.carry";

/// How deep the carry chain is followed when resolving a location.
const CARRY_DEPTH: usize = 4;

enum Event {
    Move,
    Take,
    /// `leave X` only means "put X down" when X was being carried; the other
    /// drop verbs always pin the object where the subject is.
    Drop { only_if_carried: bool },
    Give,
}

fn event_for(verb: &str) -> Option<Event> {
    Some(match verb {
        "move" | "go" | "walk" | "run" | "travel" | "head" | "return" | "come" | "journey" => Event::Move,
        "pick-up" | "pick" | "take" | "grab" | "get" | "collect" => Event::Take,
        "drop" | "put-down" | "discard" => Event::Drop { only_if_carried: false },
        "leave" => Event::Drop { only_if_carried: true },
        "give" | "hand" | "pass" => Event::Give,
        _ => return None,
    })
}

/// Apply the state rules for one stored event predicate. Returns the state
/// facts written (id filled in). Negated and modal events change nothing.
pub fn derive(
    w: &mut FactWriter<'_>,
    pred: &Pred,
    bindings: &HashMap<String, Binding>,
    source: &str,
    episode_id: Option<i64>,
) -> anyhow::Result<Vec<Fact>> {
    if pred.negated || pred.modal.is_some() {
        return Ok(vec![]);
    }
    let Some(event) = event_for(&pred.pred) else { return Ok(vec![]) };
    let arg = |i: usize| pred.args.get(i).and_then(|t| resolve_term(t, bindings));
    let adjunct = |prep: &str| pred.adjuncts.iter().find(|(p, _)| p == prep).and_then(|(_, t)| resolve_term(t, bindings));
    let Some(subject) = arg(0) else { return Ok(vec![]) };
    let sce = "";

    let mut written = Vec::new();
    match event {
        Event::Move => {
            let Some(place) = adjunct("to").or_else(|| adjunct("into")) else { return Ok(vec![]) };
            ensure_relation(w, LOCATION, &["location"], 2, sce);
            written.extend(set_location(w.store, &subject, &place, source, episode_id)?);
        }
        Event::Take => {
            let Some(object) = arg(1) else { return Ok(vec![]) };
            ensure_relation(w, CARRY, &["carry"], 2, sce);
            written.extend(start_carrying(w.store, &subject, &object, source, episode_id)?);
        }
        Event::Drop { only_if_carried } => {
            let Some(object) = arg(1) else { return Ok(vec![]) };
            let was_carried = stop_carrying(w.store, &subject, &object)?;
            if was_carried || !only_if_carried {
                if let Some(place) = location_of(w.store, &subject) {
                    ensure_relation(w, LOCATION, &["location"], 2, sce);
                    written.extend(set_location(w.store, &object, &place, source, episode_id)?);
                }
            }
        }
        Event::Give => {
            let (Some(object), Some(recipient)) = (arg(1), adjunct("to")) else { return Ok(vec![]) };
            ensure_relation(w, CARRY, &["carry"], 2, sce);
            stop_carrying(w.store, &subject, &object)?;
            written.extend(start_carrying(w.store, &recipient, &object, source, episode_id)?);
        }
    }
    Ok(written)
}

/// `location(entity) := place`, invalidating any earlier location. Writes
/// nothing when the entity is already there.
fn set_location(store: &Store, entity: &Value, place: &Value, source: &str, episode_id: Option<i64>) -> anyhow::Result<Vec<Fact>> {
    let current = store.query_facts(&ActionId(LOCATION.into()), &[Some(entity.clone()), None])?;
    if current.iter().any(|f| f.truth && f.args.get(1) == Some(place)) {
        return Ok(vec![]);
    }
    let fact = make_fact(ActionId(LOCATION.into()), vec![entity.clone(), place.clone()], false, &None, source, episode_id);
    let id = match current.split_first() {
        Some((first, rest)) => {
            for f in rest {
                store.invalidate_fact(f.id, now_ms())?;
            }
            supersede(store, first.id, &fact)?
        }
        None => store.insert_fact(&fact)?,
    };
    Ok(vec![Fact { id, ..fact }])
}

fn start_carrying(store: &Store, carrier: &Value, object: &Value, source: &str, episode_id: Option<i64>) -> anyhow::Result<Vec<Fact>> {
    let carry = ActionId(CARRY.into());
    let holders = store.query_facts(&carry, &[None, Some(object.clone())])?;
    if holders.iter().any(|f| f.truth && f.args.first() == Some(carrier)) {
        return Ok(vec![]);
    }
    // One carrier at a time: whoever held it before hands it over.
    for f in &holders {
        store.invalidate_fact(f.id, now_ms())?;
    }
    let fact = make_fact(carry, vec![carrier.clone(), object.clone()], false, &None, source, episode_id);
    let id = store.insert_fact(&fact)?;
    Ok(vec![Fact { id, ..fact }])
}

/// Retract `carry(carrier, object)`. True when there was one to retract.
fn stop_carrying(store: &Store, carrier: &Value, object: &Value) -> anyhow::Result<bool> {
    let facts = store.query_facts(&ActionId(CARRY.into()), &[Some(carrier.clone()), Some(object.clone())])?;
    let mut any = false;
    for f in facts.iter().filter(|f| f.truth) {
        store.invalidate_fact(f.id, now_ms())?;
        any = true;
    }
    Ok(any)
}

/// Who currently carries `object`, if anyone.
pub fn carrier_of(store: &Store, object: &Value) -> Option<Value> {
    store
        .query_facts(&ActionId(CARRY.into()), &[None, Some(object.clone())])
        .ok()?
        .into_iter()
        .find(|f| f.truth)
        .and_then(|f| f.args.first().cloned())
}

/// What `carrier` currently carries, oldest first. Questions reach the carry
/// facts through the generic wh path; this is the test's view of the state.
#[cfg(test)]
fn carried_by(store: &Store, carrier: &Value) -> Vec<Value> {
    let mut facts = store.query_facts(&ActionId(CARRY.into()), &[Some(carrier.clone()), None]).unwrap_or_default();
    facts.retain(|f| f.truth);
    facts.sort_by_key(|f| f.id);
    facts.into_iter().filter_map(|f| f.args.get(1).cloned()).collect()
}

/// Where `entity` is now: with its carrier if it is carried (following the
/// chain up to `CARRY_DEPTH` hops), else its own current location fact.
pub fn location_of(store: &Store, entity: &Value) -> Option<Value> {
    let mut current = entity.clone();
    for _ in 0..CARRY_DEPTH {
        match carrier_of(store, &current) {
            Some(carrier) if carrier != current => current = carrier,
            _ => break,
        }
    }
    store
        .query_facts(&ActionId(LOCATION.into()), &[Some(current), None])
        .ok()?
        .into_iter()
        .find(|f| f.truth)
        .and_then(|f| f.args.get(1).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spoon_core::can::Can;

    fn fw<'a>(can: &'a mut Can, store: &'a Store) -> FactWriter<'a> {
        FactWriter { can, store }
    }

    fn event(verb: &str, args: &[&str], adjuncts: &[(&str, &str)]) -> (Pred, HashMap<String, Binding>) {
        let mut bindings = HashMap::new();
        let mut term = |name: &str| {
            let var = format!("x_{name}");
            bindings.insert(var.clone(), Binding::Entity(Value::name(name)));
            Term::Var { var }
        };
        let args: Vec<Term> = args.iter().map(|a| term(a)).collect();
        let adjuncts: Vec<(String, Term)> = adjuncts.iter().map(|(p, a)| (p.to_string(), term(a))).collect();
        (Pred { pred: verb.into(), args, negated: false, modal: None, adjuncts, attr: None }, bindings)
    }

    fn apply(can: &mut Can, store: &Store, verb: &str, args: &[&str], adjuncts: &[(&str, &str)]) {
        let (pred, bindings) = event(verb, args, adjuncts);
        derive(&mut fw(can, store), &pred, &bindings, "test", None).unwrap();
    }

    #[test]
    fn moves_supersede_and_carried_objects_travel() {
        let mut can = Can::new();
        let store = Store::open_memory().unwrap();
        apply(&mut can, &store, "move", &["Mary"], &[("to", "garden_1")]);
        apply(&mut can, &store, "pick-up", &["Mary", "apple_1"], &[]);
        apply(&mut can, &store, "move", &["Mary"], &[("to", "kitchen_1")]);

        assert_eq!(location_of(&store, &Value::name("Mary")), Some(Value::name("kitchen_1")));
        assert_eq!(location_of(&store, &Value::name("apple_1")), Some(Value::name("kitchen_1")));
        // The old location is history, not a second current value.
        let current = store.query_facts(&ActionId(LOCATION.into()), &[Some(Value::name("Mary")), None]).unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(carried_by(&store, &Value::name("Mary")), vec![Value::name("apple_1")]);
    }

    #[test]
    fn drop_pins_the_object_where_the_subject_is() {
        let mut can = Can::new();
        let store = Store::open_memory().unwrap();
        apply(&mut can, &store, "pick-up", &["Mary", "apple_1"], &[]);
        apply(&mut can, &store, "pick-up", &["Mary", "milk_1"], &[]);
        apply(&mut can, &store, "move", &["Mary"], &[("to", "hallway_1")]);
        apply(&mut can, &store, "drop", &["Mary", "apple_1"], &[]);
        apply(&mut can, &store, "move", &["Mary"], &[("to", "office_1")]);

        assert_eq!(carried_by(&store, &Value::name("Mary")), vec![Value::name("milk_1")]);
        assert_eq!(location_of(&store, &Value::name("apple_1")), Some(Value::name("hallway_1")));
        assert_eq!(location_of(&store, &Value::name("milk_1")), Some(Value::name("office_1")));
        // Nobody said where the ball is.
        assert_eq!(location_of(&store, &Value::name("ball_1")), None);
    }

    #[test]
    fn leave_only_drops_what_was_carried() {
        let mut can = Can::new();
        let store = Store::open_memory().unwrap();
        apply(&mut can, &store, "move", &["Mary"], &[("to", "garden_1")]);
        apply(&mut can, &store, "leave", &["Mary", "garden_1"], &[]);
        assert_eq!(location_of(&store, &Value::name("garden_1")), None);
    }

    #[test]
    fn give_moves_the_carry_fact() {
        let mut can = Can::new();
        let store = Store::open_memory().unwrap();
        apply(&mut can, &store, "pick-up", &["Mary", "apple_1"], &[]);
        apply(&mut can, &store, "give", &["Mary", "apple_1"], &[("to", "John")]);
        apply(&mut can, &store, "move", &["John"], &[("to", "kitchen_1")]);

        assert_eq!(carried_by(&store, &Value::name("Mary")), Vec::<Value>::new());
        assert_eq!(carried_by(&store, &Value::name("John")), vec![Value::name("apple_1")]);
        assert_eq!(carrier_of(&store, &Value::name("apple_1")), Some(Value::name("John")));
        assert_eq!(location_of(&store, &Value::name("apple_1")), Some(Value::name("kitchen_1")));
    }

    #[test]
    fn negated_and_unknown_verbs_change_nothing() {
        let mut can = Can::new();
        let store = Store::open_memory().unwrap();
        let (mut pred, bindings) = event("move", &["Mary"], &[("to", "garden_1")]);
        pred.negated = true;
        assert!(derive(&mut fw(&mut can, &store), &pred, &bindings, "test", None).unwrap().is_empty());
        apply(&mut can, &store, "own", &["Mary", "dog_1"], &[]);
        assert_eq!(location_of(&store, &Value::name("Mary")), None);
        assert!(carried_by(&store, &Value::name("Mary")).is_empty());
    }
}
