//! The meta-vocabulary: rules that make relational properties assertable.
//!
//! Without these, every relation needs its own hand-written rule, and saying
//! that friendship is symmetric means writing a rule about friendship. With
//! them, `Symmetric<FriendWith>` is a fact a user can simply assert, and the
//! single general rule below does the rest for every relation at once.
//!
//! Each rule quantifies over the relation by putting a hole in head position:
//! `?0<?2, ?1>` concludes about whatever relation `?0` turns out to be. That is
//! the whole reason a hole is allowed to be a head. None of these rules is
//! privileged; they are ordinary stored realizations, and a brain can retire or
//! replace any of them.

use chrono::Utc;
use spoon_concept::{
    Activation, Concept, Effect, Provenance, Realization, RealizationSpec, RuleDirection, Tier,
};
use spoon_store::Store;

use crate::error::Result;

fn h(n: u32) -> Concept {
    Concept::hole(n)
}

fn rule(
    name: &str,
    target: &str,
    pattern: Concept,
    condition: Option<Concept>,
    produce: Concept,
) -> Realization {
    Realization {
        target: Concept::named(target),
        name: name.into(),
        spec: RealizationSpec::Rule {
            pattern,
            condition,
            produce,
            // Every rule here relates a relation to itself or to a close
            // cousin, so forward application would rewrite a stored fact into
            // its own consequence and bounce.
            direction: RuleDirection::Backward,
        },
        effect: Effect::Read,
        activation: Activation::new(Utc::now()),
        provenance: Provenance::Bootstrap,
        tier: Tier::Kernel,
    }
}

/// Conjunction of subgoals, understood by the derivation engine.
fn all_of(conjuncts: impl IntoIterator<Item = Concept>) -> Concept {
    Concept::call("all-of", conjuncts)
}

/// The general rules behind the meta-vocabulary.
///
/// Names are stable because they are the rule's identity for evidence. Renaming
/// one silently orphans everything Spoon had learned about how well it works.
pub fn meta_rules() -> Vec<Realization> {
    vec![
        // Symmetric<R>: R<x, y> gives R<y, x>.
        rule(
            "meta-symmetric",
            "symmetric",
            Concept::apply(h(0), vec![h(1), h(2)]),
            Some(Concept::call("symmetric", [h(0)])),
            Concept::apply(h(0), vec![h(2), h(1)]),
        ),
        // InverseOf<R1, R2>: R1<x, y> gives R2<y, x>.
        //
        // Two rules rather than one, because the relation between R1 and R2 is
        // itself symmetric and a user should not have to assert it twice.
        // Expressing that as `Symmetric<InverseOf>` would be neater but would
        // make the meta-vocabulary depend on itself in a way that is harder to
        // reason about when something goes wrong.
        rule(
            "meta-inverse-of-forward",
            "inverse-of",
            all_of([
                Concept::call("inverse-of", [h(3), h(0)]),
                Concept::apply(h(3), vec![h(1), h(2)]),
            ]),
            None,
            Concept::apply(h(0), vec![h(2), h(1)]),
        ),
        rule(
            "meta-inverse-of-backward",
            "inverse-of",
            all_of([
                Concept::call("inverse-of", [h(0), h(3)]),
                Concept::apply(h(3), vec![h(1), h(2)]),
            ]),
            None,
            Concept::apply(h(0), vec![h(2), h(1)]),
        ),
        // Transitive<R>: R<x, y> and R<y, z> give R<x, z>.
        //
        // The shared hole in the middle is what makes this a real join: the
        // same `?3` has to satisfy both premises, so the engine cannot solve
        // them independently and paste the answers together.
        rule(
            "meta-transitive",
            "transitive",
            all_of([
                Concept::apply(h(0), vec![h(1), h(3)]),
                Concept::apply(h(0), vec![h(3), h(2)]),
            ]),
            Some(Concept::call("transitive", [h(0)])),
            Concept::apply(h(0), vec![h(1), h(2)]),
        ),
        // SubtypeOf<A, B> with Participates<x, A> gives Participates<x, B>.
        //
        // This is is-a without a privileged is-a edge. Nothing in the substrate
        // knows what a type is; `SubtypeOf` is a concept like any other, and
        // this rule is what gives it consequences.
        rule(
            "meta-subtype-participation",
            "participates",
            // Conjunct order matters here, and not for style. Asking about
            // participation first restates the very goal being solved, so the
            // cycle cut fires and the branch dies. Establishing the subtype
            // link first is cheap, does not recurse, and binds `?2`, which
            // turns the participation premise into a closed question about a
            // known type.
            all_of([
                Concept::call("subtype-of", [h(2), h(3)]),
                Concept::call("participates", [h(1), h(2)]),
            ]),
            None,
            Concept::call("participates", [h(1), h(3)]),
        ),
        // SubtypeOf is transitive in its own right, so a chain of subtypes
        // reaches all the way up without a rule per level.
        rule(
            "meta-subtype-transitive",
            "subtype-of",
            all_of([
                Concept::call("subtype-of", [h(1), h(3)]),
                Concept::call("subtype-of", [h(3), h(2)]),
            ]),
            None,
            Concept::call("subtype-of", [h(1), h(2)]),
        ),
        // Synonym<surface, concept>: anything true of the concept is true of
        // what the surface form denotes.
        //
        // Deliberately narrow. It resolves a name to a concept and stops there,
        // rather than claiming the two are interchangeable everywhere. Two
        // words can be synonyms without every use of one referring to the same
        // thing, which is why SameReferent, SynonymOf and EquivalentTo are
        // different concepts.
        rule(
            "meta-synonym-resolves",
            "denotes",
            Concept::call("synonym", [h(1), h(2)]),
            None,
            Concept::call("denotes", [h(1), h(2)]),
        ),
        // DefaultExpectation<Type, Prop, Value>: participating in Type brings
        // an expectation, which specific evidence defeats.
        //
        // The world is full of exceptions, so participation gives expectations
        // rather than enforcing ontology. `Unless<Known<Prop, x>>` is negation
        // as failure: the default holds while Spoon has learned nothing
        // specific, and stops the moment it has.
        rule(
            "meta-default-expectation",
            "default-expectation",
            // The expectation is looked up first for the same reason: it is a
            // direct fact, and binding the type turns the participation premise
            // into a closed question rather than an open one that recurses.
            all_of([
                Concept::call("default-expectation", [h(2), h(3), h(4)]),
                Concept::call("participates", [h(1), h(2)]),
            ]),
            Some(Concept::call(
                "unless",
                [Concept::call("known", [h(3), h(1)])],
            )),
            Concept::apply(h(3), vec![h(1), h(4)]),
        ),
    ]
}

/// Store the meta-vocabulary. Idempotent, so re-seeding an existing brain
/// refreshes these without disturbing anything learned since.
pub fn seed_meta_rules(store: &Store) -> Result<usize> {
    let rules = meta_rules();
    for realization in &rules {
        store.put_realization(realization)?;
    }
    for name in [
        "symmetric",
        "inverse-of",
        "transitive",
        "subtype-of",
        "participates",
        "synonym",
        "denotes",
        "default-expectation",
        "known",
        "unless",
        "all-of",
    ] {
        store.register_symbol(name)?;
    }
    Ok(rules.len())
}
