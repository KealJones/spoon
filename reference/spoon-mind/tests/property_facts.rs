//! "The name of Assistant is Spoon." must be stored as rel.name(Assistant, Spoon)
//! and be answerable by "What is the name of Assistant?" and
//! "Is the wellbeing of Assistant good?". The subject is a possessed noun
//! (Referent.owner), which is how the SCE parser represents "the X of Y".

use spoon_core::can::Can;
use spoon_core::kernel::Kernel;
use spoon_core::store::Store;
use spoon_core::types::*;
use spoon_mind::discourse::{
    answer_grounded, assert_grounded, ground_all, Answer, DiscourseState, FactWriter,
};

fn named(var: &str, name: &str) -> Referent {
    Referent {
        var: var.into(),
        noun: None,
        quant: Quant::Named(name.into()),
        mods: vec![],
        owner: None,
        span: None,
    }
}

fn possessed(var: &str, noun: &str, owner: &str) -> Referent {
    Referent {
        var: var.into(),
        noun: Some(noun.into()),
        quant: Quant::Def,
        mods: vec![],
        owner: Some(owner.into()),
        span: None,
    }
}

fn wh(var: &str) -> Referent {
    Referent { var: var.into(), noun: None, quant: Quant::Wh, mods: vec![], owner: None, span: None }
}

fn var(v: &str) -> Term {
    Term::Var { var: v.into() }
}

fn be(args: Vec<Term>, attr: Option<&str>) -> Pred {
    Pred {
        pred: "be".into(),
        args,
        negated: false,
        modal: None,
        adjuncts: vec![],
        attr: attr.map(str::to_string),
    }
}

fn clause(act: Act, referents: Vec<Referent>, conditions: Vec<Pred>, sce: &str) -> Clause {
    Clause { act, referents, conditions, then: vec![], then_referents: vec![], sce: sce.into() }
}

fn seeded() -> (Can, Kernel, Store) {
    let kernel = Kernel::new();
    let mut can = Can::new();
    for c in kernel.concepts() {
        can.add_concept(c.clone());
    }
    for a in kernel.actions() {
        can.add_action(a.clone());
    }
    (can, kernel, Store::open_memory().unwrap())
}

#[test]
fn property_fact_roundtrip() {
    let (mut can, _kernel, store) = seeded();
    let mut state = DiscourseState::default();

    // The name of Assistant is Spoon.
    let c1 = clause(
        Act::Assert,
        vec![named("x0", "Assistant"), possessed("x1", "name", "x0"), named("x2", "Spoon")],
        vec![be(vec![var("x1"), var("x2")], None)],
        "The name of Assistant is Spoon.",
    );
    // The wellbeing of Assistant is good.
    let c2 = clause(
        Act::Assert,
        vec![named("x0", "Assistant"), possessed("x1", "wellbeing", "x0")],
        vec![be(vec![var("x1")], Some("good"))],
        "The wellbeing of Assistant is good.",
    );
    for g in ground_all(&mut state, &[c1, c2], &can) {
        let mut w = FactWriter { can: &mut can, store: &store };
        let out = assert_grounded(&mut w, &g, "seed", None).unwrap();
        assert!(matches!(out, spoon_mind::discourse::AssertOutcome::Stored { .. }), "{out:?}");
    }
    let name_facts = store.query_facts(&ActionId("rel.name".into()), &[Some(Value::name("Assistant")), None]).unwrap();
    assert_eq!(name_facts.len(), 1, "expected rel.name(Assistant, Spoon), got {name_facts:?}");
    assert_eq!(name_facts[0].args[1], Value::name("Spoon"));

    // What is the name of Assistant?
    let q1 = clause(
        Act::Question { kind: QuestionKind::What { focus: "x9".into() } },
        vec![wh("x9"), named("x0", "Assistant"), possessed("x1", "name", "x0")],
        vec![be(vec![var("x9"), var("x1")], None)],
        "What is the name of Assistant?",
    );
    let g = ground_all(&mut state, &[q1], &can).remove(0);
    match answer_grounded(&can, &store, &g, &QuestionKind::What { focus: "x9".into() }).unwrap() {
        Answer::Values(vs) => assert_eq!(vs, vec![Value::name("Spoon")]),
        other => panic!("expected Values, got {other:?}"),
    }

    // Is the wellbeing of Assistant good? / bad?
    for (attr, expect) in [("good", true), ("bad", false)] {
        let q = clause(
            Act::Question { kind: QuestionKind::YesNo },
            vec![named("x0", "Assistant"), possessed("x1", "wellbeing", "x0")],
            vec![be(vec![var("x1")], Some(attr))],
            "Is the wellbeing of Assistant good?",
        );
        let g = ground_all(&mut state, &[q], &can).remove(0);
        match answer_grounded(&can, &store, &g, &QuestionKind::YesNo).unwrap() {
            Answer::YesNo(b, _) => assert_eq!(b, expect, "attr {attr}"),
            other => panic!("expected YesNo, got {other:?}"),
        }
    }
}
