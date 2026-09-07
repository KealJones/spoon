//! Comparatives. `The elephant is bigger than the dog.` relates two entities,
//! so it is stored as `rel.bigger-than(elephant_1, dog_2)` rather than as an
//! attribute of the elephant that forgets the dog. A yes/no question looks for
//! the fact itself, then for a chain: every `-er than` comparison is a strict
//! order, so `bigger-than` composes up to a fixed depth. The reverse chain
//! answers "no" with the fact that contradicts the question.

use std::collections::{HashMap, HashSet, VecDeque};

use spoon_core::store::Store;
use spoon_core::types::*;

use super::answer::Answer;
use super::facts::{ensure_relation, make_fact, resolve_term, FactWriter};
use super::Binding;

/// How many comparisons may be chained.
const DEPTH: usize = 6;

/// `bigger-than`, `smarter-than`: the parser's attribute for `is ADJ than`.
pub fn is_comparative(attr: &str) -> bool {
    attr.ends_with("-than") && attr.len() > 5
}

fn relation(attr: &str) -> ActionId {
    ActionId(format!("rel.{attr}"))
}

/// The two compared entities of a `be` predicate with a comparative attribute.
fn operands(pred: &Pred, bindings: &HashMap<String, Binding>) -> Option<(Value, Value)> {
    let [a, b, ..] = pred.args.as_slice() else { return None };
    Some((resolve_term(a, bindings)?, resolve_term(b, bindings)?))
}

/// Build the relation fact for a comparative assertion.
pub fn fact_for(
    w: &mut FactWriter<'_>,
    pred: &Pred,
    attr: &str,
    bindings: &HashMap<String, Binding>,
    source: &str,
    episode_id: Option<i64>,
) -> Option<(ActionId, Fact)> {
    let (left, right) = operands(pred, bindings)?;
    let id = ensure_relation(w, &relation(attr).0, &[attr], 2, "");
    let fact = make_fact(id.clone(), vec![left, right], pred.negated, &pred.modal, source, episode_id);
    Some((id, fact))
}

/// Answer `Is X bigger than Y?` from the stored comparisons.
pub fn answer(store: &Store, pred: &Pred, attr: &str, bindings: &HashMap<String, Binding>) -> Answer {
    let Some((left, right)) = operands(pred, bindings) else {
        return Answer::Unknown { reason: "unresolved comparison".into() };
    };
    let rel = relation(attr);
    let direct = store.query_facts(&rel, &[Some(left.clone()), Some(right.clone())]).unwrap_or_default();
    if let Some(f) = direct.iter().find(|f| f.truth).or_else(|| direct.first()) {
        return Answer::YesNo(f.truth, Some(f.clone()));
    }
    let conclusion = |a: &Value, b: &Value| make_fact(rel.clone(), vec![a.clone(), b.clone()], false, &None, "derived", None);
    if reaches(store, &rel, &left, &right) {
        return Answer::YesNo(true, Some(conclusion(&left, &right)));
    }
    if reaches(store, &rel, &right, &left) {
        return Answer::YesNo(false, Some(conclusion(&right, &left)));
    }
    Answer::Unknown { reason: format!("no {attr} chain") }
}

/// Is there a chain of true `rel` facts from `from` to `to` within `DEPTH`?
fn reaches(store: &Store, rel: &ActionId, from: &Value, to: &Value) -> bool {
    let mut seen: HashSet<String> = HashSet::new();
    let mut frontier: VecDeque<(Value, usize)> = VecDeque::from([(from.clone(), 0)]);
    while let Some((node, depth)) = frontier.pop_front() {
        if depth >= DEPTH || !seen.insert(node.render()) {
            continue;
        }
        for f in store.query_facts(rel, &[Some(node), None]).unwrap_or_default() {
            let Some(next) = f.args.get(1) else { continue };
            if !f.truth {
                continue;
            }
            if next == to {
                return true;
            }
            frontier.push_back((next.clone(), depth + 1));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use spoon_core::can::Can;

    fn cmp(a: &str, b: &str) -> (Pred, HashMap<String, Binding>) {
        let mut bindings = HashMap::new();
        bindings.insert("x1".to_string(), Binding::Entity(Value::name(a)));
        bindings.insert("x2".to_string(), Binding::Entity(Value::name(b)));
        let pred = Pred {
            pred: "be".into(),
            args: vec![Term::Var { var: "x1".into() }, Term::Var { var: "x2".into() }],
            negated: false,
            modal: None,
            adjuncts: vec![],
            attr: Some("bigger-than".into()),
        };
        (pred, bindings)
    }

    #[test]
    fn chains_and_reverses() {
        let mut can = Can::new();
        let store = Store::open_memory().unwrap();
        for (a, b) in [("elephant_1", "dog_2"), ("dog_2", "cat_3")] {
            let (pred, bindings) = cmp(a, b);
            let mut w = FactWriter { can: &mut can, store: &store };
            let (_, fact) = fact_for(&mut w, &pred, "bigger-than", &bindings, "test", None).unwrap();
            store.insert_fact(&fact).unwrap();
        }
        let (pred, bindings) = cmp("elephant_1", "cat_3");
        match answer(&store, &pred, "bigger-than", &bindings) {
            Answer::YesNo(true, Some(f)) => assert_eq!(f.args, vec![Value::name("elephant_1"), Value::name("cat_3")]),
            other => panic!("expected a derived yes, got {other:?}"),
        }
        let (pred, bindings) = cmp("cat_3", "elephant_1");
        assert!(matches!(answer(&store, &pred, "bigger-than", &bindings), Answer::YesNo(false, Some(_))));
        let (pred, bindings) = cmp("cat_3", "mouse_4");
        assert!(matches!(answer(&store, &pred, "bigger-than", &bindings), Answer::Unknown { .. }));
    }
}
