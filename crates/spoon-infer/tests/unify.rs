//! Contract tests for unification. These encode properties the chaining engine
//! will assume, so a failure here is a design problem, not a code problem.

use spoon_concept::{Concept, HoleId};
use spoon_infer::{Substitution, unify_terms};

fn h(n: u32) -> Concept {
    Concept::hole(n)
}

#[test]
fn identical_ground_terms_unify_with_nothing_bound() {
    let t = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    let subst = unify_terms(&t, &t).expect("a term unifies with itself");
    assert!(subst.is_empty());
}

#[test]
fn different_atoms_do_not_unify() {
    assert!(unify_terms(&Concept::named("greg"), &Concept::named("keal")).is_none());
    assert!(unify_terms(&Concept::int(1), &Concept::int(2)).is_none());
    // 42 and 42.0 are different concepts, and unification must not paper over
    // that any more than equality does.
    assert!(unify_terms(&Concept::int(42), &Concept::float(42.0)).is_none());
}

#[test]
fn a_hole_binds_to_anything_on_either_side() {
    let greg = Concept::named("greg");
    let subst = unify_terms(&h(0), &greg).unwrap();
    assert_eq!(subst.apply(&h(0)), greg);

    // Unification is symmetric, unlike one-way matching.
    let subst = unify_terms(&greg, &h(0)).unwrap();
    assert_eq!(subst.apply(&h(0)), greg);
}

#[test]
fn both_sides_can_bind_at_once() {
    // This is the case one-way matching cannot do: a goal with its own holes
    // meeting a rule conclusion with its own.
    let goal = Concept::call("owns", [Concept::named("greg"), h(0)]);
    let conclusion = Concept::call("owns", [h(1), Concept::named("dog")]);
    let subst = unify_terms(&goal, &conclusion).expect("both sides should bind");
    assert_eq!(subst.apply(&h(0)), Concept::named("dog"));
    assert_eq!(subst.apply(&h(1)), Concept::named("greg"));
    assert_eq!(subst.apply(&goal), subst.apply(&conclusion));
}

#[test]
fn a_repeated_hole_forces_its_occurrences_to_agree() {
    let pattern = Concept::call("same", [h(0), h(0)]);
    let agrees = Concept::call("same", [Concept::named("greg"), Concept::named("greg")]);
    let differs = Concept::call("same", [Concept::named("greg"), Concept::named("keal")]);
    assert!(unify_terms(&pattern, &agrees).is_some());
    assert!(unify_terms(&pattern, &differs).is_none());
}

#[test]
fn bindings_chain_and_resolve_to_the_end() {
    // ?0 = ?1, then ?1 = Greg. Reading ?0 has to follow the chain, or a
    // derivation reports an unbound variable it has actually solved.
    let mut subst = Substitution::new();
    assert!(subst.bind(HoleId(0), h(1)));
    assert!(subst.bind(HoleId(1), Concept::named("greg")));
    assert_eq!(subst.apply(&h(0)), Concept::named("greg"));
    assert_eq!(
        subst.apply(&Concept::call("f", [h(0)])),
        Concept::call("f", [Concept::named("greg")])
    );
}

#[test]
fn the_occurs_check_refuses_an_infinite_term() {
    // Binding ?0 to f<?0> makes a term that contains itself. Printing, hashing
    // or walking it never terminates. Prolog skips this check for speed and
    // documents the unsoundness; Spoon is not fast enough for that trade.
    let mut subst = Substitution::new();
    assert!(!subst.bind(HoleId(0), Concept::call("f", [h(0)])));
    assert!(subst.is_empty(), "a refused binding must not be recorded");

    assert!(unify_terms(&h(0), &Concept::call("f", [h(0)])).is_none());
    assert!(unify_terms(&Concept::call("f", [h(0)]), &h(0)).is_none());
}

#[test]
fn the_occurs_check_follows_chains() {
    // ?0 = ?1 already, so binding ?1 to f<?0> is just as circular.
    let mut subst = Substitution::new();
    assert!(subst.bind(HoleId(0), h(1)));
    assert!(!subst.bind(HoleId(1), Concept::call("f", [h(0)])));
}

#[test]
fn a_hole_unifies_with_itself_without_binding_anything() {
    let subst = unify_terms(&h(0), &h(0)).expect("a hole unifies with itself");
    assert_eq!(subst.apply(&h(0)), h(0));
}

#[test]
fn arity_and_order_both_matter() {
    let two = Concept::call("f", [h(0), h(1)]);
    let three = Concept::call("f", [h(0), h(1), h(2)]);
    assert!(unify_terms(&two, &three).is_none());

    let ab = Concept::call("f", [Concept::named("a"), Concept::named("b")]);
    let ba = Concept::call("f", [Concept::named("b"), Concept::named("a")]);
    assert!(unify_terms(&ab, &ba).is_none());
}

#[test]
fn a_hole_in_head_position_unifies_with_a_relation() {
    // ?0<?1, ?2> is how a rule quantifies over relations, which is what
    // Symmetric<R> needs.
    let quantified = Concept::apply(h(0), vec![h(1), h(2)]);
    let concrete = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    let subst = unify_terms(&quantified, &concrete).expect("should quantify over the relation");
    assert_eq!(subst.apply(&h(0)), Concept::named("friend-with"));
    assert_eq!(subst.apply(&h(1)), Concept::named("greg"));
    assert_eq!(subst.apply(&quantified), concrete);
}

#[test]
fn applying_a_substitution_makes_both_sides_identical() {
    // The law the chaining engine depends on: after a successful unification,
    // the two terms are the same term.
    let cases = [
        (
            Concept::call("owns", [h(0), Concept::named("dog")]),
            Concept::call("owns", [Concept::named("greg"), h(1)]),
        ),
        (
            Concept::apply(h(0), vec![h(1)]),
            Concept::call("is-sad", [Concept::named("greg")]),
        ),
        (
            Concept::call("f", [Concept::call("g", [h(0)]), h(0)]),
            Concept::call(
                "f",
                [Concept::call("g", [Concept::int(1)]), Concept::int(1)],
            ),
        ),
    ];
    for (left, right) in cases {
        let subst = unify_terms(&left, &right)
            .unwrap_or_else(|| panic!("expected {left:?} and {right:?} to unify"));
        assert_eq!(subst.apply(&left), subst.apply(&right));
    }
}

#[test]
fn nested_structure_unifies_through_depth() {
    let goal = Concept::call(
        "stated",
        [Concept::named("keal"), Concept::call("is-sad", [h(0)])],
    );
    let fact = Concept::call(
        "stated",
        [
            Concept::named("keal"),
            Concept::call("is-sad", [Concept::named("greg")]),
        ],
    );
    let subst = unify_terms(&goal, &fact).unwrap();
    assert_eq!(subst.apply(&h(0)), Concept::named("greg"));
}
