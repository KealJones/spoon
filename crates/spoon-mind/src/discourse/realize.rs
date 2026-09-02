//! English for stored facts and embedded clauses, built on the SCE realizer.
//! Used when a reply quotes memory ("i know John owns a dog").

use spoon_core::can::Can;
use spoon_core::store::Store;
use spoon_core::types::*;

use super::rules::capitalize_first;

/// The words of an embedded clause ("User thinks that dogs are great." ->
/// "dogs are great"). The parser leaves `sce` empty on sub-clauses, so the
/// user's own words are taken from after "that" in the parent sentence, and
/// the realizer is the last resort.
pub fn sub_clause_text(parent_sce: &str, sub: &Clause) -> String {
    let own = trim_sentence(&sub.sce);
    if !own.is_empty() {
        return own;
    }
    if let Some((_, rest)) = parent_sce.split_once(" that ") {
        let rest = trim_sentence(rest);
        if !rest.is_empty() {
            return rest;
        }
    }
    trim_sentence(&spoon_lang::sce::realize(sub))
}

fn trim_sentence(s: &str) -> String {
    s.trim().trim_end_matches(['.', '!', '?']).trim().to_string()
}

/// How a minted entity reads: "a dog" when memory is quoted cold, "the dog"
/// when the entity was just mentioned (answers and yes/no evidence).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Article {
    Indefinite,
    Definite,
}

/// A stored fact as a short English clause without the final period:
/// `rel.own(John, dog_1)` -> "John owns a dog", `rel.is(dog_1, brown)` ->
/// "a dog is brown", `rel.color(dog_1, brown)` -> "the color of a dog is
/// brown". Typing and count facts (`is_a`, `count`) have nothing to say and
/// give `None`.
pub fn realize_fact(can: &Can, store: &Store, fact: &Fact) -> Option<String> {
    realize_fact_with(can, store, fact, Article::Indefinite)
}

/// `realize_fact` with a choice of article for minted entities:
/// `rel.bigger-than(elephant_1, cat_2)` -> "the elephant is bigger than the cat".
pub fn realize_fact_with(can: &Can, store: &Store, fact: &Fact, article: Article) -> Option<String> {
    let rel = fact.pred.0.strip_prefix("rel.")?;
    if matches!(rel, "is_a" | "count") || fact.args.is_empty() {
        return None;
    }
    let not = if fact.truth { "" } else { "not " };
    let np = |v: &Value| noun_phrase(can, store, v, article);
    if rel == "is" {
        let [subject, attr] = fact.args.as_slice() else { return None };
        return Some(format!("{} is {not}{}", np(subject), attr.render()));
    }
    if let Some(adj) = rel.strip_suffix("-than") {
        let [left, right] = fact.args.as_slice() else { return None };
        return Some(format!("{} is {not}{adj} than {}", np(left), np(right)));
    }
    if rel == "location" {
        let [entity, place] = fact.args.as_slice() else { return None };
        return Some(format!("{} is {not}in {}", np(entity), np(place)));
    }
    if fact.args.iter().all(|a| matches!(a, Value::Name(_))) {
        // A verb relation between entities: let the SCE realizer conjugate.
        let referents: Vec<Referent> = fact
            .args
            .iter()
            .enumerate()
            .map(|(i, a)| referent(can, store, a, &format!("x{i}"), article))
            .collect();
        let clause = Clause {
            act: Act::Assert,
            conditions: vec![Pred {
                pred: rel.to_string(),
                args: (0..fact.args.len()).map(|i| Term::Var { var: format!("x{i}") }).collect(),
                negated: !fact.truth,
                modal: None,
                adjuncts: vec![],
                attr: None,
            }],
            referents,
            then: vec![],
            then_referents: vec![],
            sce: String::new(),
        };
        let s = trim_sentence(&spoon_lang::sce::realize(&clause));
        let starts_with_name = matches!(clause.referents.first().map(|r| &r.quant), Some(Quant::Named(_)));
        return Some(if starts_with_name { s } else { lowercase_first(&s) });
    }
    // A property with a literal value: "the color of a dog is brown".
    let [owner, value] = fact.args.as_slice() else { return None };
    Some(format!("the {rel} of {} is {not}{}", np(owner), value.render()))
}

/// A value as it should be spoken in an answer: minted entities become
/// definite noun phrases (`bathroom_1` -> "the bathroom"), everything else is
/// returned untouched so names and numbers keep their type.
pub fn display_value(can: &Can, store: &Store, value: &Value) -> Value {
    match entity_noun(can, store, value) {
        Some(noun) => Value::Text(format!("the {noun}")),
        None => value.clone(),
    }
}

/// The referent for a fact argument. Minted entities (`dog_1`, which carry an
/// `is_a` fact) become noun phrases; names stay names.
fn referent(can: &Can, store: &Store, value: &Value, var: &str, article: Article) -> Referent {
    let (noun, quant) = match value {
        Value::Name(_) => match entity_noun(can, store, value) {
            Some(noun) => (Some(noun), if article == Article::Definite { Quant::Def } else { Quant::Indef }),
            None => (None, Quant::Named(value.render())),
        },
        v => (None, Quant::Literal(v.clone())),
    };
    Referent { var: var.into(), noun, quant, mods: vec![], owner: None, span: None }
}

fn noun_phrase(can: &Can, store: &Store, value: &Value, article: Article) -> String {
    match (value, entity_noun(can, store, value)) {
        (Value::Name(_), Some(noun)) => {
            let article = match article {
                Article::Definite => "the",
                Article::Indefinite if noun.starts_with(['a', 'e', 'i', 'o', 'u']) => "an",
                Article::Indefinite => "a",
            };
            format!("{article} {noun}")
        }
        (Value::Text(s), _) => format!("\"{s}\""),
        (v, _) => v.render(),
    }
}

/// The noun of a minted entity, from its `is_a` fact: `dog_1` -> "dog".
fn entity_noun(can: &Can, store: &Store, value: &Value) -> Option<String> {
    let Value::Name(name) = value else { return None };
    if matches!(name.as_str(), "User" | "Assistant") {
        return None;
    }
    let is_a = ActionId("rel.is_a".into());
    let concept = store
        .query_facts(&is_a, &[Some(value.clone()), None])
        .ok()?
        .into_iter()
        .find(|f| f.truth)
        .and_then(|f| f.args.get(1).cloned())?;
    let Value::Name(concept) = concept else { return None };
    Some(
        can.concept(&ConceptId(concept.clone()))
            .and_then(|c| c.nouns.first().cloned())
            .unwrap_or_else(|| concept.to_lowercase()),
    )
}

fn lowercase_first(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) => c.to_lowercase().collect::<String>() + cs.as_str(),
        None => String::new(),
    }
}

/// `dog` -> `Dog`, used to read a topic noun as a concept or proper name.
pub fn concept_name(noun: &str) -> String {
    capitalize_first(noun)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(sce: &str) -> Clause {
        Clause {
            act: Act::Assert,
            referents: vec![Referent {
                var: "x2".into(),
                noun: Some("dog".into()),
                quant: Quant::Indef,
                mods: vec![],
                owner: None,
                span: None,
            }],
            conditions: vec![Pred {
                pred: "be".into(),
                args: vec![Term::Var { var: "x2".into() }],
                negated: false,
                modal: None,
                adjuncts: vec![],
                attr: Some("great".into()),
            }],
            then: vec![],
            then_referents: vec![],
            sce: sce.into(),
        }
    }

    #[test]
    fn sub_clause_prefers_the_users_words() {
        assert_eq!(sub_clause_text("User thinks that dogs are great.", &sub("")), "dogs are great");
        assert_eq!(sub_clause_text("User thinks that dogs are great.", &sub("Dogs are great.")), "Dogs are great");
        assert_eq!(sub_clause_text("User thinks something.", &sub("")), "A dog is great");
    }

    #[test]
    fn facts_read_as_english() {
        let mut can = Can::new();
        can.add_concept(Concept::entity("Dog", &["dog"]));
        let store = Store::open_memory().unwrap();
        let fact = |pred: &str, args: Vec<Value>| Fact {
            id: 0,
            pred: ActionId(pred.into()),
            args,
            truth: true,
            modal: None,
            asserted_at: 0,
            invalidated_at: None,
            source: "test".into(),
            episode_id: None,
        };
        store.insert_fact(&fact("rel.is_a", vec![Value::name("dog_1"), Value::name("Dog")])).unwrap();

        let own = fact("rel.own", vec![Value::name("John"), Value::name("dog_1")]);
        assert_eq!(realize_fact(&can, &store, &own), Some("John owns a dog".into()));
        let is = fact("rel.is", vec![Value::name("dog_1"), Value::text("brown")]);
        assert_eq!(realize_fact(&can, &store, &is), Some("a dog is brown".into()));
        let color = fact("rel.color", vec![Value::name("dog_1"), Value::text("brown")]);
        assert_eq!(realize_fact(&can, &store, &color), Some("the color of a dog is brown".into()));
        let typing = fact("rel.is_a", vec![Value::name("dog_1"), Value::name("Dog")]);
        assert_eq!(realize_fact(&can, &store, &typing), None);
    }

    #[test]
    fn definite_facts_and_display_values() {
        let mut can = Can::new();
        can.add_concept(Concept::entity("Elephant", &["elephant"]));
        can.add_concept(Concept::entity("Cat", &["cat"]));
        let store = Store::open_memory().unwrap();
        let fact = |pred: &str, args: Vec<Value>| Fact {
            id: 0,
            pred: ActionId(pred.into()),
            args,
            truth: true,
            modal: None,
            asserted_at: 0,
            invalidated_at: None,
            source: "test".into(),
            episode_id: None,
        };
        store.insert_fact(&fact("rel.is_a", vec![Value::name("elephant_1"), Value::name("Elephant")])).unwrap();
        store.insert_fact(&fact("rel.is_a", vec![Value::name("cat_2"), Value::name("Cat")])).unwrap();

        let bigger = fact("rel.bigger-than", vec![Value::name("elephant_1"), Value::name("cat_2")]);
        assert_eq!(
            realize_fact_with(&can, &store, &bigger, Article::Definite),
            Some("the elephant is bigger than the cat".into())
        );
        let at = fact("rel.location", vec![Value::name("Mary"), Value::name("cat_2")]);
        assert_eq!(realize_fact_with(&can, &store, &at, Article::Definite), Some("Mary is in the cat".into()));
        assert_eq!(display_value(&can, &store, &Value::name("cat_2")), Value::text("the cat"));
        assert_eq!(display_value(&can, &store, &Value::name("Mary")), Value::name("Mary"));
        assert_eq!(display_value(&can, &store, &Value::Int(3)), Value::Int(3));
    }
}
