//! Integration tests for the dispatch layer.
//! All clauses are constructed directly (no SCE parser) per instructions.

use spoon_core::can::Can;
use spoon_core::kernel::{Kernel, NoHost};
use spoon_core::store::Store;
use spoon_core::types::*;

use spoon_mind::discourse::{ground_all, DiscourseState};
use spoon_mind::dispatch::{dispatch, dispatch_turn, DispatchCtx, Dispatched};

// ---- helpers ---------------------------------------------------------------

fn make_can(kernel: &Kernel) -> Can {
    let mut can = Can::new();
    for a in kernel.actions() {
        can.add_action(a.clone());
    }
    for c in kernel.concepts() {
        can.add_concept(c.clone());
    }
    can
}

fn make_ctx<'a>(
    can: &'a mut Can,
    store: &'a Store,
    kernel: &'a Kernel,
    returning: bool,
) -> DispatchCtx<'a> {
    DispatchCtx {
        can,
        store,
        kernel,
        host: &NoHost,
        session_id: "test",
        episode_id: None,
        returning_user: returning,
    }
}

/// Build a minimal Assert clause.
fn assert_clause(pred: &str, args: Vec<(&str, Quant)>, sce: &str) -> Clause {
    let referents: Vec<Referent> = args.iter().map(|(var, quant)| {
        let noun = match quant {
            Quant::Named(_) => None,
            Quant::Indef | Quant::Def => Some(var.to_string()),
            _ => None,
        };
        Referent { var: var.to_string(), noun, quant: quant.clone(), mods: vec![], owner: None, span: None }
    }).collect();
    let terms: Vec<Term> = args.iter().map(|(var, _)| Term::Var { var: var.to_string() }).collect();
    Clause {
        act: Act::Assert,
        referents,
        conditions: vec![Pred {
            pred: pred.to_string(),
            args: terms,
            negated: false,
            modal: None,
            adjuncts: vec![],
            attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: sce.to_string(),
    }
}

/// Build an attr assert clause (copula "be" with attribute).
fn attr_assert_clause(subject_var: &str, subject_quant: Quant, attr: &str, sce: &str) -> Clause {
    Clause {
        act: Act::Assert,
        referents: vec![
            Referent { var: subject_var.to_string(), noun: None, quant: subject_quant, mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "be".to_string(),
            args: vec![Term::Var { var: subject_var.to_string() }],
            negated: false,
            modal: None,
            adjuncts: vec![],
            attr: Some(attr.to_string()),
        }],
        then: vec![],
        then_referents: vec![],
        sce: sce.to_string(),
    }
}

/// Build a be-NP assert clause (copula "be" with a noun object).
fn be_np_clause(subj_var: &str, subj_quant: Quant, obj_var: &str, obj_quant: Quant, negated: bool, sce: &str) -> Clause {
    Clause {
        act: Act::Assert,
        referents: vec![
            Referent { var: subj_var.to_string(), noun: None, quant: subj_quant, mods: vec![], owner: None, span: None },
            Referent { var: obj_var.to_string(), noun: Some(obj_var.to_string()), quant: obj_quant, mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "be".to_string(),
            args: vec![Term::Var { var: subj_var.to_string() }, Term::Var { var: obj_var.to_string() }],
            negated,
            modal: None,
            adjuncts: vec![],
            attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: sce.to_string(),
    }
}

/// Build a Who/What question clause.
fn wh_question_clause(pred: &str, args: Vec<(&str, Quant)>, _focus: &str, kind: QuestionKind, sce: &str) -> Clause {
    let referents: Vec<Referent> = args.iter().map(|(var, quant)| {
        let noun = match quant {
            Quant::Wh | Quant::Named(_) => None,
            _ => Some(var.to_string()),
        };
        Referent { var: var.to_string(), noun, quant: quant.clone(), mods: vec![], owner: None, span: None }
    }).collect();
    let terms: Vec<Term> = args.iter().map(|(var, _)| Term::Var { var: var.to_string() }).collect();
    Clause {
        act: Act::Question { kind },
        referents,
        conditions: vec![Pred {
            pred: pred.to_string(), args: terms, negated: false, modal: None, adjuncts: vec![], attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: sce.to_string(),
    }
}

/// Build a YesNo question clause.
fn yesno_clause(pred: &str, args: Vec<(&str, Quant)>, sce: &str) -> Clause {
    let referents: Vec<Referent> = args.iter().map(|(var, quant)| {
        let noun = match quant {
            Quant::Named(_) => None,
            _ => Some(var.to_string()),
        };
        Referent { var: var.to_string(), noun, quant: quant.clone(), mods: vec![], owner: None, span: None }
    }).collect();
    let terms: Vec<Term> = args.iter().map(|(var, _)| Term::Var { var: var.to_string() }).collect();
    Clause {
        act: Act::Question { kind: QuestionKind::YesNo },
        referents,
        conditions: vec![Pred {
            pred: pred.to_string(), args: terms, negated: false, modal: None, adjuncts: vec![], attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: sce.to_string(),
    }
}

// ---- tests ----------------------------------------------------------------

/// Test 1: "User greets Assistant." -> Greet{returning:false/true}
#[test]
fn greet_returns_greet_move() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    let clause = assert_clause(
        "greet",
        vec![("x1", Quant::Named("User".into())), ("x2", Quant::Named("Assistant".into()))],
        "User greets Assistant.",
    );

    // Not returning.
    let gs = ground_all(&mut state, &[clause.clone()], &can);
    let result = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match result {
        Dispatched::Moves(mvs) => {
            assert!(mvs.iter().any(|m| matches!(m, Move::Greet { returning: false })),
                "expected Greet{{returning:false}}, got {:?}", mvs);
        }
        other => panic!("expected Moves, got {:?}", other),
    }

    // Returning.
    let gs2 = ground_all(&mut state, &[clause], &can);
    let result2 = dispatch(&mut make_ctx(&mut can, &store, &kernel, true), &gs2[0], &state).unwrap();
    match result2 {
        Dispatched::Moves(mvs) => {
            assert!(mvs.iter().any(|m| matches!(m, Move::Greet { returning: true })),
                "expected Greet{{returning:true}}, got {:?}", mvs);
        }
        other => panic!("expected Moves, got {:?}", other),
    }
}

/// Test 2: thanks -> Ack("you're welcome"), say-goodbye -> Farewell.
#[test]
fn thanks_and_farewell() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    // Thanks
    let thanks_clause = assert_clause(
        "thank",
        vec![("x1", Quant::Named("User".into())), ("x2", Quant::Named("Assistant".into()))],
        "User thanks Assistant.",
    );
    let gs = ground_all(&mut state, &[thanks_clause], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match r {
        Dispatched::Moves(mvs) => {
            assert!(mvs.iter().any(|m| matches!(m, Move::Ack { .. })),
                "expected Ack for thanks, got {:?}", mvs);
        }
        other => panic!("expected Moves, got {:?}", other),
    }

    // Farewell - using "say-goodbye" which is registered in dialog.farewell.
    let farewell_clause = assert_clause(
        "say-goodbye",
        vec![("x1", Quant::Named("User".into())), ("x2", Quant::Named("Assistant".into()))],
        "User says-goodbye-to Assistant.",
    );
    let gs2 = ground_all(&mut state, &[farewell_clause], &can);
    let r2 = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs2[0], &state).unwrap();
    match r2 {
        Dispatched::Moves(mvs) => {
            assert!(mvs.iter().any(|m| matches!(m, Move::Farewell)),
                "expected Farewell, got {:?}", mvs);
        }
        other => panic!("expected Moves, got {:?}", other),
    }
}

/// Test 3: assert fact then who-question.
#[test]
fn assert_then_who_question() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    // "John owns a dog."
    let mut assert_c = assert_clause(
        "own",
        vec![("x1", Quant::Named("John".into())), ("x2", Quant::Indef)],
        "John owns a dog.",
    );
    assert_c.referents[1].noun = Some("dog".into());
    let gs = ground_all(&mut state, &[assert_c.clone()], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    assert!(matches!(r, Dispatched::Moves(_)), "expected Moves, got {:?}", r);

    // "Who owns a dog?" - Wh focus x_who, indef dog resolves to dog_1 via salience.
    let q_clause = wh_question_clause(
        "own",
        vec![("x_who", Quant::Wh), ("x2", Quant::Indef)],
        "x_who",
        QuestionKind::Who { focus: "x_who".into() },
        "Who owns a dog?",
    );
    // Set noun for x2 referent so discourse can match dog_1.
    let mut q_clause = q_clause;
    q_clause.referents[1].noun = Some("dog".into());

    let gs2 = ground_all(&mut state, &[q_clause], &can);
    let r2 = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs2[0], &state).unwrap();
    match r2 {
        Dispatched::Moves(mvs) => {
            let has_john = mvs.iter().any(|m| {
                if let Move::Answer { values, .. } = m {
                    values.iter().any(|v| v.render().to_lowercase().contains("john"))
                } else { false }
            });
            assert!(has_john, "expected Answer containing John, got {:?}", mvs);
        }
        other => panic!("expected Moves, got {:?}", other),
    }
}

/// Test 4: yes/no from facts.
#[test]
fn yesno_from_facts() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    // Assert "John owns a dog."
    let mut assert_c = assert_clause(
        "own",
        vec![("x1", Quant::Named("John".into())), ("x2", Quant::Indef)],
        "John owns a dog.",
    );
    assert_c.referents[1].noun = Some("dog".into());
    let gs = ground_all(&mut state, &[assert_c], &can);
    dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();

    // "Does John own a dog?" -> YesNo true.
    let q1 = yesno_clause(
        "own",
        vec![("x1", Quant::Named("John".into())), ("x2", Quant::Indef)],
        "Does John own a dog?",
    );
    let mut q1 = q1;
    q1.referents[1].noun = Some("dog".into());
    let gs1 = ground_all(&mut state, &[q1], &can);
    let r1 = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs1[0], &state).unwrap();
    match r1 {
        Dispatched::Moves(mvs) => {
            let yes = mvs.iter().any(|m| matches!(m, Move::YesNo { answer: true, .. }));
            assert!(yes, "expected YesNo true for John, got {:?}", mvs);
        }
        other => panic!("expected Moves, got {:?}", other),
    }

    // "Does Mary own a dog?" -> YesNo false or Answer-empty (documented: Answer-empty when no fact).
    let q2 = yesno_clause(
        "own",
        vec![("x1", Quant::Named("Mary".into())), ("x2", Quant::Indef)],
        "Does Mary own a dog?",
    );
    let mut q2 = q2;
    q2.referents[1].noun = Some("dog".into());
    let gs2 = ground_all(&mut state, &[q2], &can);
    let r2 = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs2[0], &state).unwrap();
    // Either YesNo(false) or an honest unknown - both are valid (no Mary fact).
    match r2 {
        Dispatched::Moves(mvs) => {
            let is_no = mvs.iter().any(|m| {
                matches!(m, Move::YesNo { answer: false, .. })
                    || matches!(m, Move::Explain { text } if text.starts_with("I don't know whether Mary"))
            });
            assert!(is_no, "expected YesNo(false) or honest unknown for Mary, got {:?}", mvs);
        }
        other => panic!("expected Moves for Mary question, got {:?}", other),
    }
}

/// Test 5: "User is tired." -> Empathize{feeling contains "tired"}.
#[test]
fn small_talk_feeling() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    let clause = attr_assert_clause("x1", Quant::Named("User".into()), "tired", "User is tired.");
    let gs = ground_all(&mut state, &[clause], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match r {
        Dispatched::Moves(mvs) => {
            let has_empathy = mvs.iter().any(|m| {
                if let Move::Empathize { feeling, .. } = m {
                    feeling.contains("tired")
                } else { false }
            });
            assert!(has_empathy, "expected Empathize with feeling 'tired', got {:?}", mvs);
        }
        other => panic!("expected Moves, got {:?}", other),
    }
}

/// Test 6: "User thinks that the weekend is too short." -> Reflect.
#[test]
fn belief_reflects() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    // Build the sub-clause.
    let sub_clause = Clause {
        act: Act::Assert,
        referents: vec![],
        conditions: vec![],
        then: vec![],
        then_referents: vec![],
        sce: "the weekend is too short".to_string(),
    };
    let clause = Clause {
        act: Act::Assert,
        referents: vec![
            Referent { var: "x1".into(), noun: None, quant: Quant::Named("User".into()), mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "think".to_string(),
            args: vec![
                Term::Var { var: "x1".into() },
                Term::Sub { clause: Box::new(sub_clause) },
            ],
            negated: false,
            modal: None,
            adjuncts: vec![],
            attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "User thinks that the weekend is too short.".to_string(),
    };
    let gs = ground_all(&mut state, &[clause], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match r {
        Dispatched::Moves(mvs) => {
            let has_reflect = mvs.iter().any(|m| matches!(m, Move::Reflect { .. }));
            assert!(has_reflect, "expected Reflect, got {:?}", mvs);
        }
        other => panic!("expected Moves, got {:?}", other),
    }
}

/// Test 7: arithmetic command "calculate 3 / 500 * 3600" -> Result{value ~ 21.6}.
#[test]
fn arith_command() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    // ArithExpr: (3 / 500) * 3600
    let expr = ArithExpr::Bin {
        op: ArithOp::Mul,
        lhs: Box::new(ArithExpr::Bin {
            op: ArithOp::Div,
            lhs: Box::new(ArithExpr::Num { value: 3.0 }),
            rhs: Box::new(ArithExpr::Num { value: 500.0 }),
        }),
        rhs: Box::new(ArithExpr::Num { value: 3600.0 }),
    };

    // Verify compile produces math.div and math.mul.
    let compiled = spoon_mind::dispatch::arith::compile(&expr);
    let mut action_ids = vec![];
    compiled.actions(&mut action_ids);
    let id_strs: Vec<&str> = action_ids.iter().map(|a| a.0.as_str()).collect();
    assert!(id_strs.contains(&"math.div"), "compile should reference math.div, got {:?}", id_strs);
    assert!(id_strs.contains(&"math.mul"), "compile should reference math.mul, got {:?}", id_strs);

    // Build command clause.
    let clause = Clause {
        act: Act::Command,
        referents: vec![
            Referent { var: "x1".into(), noun: None, quant: Quant::Named("Assistant".into()), mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "calculate".to_string(),
            args: vec![Term::Arith { expr }],
            negated: false,
            modal: None,
            adjuncts: vec![],
            attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "Assistant, calculate 3 / 500 * 3600!".to_string(),
    };
    let gs = ground_all(&mut state, &[clause], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match r {
        Dispatched::Moves(mvs) => {
            let result_val = mvs.iter().find_map(|m| {
                if let Move::Result { value, .. } = m { Some(value.clone()) } else { None }
            });
            let val = result_val.expect("expected Result move");
            let f = match val {
                Value::Float(f) => f,
                Value::Int(i) => i as f64,
                other => panic!("expected Float result, got {:?}", other),
            };
            assert!((f - 21.6).abs() < 1e-9, "expected 21.6, got {}", f);
        }
        other => panic!("expected Moves, got {:?}", other),
    }
}

/// Test 8: command maps to Intent with known action.
#[test]
fn command_to_intent() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);

    // Add a dummy pure action "demo.shout(Text) -> Text" with verb "shout".
    can.add_action(Action::primitive(
        "demo.shout",
        &["shout"],
        vec![Input::required("text", Type::Text)],
        Type::Text,
        Effect::Pure,
        "shout text back",
    ));

    let mut state = DiscourseState::default();
    let clause = Clause {
        act: Act::Command,
        referents: vec![
            Referent { var: "x1".into(), noun: None, quant: Quant::Named("Assistant".into()), mods: vec![], owner: None, span: None },
            Referent { var: "x2".into(), noun: None, quant: Quant::Literal(Value::Text("hello".into())), mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "shout".to_string(),
            args: vec![Term::Var { var: "x1".into() }, Term::Var { var: "x2".into() }],
            negated: false, modal: None, adjuncts: vec![], attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "Assistant, shout 'hello'!".to_string(),
    };
    let gs = ground_all(&mut state, &[clause], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match r {
        Dispatched::Plan { intent, .. } => {
            assert!(
                matches!(&intent.goal, Goal::Action { action } if action.0 == "demo.shout"),
                "expected Action(demo.shout), got {:?}", intent.goal
            );
            let text_sig = intent.signals.iter().find(|s| s.ty == Type::Text);
            assert!(text_sig.is_some(), "expected a Text signal, got {:?}", intent.signals);
            let v = text_sig.unwrap().value.as_str().unwrap_or("");
            assert_eq!(v, "hello", "expected signal value 'hello', got '{}'", v);
        }
        other => panic!("expected Plan, got {:?}", other),
    }
}

/// Test 9: unknown verb -> UnknownCapability with ONE move asking for SCE examples.
#[test]
fn unknown_verb() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    let clause = Clause {
        act: Act::Command,
        referents: vec![
            Referent { var: "x1".into(), noun: None, quant: Quant::Named("Assistant".into()), mods: vec![], owner: None, span: None },
            Referent { var: "x2".into(), noun: Some("dog".into()), quant: Quant::Def, mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "frobnicate".to_string(),
            args: vec![Term::Var { var: "x1".into() }, Term::Var { var: "x2".into() }],
            negated: false, modal: None, adjuncts: vec![], attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "Assistant, frobnicate the dog!".to_string(),
    };
    let gs = ground_all(&mut state, &[clause], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match r {
        Dispatched::UnknownCapability { verb, fallback, .. } => {
            assert_eq!(verb, "frobnicate");
            assert_eq!(fallback.len(), 1, "one move, got {:?}", fallback);
            match &fallback[0] {
                Move::Clarify { question, .. } => {
                    assert!(question.starts_with("I can't frobnicate yet."), "{question}");
                    assert!(question.contains("'The frobnicate of "), "{question}");
                }
                other => panic!("expected Clarify asking for examples, got {:?}", other),
            }
        }
        other => panic!("expected UnknownCapability, got {:?}", other),
    }
}

/// Test 10: self-model identity and wellbeing queries.
#[test]
fn self_identity_and_wellbeing() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    // Seed: "The name of Assistant is Spoon." (as rel.name(Assistant, Spoon))
    let name_assert = assert_clause(
        "name",
        vec![("x_asst", Quant::Named("Assistant".into())), ("x_spoon", Quant::Named("Spoon".into()))],
        "The name of Assistant is Spoon.",
    );
    let gs = ground_all(&mut state, &[name_assert], &can);
    dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();

    // Seed: "The wellbeing of Assistant is good." (as rel.wellbeing(Assistant, Text("good")))
    let wb_assert = Clause {
        act: Act::Assert,
        referents: vec![
            Referent { var: "x_asst".into(), noun: None, quant: Quant::Named("Assistant".into()), mods: vec![], owner: None, span: None },
            Referent { var: "x_good".into(), noun: None, quant: Quant::Literal(Value::Text("good".into())), mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "wellbeing".to_string(),
            args: vec![Term::Var { var: "x_asst".into() }, Term::Value { value: Value::Text("good".into()) }],
            negated: false, modal: None, adjuncts: vec![], attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "The wellbeing of Assistant is good.".to_string(),
    };
    let gs = ground_all(&mut state, &[wb_assert], &can);
    dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();

    // Seed: "Assistant is not a person."
    let person_assert = be_np_clause(
        "x_asst", Quant::Named("Assistant".into()),
        "person", Quant::Indef,
        true, // negated
        "Assistant is not a person.",
    );
    let gs = ground_all(&mut state, &[person_assert], &can);
    dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();

    // Query: "What is the name of Assistant?" -> Answer containing "Spoon".
    let name_q = wh_question_clause(
        "name",
        vec![("x_asst", Quant::Named("Assistant".into())), ("x_focus", Quant::Wh)],
        "x_focus",
        QuestionKind::What { focus: "x_focus".into() },
        "What is the name of Assistant?",
    );
    let gs = ground_all(&mut state, &[name_q], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match r {
        Dispatched::Moves(mvs) => {
            let has_spoon = mvs.iter().any(|m| {
                if let Move::Answer { values, .. } = m {
                    values.iter().any(|v| v.render().to_lowercase().contains("spoon"))
                } else if let Move::Explain { text } = m {
                    text.to_lowercase().contains("spoon")
                } else { false }
            });
            assert!(has_spoon, "expected 'Spoon' in answer, got {:?}", mvs);
        }
        other => panic!("expected Moves for name query, got {:?}", other),
    }

    // Query: "What is the wellbeing of Assistant?" -> Answer containing "good".
    let wb_q = wh_question_clause(
        "wellbeing",
        vec![("x_asst", Quant::Named("Assistant".into())), ("x_focus", Quant::Wh)],
        "x_focus",
        QuestionKind::What { focus: "x_focus".into() },
        "What is the wellbeing of Assistant?",
    );
    let gs = ground_all(&mut state, &[wb_q], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match r {
        Dispatched::Moves(mvs) => {
            let has_good = mvs.iter().any(|m| {
                if let Move::Answer { values, .. } = m {
                    values.iter().any(|v| v.render().to_lowercase().contains("good"))
                } else if let Move::Explain { text } = m {
                    text.to_lowercase().contains("good") || text.contains("fine")
                } else { false }
            });
            assert!(has_good, "expected 'good' in wellbeing answer, got {:?}", mvs);
        }
        other => panic!("expected Moves for wellbeing query, got {:?}", other),
    }

    // Query: "Is Assistant a person?" -> YesNo{answer:false}.
    // Use Term::Value{Name("Person")} for the object so answer_be_yesno can find the target
    // concept without needing the all_refs slice (which answer_yes_no passes as empty).
    let person_q = Clause {
        act: Act::Question { kind: QuestionKind::YesNo },
        referents: vec![
            Referent { var: "x_asst".into(), noun: None, quant: Quant::Named("Assistant".into()), mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "be".to_string(),
            args: vec![
                Term::Var { var: "x_asst".into() },
                Term::Value { value: Value::Name("Person".into()) },
            ],
            negated: false, modal: None, adjuncts: vec![], attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "Is Assistant a person?".to_string(),
    };
    let gs = ground_all(&mut state, &[person_q], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match r {
        Dispatched::Moves(mvs) => {
            let is_false = mvs.iter().any(|m| matches!(m, Move::YesNo { answer: false, .. }));
            assert!(is_false, "expected YesNo{{answer:false}} for 'Is Assistant a person?', got {:?}", mvs);
        }
        other => panic!("expected Moves for person query, got {:?}", other),
    }
}

/// Test 11: capabilities -> Explain mentioning "calculate" and "remember".
#[test]
fn capabilities() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    // "What can Assistant do?" -> pred "can" with modal Can.
    let clause = Clause {
        act: Act::Question { kind: QuestionKind::What { focus: "x_do".into() } },
        referents: vec![
            Referent { var: "x_asst".into(), noun: None, quant: Quant::Named("Assistant".into()), mods: vec![], owner: None, span: None },
            Referent { var: "x_do".into(), noun: None, quant: Quant::Wh, mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "do".to_string(),
            args: vec![Term::Var { var: "x_asst".into() }, Term::Var { var: "x_do".into() }],
            negated: false,
            modal: Some(Modal::Can),
            adjuncts: vec![],
            attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "What can Assistant do?".to_string(),
    };
    let gs = ground_all(&mut state, &[clause], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    match r {
        Dispatched::Moves(mvs) => {
            let text = mvs.iter().find_map(|m| {
                if let Move::Explain { text } = m { Some(text.clone()) } else { None }
            });
            let text = text.expect("expected Explain move, got {:?}");
            let lower = text.to_lowercase();
            assert!(
                lower.contains("calculat") || lower.contains("math"),
                "expected mention of math/calculate in capabilities, got: {}", text
            );
            assert!(
                lower.contains("remember") || lower.contains("fact"),
                "expected mention of remember/facts in capabilities, got: {}", text
            );
        }
        other => panic!("expected Moves, got {:?}", other),
    }
}

/// Test 12: opinion from stance and fallback for unknown topic.
#[test]
fn opinion_from_stance_and_fallback() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);

    // Upsert a stance on pineapple pizza.
    store.upsert_stance(&spoon_core::store::Stance {
        id: 0,
        topic: "pineapple pizza".to_string(),
        stance: "it's fine, sweet and salty works".to_string(),
        reasons: vec!["sweet and salty is a classic combination".to_string()],
        confidence: 0.6,
        source: "test".to_string(),
        at: 0,
    }).unwrap();

    let mut state = DiscourseState::default();

    // "What does Assistant think about pineapple-pizza?"
    let pizza_q = Clause {
        act: Act::Question { kind: QuestionKind::What { focus: "x_pizza".into() } },
        referents: vec![
            Referent { var: "x_asst".into(), noun: None, quant: Quant::Named("Assistant".into()), mods: vec![], owner: None, span: None },
            Referent { var: "x_pizza".into(), noun: Some("pineapple-pizza".into()), quant: Quant::Def, mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "think".to_string(),
            args: vec![Term::Var { var: "x_asst".into() }, Term::Var { var: "x_pizza".into() }],
            negated: false, modal: None, adjuncts: vec![], attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "What does Assistant think about pineapple-pizza?".to_string(),
    };
    let gs = ground_all(&mut state, &[pizza_q], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();
    assert!(
        matches!(r, Dispatched::Moves(ref mvs) if mvs.iter().any(|m| matches!(m, Move::Opinion { .. }))),
        "expected Opinion for pineapple pizza, got {:?}", r
    );

    // "What does Assistant think about social-media?" -> NeedsTeacher.
    let social_q = Clause {
        act: Act::Question { kind: QuestionKind::What { focus: "x_sm".into() } },
        referents: vec![
            Referent { var: "x_asst".into(), noun: None, quant: Quant::Named("Assistant".into()), mods: vec![], owner: None, span: None },
            Referent { var: "x_sm".into(), noun: Some("social-media".into()), quant: Quant::Def, mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "think".to_string(),
            args: vec![Term::Var { var: "x_asst".into() }, Term::Var { var: "x_sm".into() }],
            negated: false, modal: None, adjuncts: vec![], attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "What does Assistant think about social-media?".to_string(),
    };
    let gs2 = ground_all(&mut state, &[social_q], &can);
    let r2 = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs2[0], &state).unwrap();
    match r2 {
        Dispatched::NeedsTeacher { ask: spoon_mind::dispatch::TeacherAsk::Stance { topic }, fallback } => {
            assert!(topic.to_lowercase().contains("social"), "expected topic about social, got {}", topic);
            assert!(!fallback.is_empty(), "expected non-empty fallback");
        }
        other => panic!("expected NeedsTeacher(Stance) for social-media, got {:?}", other),
    }
}

/// Test 13: advice fallback with roommate situation.
#[test]
fn advice_fallback() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);
    let mut state = DiscourseState::default();

    // "User fights with User's roommate."
    let fight_clause = Clause {
        act: Act::Assert,
        referents: vec![
            Referent { var: "x_user".into(), noun: None, quant: Quant::Named("User".into()), mods: vec![], owner: None, span: None },
            Referent { var: "x_roommate".into(), noun: Some("roommate".into()), quant: Quant::Indef, mods: vec![], owner: Some("x_user".into()), span: None },
        ],
        conditions: vec![Pred {
            pred: "fight".to_string(),
            args: vec![Term::Var { var: "x_user".into() }],
            negated: false, modal: None,
            adjuncts: vec![("with".to_string(), Term::Var { var: "x_roommate".into() })],
            attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "User fights with User's roommate.".to_string(),
    };
    let gs = ground_all(&mut state, &[fight_clause.clone()], &can);
    dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs[0], &state).unwrap();

    // Simulate brain updating last_clauses.
    state.last_clauses = vec![fight_clause];

    // "What should User do?"
    let advice_q = Clause {
        act: Act::Question { kind: QuestionKind::What { focus: "x_action".into() } },
        referents: vec![
            Referent { var: "x_user".into(), noun: None, quant: Quant::Named("User".into()), mods: vec![], owner: None, span: None },
            Referent { var: "x_action".into(), noun: None, quant: Quant::Wh, mods: vec![], owner: None, span: None },
        ],
        conditions: vec![Pred {
            pred: "do".to_string(),
            args: vec![Term::Var { var: "x_user".into() }, Term::Var { var: "x_action".into() }],
            negated: false,
            modal: Some(Modal::Should),
            adjuncts: vec![],
            attr: None,
        }],
        then: vec![],
        then_referents: vec![],
        sce: "What should User do?".to_string(),
    };
    let gs2 = ground_all(&mut state, &[advice_q], &can);
    let r = dispatch(&mut make_ctx(&mut can, &store, &kernel, false), &gs2[0], &state).unwrap();
    match r {
        Dispatched::NeedsTeacher { ask: spoon_mind::dispatch::TeacherAsk::Advice { situation, .. }, fallback } => {
            assert!(
                situation.to_lowercase().contains("roommate") || situation.to_lowercase().contains("fight"),
                "expected situation about roommate/fight, got '{}'", situation
            );
            let has_reflect = fallback.iter().any(|m| matches!(m, Move::Reflect { .. }));
            let has_clarify = fallback.iter().any(|m| matches!(m, Move::Clarify { .. }));
            assert!(has_reflect, "expected Reflect in fallback, got {:?}", fallback);
            assert!(has_clarify, "expected Clarify in fallback, got {:?}", fallback);
        }
        other => panic!("expected NeedsTeacher(Advice), got {:?}", other),
    }
}

/// Test 14: dispatch_turn merges two clause results.
#[test]
fn dispatch_turn_merges() {
    let kernel = Kernel::new();
    let store = Store::open_memory().unwrap();
    let mut can = make_can(&kernel);

    // Seed wellbeing fact.
    let wb_fact = Fact {
        id: 0,
        pred: ActionId("rel.wellbeing".into()),
        args: vec![Value::Name("Assistant".into()), Value::Text("good".into())],
        truth: true,
        modal: None,
        asserted_at: 0,
        invalidated_at: None,
        source: "seed".into(),
        episode_id: None,
    };
    // We need to ensure the action exists in CAN before insert (or insert directly).
    can.add_action(Action {
        id: ActionId("rel.wellbeing".into()),
        inputs: vec![Input::required("a", Type::Any), Input::required("b", Type::Any)],
        output: Type::Bool,
        effect: Effect::Pure,
        imp: Impl::Primitive,
        role: Role::Relation,
        verbs: vec!["wellbeing".to_string()],
        phrasings: vec![],
        description: "wellbeing relation".to_string(),
        tier: Tier::Provisional,
        provenance: Provenance::Kernel,
        stats: Stats::default(),
    });
    store.insert_fact(&wb_fact).unwrap();

    let mut state = DiscourseState::default();

    // Clause 1: "User greets Assistant."
    let greet_c = assert_clause(
        "greet",
        vec![("x1", Quant::Named("User".into())), ("x2", Quant::Named("Assistant".into()))],
        "User greets Assistant.",
    );

    // Clause 2: "What is the wellbeing of Assistant?"
    let wb_q = wh_question_clause(
        "wellbeing",
        vec![("x_asst", Quant::Named("Assistant".into())), ("x_focus", Quant::Wh)],
        "x_focus",
        QuestionKind::What { focus: "x_focus".into() },
        "What is the wellbeing of Assistant?",
    );

    let clauses = vec![greet_c, wb_q];
    let gs = ground_all(&mut state, &clauses, &can);
    let r = dispatch_turn(&mut make_ctx(&mut can, &store, &kernel, false), &gs, &state).unwrap();
    match r {
        Dispatched::Moves(mvs) => {
            let has_greet = mvs.iter().any(|m| matches!(m, Move::Greet { .. }));
            let has_answer = mvs.iter().any(|m| {
                matches!(m, Move::Answer { .. }) || matches!(m, Move::Explain { .. })
            });
            assert!(has_greet, "expected Greet in merged moves, got {:?}", mvs);
            assert!(has_answer, "expected Answer or Explain in merged moves, got {:?}", mvs);
        }
        other => panic!("expected Moves from dispatch_turn, got {:?}", other),
    }
}

/// Test 15: no LLM import in dispatch source files.
#[test]
fn no_llm_import() {
    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dispatch");
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&src_dir).expect("dispatch dir should exist") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(
                !content.contains("spoon_core::llm"),
                "LLM import found in {:?} - hard rule violation",
                path
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "expected to check at least one dispatch source file");
}
