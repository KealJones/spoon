//! Integration tests for the discourse layer.
//!
//! Builds clauses BY HAND (no parser dependency). Runs all 11 scenarios
//! sequentially with shared DiscourseState so salience accumulates naturally.

use spoon_core::types::*;
use spoon_core::{Can, Store};
use spoon_mind::discourse::*;

// ---------- seeded CAN ----------

fn seeded_can() -> Can {
    let mut can = Can::new();
    let kernel = spoon_core::Provenance::Kernel;
    let tier = spoon_core::Tier::Kernel;

    let mk = |id: &str, nouns: &[&str], extends: &[&str]| Concept {
        id: ConceptId(id.into()),
        kind: ConceptKind::Entity,
        extends: extends.iter().map(|s| ConceptId(s.to_string())).collect(),
        role_of: None,
        nouns: nouns.iter().map(|s| s.to_string()).collect(),
        description: String::new(),
        tier,
        provenance: kernel.clone(),
    };

    can.add_concept(mk("Thing", &["thing"], &[]));
    can.add_concept(mk("Person", &["person"], &["Thing"]));
    can.add_concept(mk("Animal", &["animal"], &["Thing"]));
    can.add_concept(mk("Dog", &["dog"], &["Animal"]));
    can.add_concept(mk("Cat", &["cat"], &["Animal"]));
    can.add_concept(mk("User", &["user"], &["Person"]));
    can.add_concept(mk("Assistant", &["assistant"], &["Thing"]));
    can
}

// ---------- clause builders ----------

fn v(var: &str) -> Term {
    Term::Var { var: var.into() }
}

fn ref_named(var: &str, name: &str) -> Referent {
    Referent {
        var: var.into(),
        noun: None,
        quant: Quant::Named(name.into()),
        mods: vec![],
        owner: None,
        span: None,
    }
}

fn ref_indef(var: &str, noun: &str) -> Referent {
    Referent {
        var: var.into(),
        noun: Some(noun.into()),
        quant: Quant::Indef,
        mods: vec![],
        owner: None,
        span: None,
    }
}

fn ref_def(var: &str, noun: &str) -> Referent {
    Referent {
        var: var.into(),
        noun: Some(noun.into()),
        quant: Quant::Def,
        mods: vec![],
        owner: None,
        span: None,
    }
}

fn ref_wh(var: &str, noun: Option<&str>) -> Referent {
    Referent {
        var: var.into(),
        noun: noun.map(|s| s.into()),
        quant: Quant::Wh,
        mods: vec![],
        owner: None,
        span: None,
    }
}

fn ref_every(var: &str, noun: &str) -> Referent {
    Referent {
        var: var.into(),
        noun: Some(noun.into()),
        quant: Quant::Every,
        mods: vec![],
        owner: None,
        span: None,
    }
}

fn ref_count(var: &str, noun: &str, n: u32) -> Referent {
    Referent {
        var: var.into(),
        noun: Some(noun.into()),
        quant: Quant::Count(n),
        mods: vec![],
        owner: None,
        span: None,
    }
}

fn pred_own(x1: &str, x2: &str, negated: bool) -> Pred {
    Pred {
        pred: "own".into(),
        args: vec![v(x1), v(x2)],
        negated,
        modal: None,
        adjuncts: vec![],
        attr: None,
    }
}

fn pred_be_attr(x1: &str, attr: &str) -> Pred {
    Pred {
        pred: "be".into(),
        args: vec![v(x1)],
        negated: false,
        modal: None,
        adjuncts: vec![],
        attr: Some(attr.into()),
    }
}

fn pred_be_np(x1: &str, x2: &str) -> Pred {
    Pred {
        pred: "be".into(),
        args: vec![v(x1), v(x2)],
        negated: false,
        modal: None,
        adjuncts: vec![],
        attr: None,
    }
}

fn assert_clause(sce: &str, refs: Vec<Referent>, conds: Vec<Pred>) -> Clause {
    Clause {
        act: Act::Assert,
        referents: refs,
        conditions: conds,
        then: vec![],
        then_referents: vec![],
        sce: sce.into(),
    }
}

fn yesno_clause(sce: &str, refs: Vec<Referent>, conds: Vec<Pred>) -> Clause {
    Clause {
        act: Act::Question {
            kind: QuestionKind::YesNo,
        },
        referents: refs,
        conditions: conds,
        then: vec![],
        then_referents: vec![],
        sce: sce.into(),
    }
}

// ---------- single sequential test ----------

#[test]
fn discourse_integration() {
    let store = Store::open_memory().expect("open memory store");
    let mut can = seeded_can();
    let mut state = DiscourseState::default();

    // ================================================================
    // Test 1: "John owns a dog."
    // Ground: John Named, dog Indef -> dog_1 minted; new_entities len 1.
    // Assert: facts own(John, dog_1) and is_a(dog_1, Dog); rel.own created.
    // ================================================================
    let c1 = assert_clause(
        "John owns a dog.",
        vec![ref_named("x1", "John"), ref_indef("x2", "dog")],
        vec![pred_own("x1", "x2", false)],
    );
    let gs1 = ground_all(&mut state, &[c1], &can);
    let g1 = &gs1[0];

    assert_eq!(g1.new_entities.len(), 1, "dog_1 should be the only new entity");
    let dog1_id = if let Value::Name(n) = &g1.new_entities[0].id { n.clone() } else { panic!("not a name") };
    assert_eq!(dog1_id, "dog_1");
    assert!(matches!(g1.bindings["x1"], Binding::Entity(Value::Name(ref n)) if n == "John"));
    assert!(matches!(g1.bindings["x2"], Binding::Entity(Value::Name(ref n)) if n == "dog_1"));

    let out1 = assert_grounded(
        &mut FactWriter { can: &mut can, store: &store },
        g1,
        "user",
        None,
    )
    .expect("assert_grounded test 1");

    if let AssertOutcome::Stored { facts, new_relations: _, .. } = &out1 {
        // own(John, dog_1) and is_a(dog_1, Dog)
        assert!(facts.iter().any(|f| f.pred.0 == "rel.own"), "rel.own fact missing");
        assert!(facts.iter().any(|f| f.pred.0 == "rel.is_a"), "rel.is_a fact missing");
        // rel.own was registered in the CAN.
        let own_id = ActionId("rel.own".into());
        let act = can.action(&own_id).expect("rel.own should exist in CAN");
        assert_eq!(act.role, Role::Relation, "rel.own role should be Relation");
    } else {
        panic!("Expected Stored, got {:?}", out1);
    }

    // ================================================================
    // Test 2: "Who owns a dog?" -> Answer::Values([Name "John"])
    // ================================================================
    let c2 = Clause {
        act: Act::Question { kind: QuestionKind::Who { focus: "x1".into() } },
        referents: vec![ref_wh("x1", None), ref_indef("x2", "dog")],
        conditions: vec![pred_own("x1", "x2", false)],
        then: vec![],
        then_referents: vec![],
        sce: "Who owns a dog?".into(),
    };
    let gs2 = ground_all(&mut state, &[c2], &can);
    let ans2 = answer_grounded(
        &can,
        &store,
        &gs2[0],
        &QuestionKind::Who { focus: "x1".into() },
    )
    .expect("answer test 2");
    if let Answer::Values(vals) = &ans2 {
        assert!(
            vals.iter().any(|v| matches!(v, Value::Name(n) if n == "John")),
            "Expected John in answer, got {:?}",
            vals
        );
    } else {
        panic!("Expected Values, got {:?}", ans2);
    }

    // ================================================================
    // Test 3: "Does John own a dog?" -> YesNo(true)
    //         "Does Mary own a dog?" -> Unknown
    // ================================================================
    let c3a = yesno_clause(
        "Does John own a dog?",
        vec![ref_named("x1", "John"), ref_indef("x2", "dog")],
        vec![pred_own("x1", "x2", false)],
    );
    let gs3a = ground_all(&mut state, &[c3a], &can);
    let ans3a = answer_grounded(&can, &store, &gs3a[0], &QuestionKind::YesNo)
        .expect("answer test 3a");
    assert!(
        matches!(ans3a, Answer::YesNo(true, _)),
        "Expected YesNo(true), got {:?}",
        ans3a
    );

    let c3b = yesno_clause(
        "Does Mary own a dog?",
        vec![ref_named("x1", "Mary"), ref_indef("x2", "dog")],
        vec![pred_own("x1", "x2", false)],
    );
    let gs3b = ground_all(&mut state, &[c3b], &can);
    let ans3b = answer_grounded(&can, &store, &gs3b[0], &QuestionKind::YesNo)
        .expect("answer test 3b");
    assert!(
        matches!(ans3b, Answer::Unknown { .. }),
        "Expected Unknown, got {:?}",
        ans3b
    );

    // ================================================================
    // Test 4: "The dog is brown." -> Def resolves to dog_1; no new entity.
    //         "Is the dog brown?" -> YesNo(true)
    // ================================================================
    let c4 = assert_clause(
        "The dog is brown.",
        vec![ref_def("x1", "dog")],
        vec![pred_be_attr("x1", "brown")],
    );
    let gs4 = ground_all(&mut state, &[c4], &can);
    assert_eq!(gs4[0].new_entities.len(), 0, "The dog should resolve to dog_1 (no new entity)");
    assert!(
        matches!(gs4[0].bindings["x1"], Binding::Entity(Value::Name(ref n)) if n == "dog_1"),
        "Def should resolve to dog_1, got {:?}",
        gs4[0].bindings["x1"]
    );
    assert_grounded(
        &mut FactWriter { can: &mut can, store: &store },
        &gs4[0],
        "user",
        None,
    )
    .expect("assert test 4");

    let c4q = yesno_clause(
        "Is the dog brown?",
        vec![ref_def("x1", "dog")],
        vec![pred_be_attr("x1", "brown")],
    );
    let gs4q = ground_all(&mut state, &[c4q], &can);
    let ans4 = answer_grounded(&can, &store, &gs4q[0], &QuestionKind::YesNo)
        .expect("answer test 4");
    assert!(
        matches!(ans4, Answer::YesNo(true, _)),
        "Expected YesNo(true) for 'is the dog brown', got {:?}",
        ans4
    );

    // ================================================================
    // Test 5: "Ben owns 10 apples." -> Count(10)
    // ================================================================
    let c5 = assert_clause(
        "Ben owns 10 apples.",
        vec![ref_named("x1", "Ben"), ref_count("x2", "apple", 10)],
        vec![pred_own("x1", "x2", false)],
    );
    let gs5 = ground_all(&mut state, &[c5], &can);
    // apple_1 minted with count=10 mod.
    let apple = &gs5[0].new_entities[0];
    assert!(
        apple.mods.iter().any(|m| m == "count=10"),
        "apple entity should have count=10 mod"
    );
    assert_grounded(
        &mut FactWriter { can: &mut can, store: &store },
        &gs5[0],
        "user",
        None,
    )
    .expect("assert test 5");

    let c5q = Clause {
        act: Act::Question { kind: QuestionKind::HowMany { focus: "x2".into() } },
        referents: vec![ref_named("x1", "Ben"), ref_wh("x2", Some("apple"))],
        conditions: vec![pred_own("x1", "x2", false)],
        then: vec![],
        then_referents: vec![],
        sce: "How many apples does Ben own?".into(),
    };
    let gs5q = ground_all(&mut state, &[c5q], &can);
    let ans5 = answer_grounded(
        &can,
        &store,
        &gs5q[0],
        &QuestionKind::HowMany { focus: "x2".into() },
    )
    .expect("answer test 5");
    assert!(
        matches!(ans5, Answer::Count(10)),
        "Expected Count(10), got {:?}",
        ans5
    );

    // ================================================================
    // Test 6: "Every dog is an animal." -> Universal; rule stored.
    //         "Is dog_1 an animal?" -> YesNo(true) via forward chain.
    // ================================================================
    let c6 = assert_clause(
        "Every dog is an animal.",
        vec![ref_every("x1", "dog"), ref_indef("x2", "animal")],
        vec![pred_be_np("x1", "x2")],
    );
    let gs6 = ground_all(&mut state, &[c6], &can);
    let out6 = assert_grounded(
        &mut FactWriter { can: &mut can, store: &store },
        &gs6[0],
        "user",
        None,
    )
    .expect("assert test 6");
    assert!(
        matches!(out6, AssertOutcome::Universal { .. }),
        "Expected Universal, got {:?}",
        out6
    );

    // "Is dog_1 an animal?" - build clause with Named("dog_1")
    let c6q = yesno_clause(
        "Is dog_1 an animal?",
        vec![
            Referent {
                var: "x1".into(),
                noun: None,
                quant: Quant::Named("dog_1".into()),
                mods: vec![],
                owner: None,
                span: None,
            },
            ref_indef("x2", "animal"),
        ],
        vec![pred_be_np("x1", "x2")],
    );
    let gs6q = ground_all(&mut state, &[c6q], &can);
    let ans6 = answer_grounded(&can, &store, &gs6q[0], &QuestionKind::YesNo)
        .expect("answer test 6");
    assert!(
        matches!(ans6, Answer::YesNo(true, _)),
        "Expected YesNo(true) for 'Is dog_1 an animal?', got {:?}",
        ans6
    );

    // ================================================================
    // Test 7: "John does not own a cat." -> YesNo(false) on query.
    // ================================================================
    let c7 = assert_clause(
        "John does not own a cat.",
        vec![ref_named("x1", "John"), ref_indef("x2", "cat")],
        vec![pred_own("x1", "x2", true)],
    );
    let gs7 = ground_all(&mut state, &[c7], &can);
    let cat1_name = if let Binding::Entity(Value::Name(n)) = &gs7[0].bindings["x2"] {
        n.clone()
    } else {
        panic!("cat entity not bound")
    };
    assert_grounded(
        &mut FactWriter { can: &mut can, store: &store },
        &gs7[0],
        "user",
        None,
    )
    .expect("assert test 7");

    let c7q = yesno_clause(
        "Does John own a cat?",
        vec![ref_named("x1", "John"), ref_indef("x2", "cat")],
        vec![pred_own("x1", "x2", false)],
    );
    let gs7q = ground_all(&mut state, &[c7q], &can);
    let ans7 = answer_grounded(&can, &store, &gs7q[0], &QuestionKind::YesNo)
        .expect("answer test 7");
    assert!(
        matches!(ans7, Answer::YesNo(false, _)),
        "Expected YesNo(false), got {:?}",
        ans7
    );

    // ================================================================
    // Test 8: Contradiction: assert own(John, dog_1) false after true.
    //         supersede -> query shows false.
    // ================================================================
    let c8 = assert_clause(
        "John does not own dog_1.",
        vec![
            ref_named("x1", "John"),
            Referent {
                var: "x2".into(),
                noun: None,
                quant: Quant::Named("dog_1".into()),
                mods: vec![],
                owner: None,
                span: None,
            },
        ],
        vec![pred_own("x1", "x2", true)],
    );
    let gs8 = ground_all(&mut state, &[c8], &can);
    let out8 = assert_grounded(
        &mut FactWriter { can: &mut can, store: &store },
        &gs8[0],
        "user",
        None,
    )
    .expect("assert test 8");
    let (ex_id, incoming_fact) = match out8 {
        AssertOutcome::Contradiction { existing, incoming } => {
            assert!(existing.truth, "existing should be truth=true");
            assert!(!incoming.truth, "incoming should be truth=false");
            (existing.id, incoming)
        }
        other => panic!("Expected Contradiction, got {:?}", other),
    };

    supersede(&store, ex_id, &incoming_fact).expect("supersede");

    // After supersede: query own(John, dog_1) -> false.
    let c8q = yesno_clause(
        "Does John own dog_1?",
        vec![
            ref_named("x1", "John"),
            Referent {
                var: "x2".into(),
                noun: None,
                quant: Quant::Named("dog_1".into()),
                mods: vec![],
                owner: None,
                span: None,
            },
        ],
        vec![pred_own("x1", "x2", false)],
    );
    let gs8q = ground_all(&mut state, &[c8q], &can);
    let ans8 = answer_grounded(&can, &store, &gs8q[0], &QuestionKind::YesNo)
        .expect("answer test 8");
    assert!(
        matches!(ans8, Answer::YesNo(false, _)),
        "After supersede, expected YesNo(false), got {:?}",
        ans8
    );

    // ================================================================
    // Test 9: Correction detection table (10 utterances).
    // ================================================================
    let cases: &[(&str, Option<Correction>)] = &[
        ("no i meant the other file", Some(Correction::Meant { text: "the other file".into() })),
        ("nvm", Some(Correction::Undo)),
        ("yep", Some(Correction::Confirm)),
        ("triple means multiply by three", Some(Correction::Synonym { word: "triple".into(), means: "multiply by three".into() })),
        ("wrong", Some(Correction::Wrong)),
        ("yes", Some(Correction::Confirm)),
        ("no", Some(Correction::Wrong)),
        ("forget that", Some(Correction::Undo)),
        ("when i say foo i mean bar", Some(Correction::Synonym { word: "foo".into(), means: "bar".into() })),
        ("that's wrong", Some(Correction::Wrong)),
    ];
    for (text, expected) in cases {
        let got = detect_correction(text);
        assert_eq!(
            got.as_ref().map(|c| std::mem::discriminant(c)),
            expected.as_ref().map(|c| std::mem::discriminant(c)),
            "detect_correction({:?}): expected {:?}, got {:?}",
            text,
            expected,
            got
        );
        // For Meant and Synonym, also check the inner text.
        if let (Some(Correction::Meant { text: exp_text }), Some(Correction::Meant { text: got_text })) = (expected, &got) {
            assert_eq!(got_text, exp_text, "Meant text mismatch for '{}'", text);
        }
        if let (Some(Correction::Synonym { word: ew, means: em }), Some(Correction::Synonym { word: gw, means: gm })) = (expected, &got) {
            assert_eq!(gw, ew, "Synonym word mismatch for '{}'", text);
            assert_eq!(gm, em, "Synonym means mismatch for '{}'", text);
        }
    }

    // ================================================================
    // Test 10: Keywords extraction - non-empty and lowercase.
    // ================================================================
    let kw_clause = assert_clause(
        "John owns a dog.",
        vec![ref_named("x1", "John"), ref_indef("x2", "dog")],
        vec![pred_own("x1", "x2", false)],
    );
    let kws = extract_keywords(&[kw_clause], "John owns a dog.");
    assert!(!kws.is_empty(), "keywords should be non-empty");
    for kw in &kws {
        assert_eq!(
            kw.to_lowercase(),
            kw.as_str(),
            "keyword '{}' should be lowercase",
            kw
        );
    }
    assert!(kws.iter().any(|k| k == "john"), "should contain 'john'");
    assert!(kws.iter().any(|k| k == "dog"), "should contain 'dog'");

    // ================================================================
    // Test 11: Salience - "the animal" resolves to most recent (cat_1).
    //
    // Refresh cat_1's salience after test 8 re-touched dog_1, to ensure
    // cat_1 is definitively the most recently mentioned animal entity.
    // ================================================================
    let c11_setup = assert_clause(
        "The cat is present.",
        vec![Referent {
            var: "x1".into(),
            noun: None,
            quant: Quant::Named(cat1_name.clone()),
            mods: vec![],
            owner: None,
            span: None,
        }],
        vec![pred_be_attr("x1", "present")],
    );
    ground_all(&mut state, &[c11_setup], &can); // bumps cat_1.last_mentioned past dog_1
    let dog1_entity = state.entities.iter().find(|e| e.id == Value::Name("dog_1".into()));
    let cat_entity = state
        .entities
        .iter()
        .find(|e| e.id == Value::Name(cat1_name.clone()));
    let dog1_turn = dog1_entity.map(|e| e.last_mentioned).unwrap_or(0);
    let cat1_turn = cat_entity.map(|e| e.last_mentioned).unwrap_or(0);
    assert!(
        cat1_turn > dog1_turn,
        "cat_1 (turn {}) should be more recent than dog_1 (turn {})",
        cat1_turn,
        dog1_turn
    );

    let c11 = yesno_clause(
        "Is the animal present?",
        vec![ref_def("x1", "animal")],
        vec![pred_be_attr("x1", "present")],
    );
    let gs11 = ground_all(&mut state, &[c11], &can);
    let resolved = &gs11[0].bindings["x1"];
    assert!(
        matches!(resolved, Binding::Entity(Value::Name(n)) if n == &cat1_name),
        "Expected 'the animal' to resolve to {} (most recent), got {:?}",
        cat1_name,
        resolved
    );
}
