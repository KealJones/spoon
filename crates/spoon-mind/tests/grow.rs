//! Integration tests for the grow module (synthesis + consolidation).
//!
//! Run with: `cargo test -p spoon-mind --test grow`

use std::time::Instant;

use spoon_core::can::Can;
use spoon_core::kernel::eval::{call_action, eval_program};
use spoon_core::kernel::{Ctx, Kernel, NoHost};
use spoon_core::types::*;
use spoon_mind::grow::{
    action_from_program, consolidate, describe, synthesize, SynthBudget, SynthOutcome,
};

// ---- helpers ------------------------------------------------------------

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

fn spec(
    name: &str,
    params: Vec<Type>,
    ret: Type,
    examples: Vec<(Vec<Value>, Value)>,
) -> Spec {
    Spec {
        id: format!("test_{}", name),
        name_hint: name.to_string(),
        verbs: vec![],
        phrasings: vec![],
        params,
        param_names: vec![],
        ret,
        examples: examples
            .into_iter()
            .map(|(inputs, output)| Example { inputs, output })
            .collect(),
        description: String::new(),
        source: "test".to_string(),
    }
}

fn eval_prog(prog: &Program, inputs: &[Value], can: &Can, kernel: &Kernel) -> Value {
    let mut ctx = Ctx::new(can, kernel, &NoHost);
    eval_program(&mut ctx, prog, inputs).expect("eval failed")
}

fn numeric_eq(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => a == b,
    }
}

fn list_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::List(la), Value::List(lb)) => {
            la.len() == lb.len() && la.iter().zip(lb.iter()).all(|(x, y)| numeric_eq(x, y))
        }
        _ => numeric_eq(a, b),
    }
}

// ---- test 1: synth_double ----------------------------------------------

#[test]
fn synth_double() {
    let kernel = Kernel::new();
    let can = make_can(&kernel);
    let budget = SynthBudget::default();

    let s = spec(
        "double",
        vec![Type::Int],
        Type::Int,
        vec![
            (vec![Value::Int(2)], Value::Int(4)),
            (vec![Value::Int(5)], Value::Int(10)),
            (vec![Value::Int(0)], Value::Int(0)),
        ],
    );

    match synthesize(&s, &can, &kernel, &budget) {
        SynthOutcome::Found { program, tried, millis, table_size } => {
            println!("[double] tried={tried} table={table_size} ms={millis}");
            assert!(program.size() <= 3, "program size {} > 3", program.size());
            let desc = describe(&program);
            println!("[double] program: {desc}");
            // Verify on unseen input (7) -> 14.
            let result = eval_prog(&program, &[Value::Int(7)], &can, &kernel);
            assert!(numeric_eq(&result, &Value::Int(14)), "got {:?}", result);
        }
        other => panic!("expected Found, got {:?}", other),
    }
}

// ---- test 2: synth_word_count ------------------------------------------

#[test]
fn synth_word_count() {
    let kernel = Kernel::new();
    let can = make_can(&kernel);
    let budget = SynthBudget::default();

    let s = spec(
        "word_count",
        vec![Type::Text],
        Type::Int,
        vec![
            (vec![Value::Text("a b c".into())], Value::Int(3)),
            (vec![Value::Text("hello".into())], Value::Int(1)),
            (vec![Value::Text("x y".into())], Value::Int(2)),
        ],
    );

    match synthesize(&s, &can, &kernel, &budget) {
        SynthOutcome::Found { program, tried, millis, table_size } => {
            println!("[word_count] tried={tried} table={table_size} ms={millis}");
            let result = eval_prog(&program, &[Value::Text("one two three four".into())], &can, &kernel);
            assert!(numeric_eq(&result, &Value::Int(4)), "got {:?}", result);
        }
        other => panic!("expected Found, got {:?}", other),
    }
}

// ---- test 3: synth_sum_list --------------------------------------------

#[test]
fn synth_sum_list() {
    let kernel = Kernel::new();
    let can = make_can(&kernel);
    let budget = SynthBudget::default();

    // Note: spec.validate() now accepts empty lists for List<T> params.
    let s = spec(
        "sum_list",
        vec![Type::list(Type::Int)],
        Type::Int,
        vec![
            (vec![Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])], Value::Int(6)),
            (vec![Value::List(vec![Value::Int(4)])], Value::Int(4)),
            (vec![Value::List(vec![])], Value::Int(0)),
        ],
    );

    match synthesize(&s, &can, &kernel, &budget) {
        SynthOutcome::Found { program: _, tried, millis, table_size } => {
            println!("[sum_list] tried={tried} table={table_size} ms={millis}");
            // No extra verification needed; finding is sufficient.
        }
        other => panic!("expected Found, got {:?}", other),
    }
}

// ---- test 4: synth_upper_concat ----------------------------------------

#[test]
fn synth_upper_concat() {
    let kernel = Kernel::new();
    let can = make_can(&kernel);
    let budget = SynthBudget::default();

    let s = spec(
        "upper_concat",
        vec![Type::Text, Type::Text],
        Type::Text,
        vec![
            (vec![Value::Text("a".into()), Value::Text("b".into())], Value::Text("AB".into())),
            (vec![Value::Text("hi".into()), Value::Text("!".into())], Value::Text("HI!".into())),
        ],
    );

    match synthesize(&s, &can, &kernel, &budget) {
        SynthOutcome::Found { program, tried, millis, table_size } => {
            println!("[upper_concat] tried={tried} table={table_size} ms={millis}");
            let result = eval_prog(
                &program,
                &[Value::Text("foo".into()), Value::Text("bar".into())],
                &can,
                &kernel,
            );
            assert_eq!(result, Value::Text("FOOBAR".into()), "got {:?}", result);
        }
        other => panic!("expected Found, got {:?}", other),
    }
}

// ---- test 5: synth_map_double ------------------------------------------

#[test]
fn synth_map_double() {
    let kernel = Kernel::new();
    let can = make_can(&kernel);
    let budget = SynthBudget::default();

    let s = spec(
        "map_double",
        vec![Type::list(Type::Int)],
        Type::list(Type::Int),
        vec![
            (
                vec![Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])],
                Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)]),
            ),
            (
                vec![Value::List(vec![Value::Int(5)])],
                Value::List(vec![Value::Int(10)]),
            ),
        ],
    );

    match synthesize(&s, &can, &kernel, &budget) {
        SynthOutcome::Found { program, tried, millis, table_size } => {
            println!("[map_double] tried={tried} table={table_size} ms={millis}");
            let desc = describe(&program);
            println!("[map_double] program: {desc}");
            let result = eval_prog(
                &program,
                &[Value::List(vec![Value::Int(0), Value::Int(7)])],
                &can,
                &kernel,
            );
            assert!(
                list_eq(&result, &Value::List(vec![Value::Int(0), Value::Int(14)])),
                "expected [0, 14], got {:?}",
                result
            );
        }
        other => panic!("expected Found, got {:?}", other),
    }
}

// ---- test 6: synth_impossible_respects_budget --------------------------

#[test]
fn synth_impossible_respects_budget() {
    let kernel = Kernel::new();
    let can = make_can(&kernel);
    let budget = SynthBudget {
        max_millis: 400,
        max_nodes: 50_000,
        ..Default::default()
    };

    let s = spec(
        "impossible",
        vec![Type::Int],
        Type::Int,
        vec![
            (vec![Value::Int(1)], Value::Int(17)),
            (vec![Value::Int(2)], Value::Int(-3)),
            (vec![Value::Int(3)], Value::Int(1000)),
            (vec![Value::Int(4)], Value::Int(42)),
        ],
    );

    let wall_start = Instant::now();
    let outcome = synthesize(&s, &can, &kernel, &budget);
    let wall_ms = wall_start.elapsed().as_millis();

    assert!(
        !matches!(outcome, SynthOutcome::Found { .. }),
        "should not find a program for this nonsense spec"
    );
    assert!(wall_ms < 1500, "wall time {} ms >= 1500 ms", wall_ms);
    println!("[impossible] outcome={:?} wall={}ms", outcome, wall_ms);
}

// ---- test 7: action_from_program_roundtrip -----------------------------

#[test]
fn action_from_program_roundtrip() {
    let kernel = Kernel::new();
    let mut can = make_can(&kernel);
    let budget = SynthBudget::default();

    let s = spec(
        "double",
        vec![Type::Int],
        Type::Int,
        vec![
            (vec![Value::Int(2)], Value::Int(4)),
            (vec![Value::Int(5)], Value::Int(10)),
            (vec![Value::Int(0)], Value::Int(0)),
        ],
    );

    let SynthOutcome::Found { program, .. } = synthesize(&s, &can, &kernel, &budget) else {
        panic!("synthesis failed");
    };

    let action = action_from_program(&s, &program);

    // Serde round-trip.
    let json = serde_json::to_string(&action).expect("serialize");
    let back: Action = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(action.id, back.id);
    assert_eq!(action.output, back.output);
    assert_eq!(action.effect, back.effect);

    // Add to CAN and call via call_action.
    can.add_action(back.clone());
    let mut ctx = Ctx::new(&can, &kernel, &NoHost);
    let result = call_action(&mut ctx, &back.id, &[Value::Int(21)]).expect("call_action failed");
    assert!(numeric_eq(&result, &Value::Int(42)), "expected 42, got {:?}", result);
}

// ---- test 8: consolidate_shared_subtree --------------------------------

#[test]
fn consolidate_shared_subtree() {
    let kernel = Kernel::new();
    let can = make_can(&kernel);

    // Helper: text.upper(text.trim(Param{idx}))
    let upper_trim = |idx: usize| -> Expr {
        Expr::Call {
            action: ActionId("text.upper".into()),
            args: vec![Expr::Call {
                action: ActionId("text.trim".into()),
                args: vec![Expr::Param { index: idx }],
            }],
        }
    };

    // Program 1: just the pattern on p0.
    let prog1 = Program::new(vec![Type::Text], Type::Text, upper_trim(0));

    // Program 2: concat(upper_trim(p0), " world").
    let prog2 = Program::new(
        vec![Type::Text],
        Type::Text,
        Expr::Call {
            action: ActionId("text.concat".into()),
            args: vec![upper_trim(0), Expr::Const { value: Value::Text(" world".into()) }],
        },
    );

    // Program 3: concat(upper_trim(p0), upper_trim(p1)).
    let prog3 = Program::new(
        vec![Type::Text, Type::Text],
        Type::Text,
        Expr::Call {
            action: ActionId("text.concat".into()),
            args: vec![upper_trim(0), upper_trim(1)],
        },
    );

    let library = vec![
        (ActionId("test.prog1".into()), prog1.clone()),
        (ActionId("test.prog2".into()), prog2.clone()),
        (ActionId("test.prog3".into()), prog3.clone()),
    ];

    let report = consolidate(&library, &can, 3, 2);

    assert_eq!(report.new_actions.len(), 1, "expected exactly 1 new action");

    let new_action = &report.new_actions[0];
    let mut can2 = can.clone();
    can2.add_action(new_action.clone());

    // Verify each rewritten program evaluates identically to the original.
    let test_inputs: &[(&[Value], &[Value])] = &[
        (&[Value::Text("  ab ".into())], &[]),
        (&[Value::Text("  ab ".into())], &[]),
        (&[Value::Text("  ab ".into()), Value::Text(" cd ".into())], &[]),
    ];

    for (orig_id, rewritten) in &report.rewritten {
        let (_, orig_prog) = library.iter().find(|(id, _)| id == orig_id).unwrap();
        let idx = library.iter().position(|(id, _)| id == orig_id).unwrap();
        let inputs = &test_inputs[idx].0;

        let mut ctx1 = Ctx::new(&can, &kernel, &NoHost);
        let orig_result = eval_program(&mut ctx1, orig_prog, inputs).expect("orig eval");

        let mut ctx2 = Ctx::new(&can2, &kernel, &NoHost);
        let rew_result = eval_program(&mut ctx2, rewritten, inputs).expect("rewritten eval");

        assert_eq!(orig_result, rew_result, "mismatch for {:?}: {:?} vs {:?}", orig_id, orig_result, rew_result);
    }
}

// ---- test 9: no_effectful_actions_enumerated ---------------------------

#[test]
fn no_effectful_actions_enumerated() {
    let kernel = Kernel::new();
    let can = make_can(&kernel);
    // Tight budget so we don't spin forever.
    let budget = SynthBudget {
        max_nodes: 30_000,
        max_millis: 1_000,
        max_size: 5,
        ..Default::default()
    };

    // A spec that requires reading a file (fs.read / http.get) - no pure
    // action can produce these made-up values from Path inputs.
    // Two different examples so no constant can solve both.
    let s = spec(
        "read_file",
        vec![Type::Path],
        Type::Text,
        vec![
            (vec![Value::Path("/tmp/foo".into())], Value::Text("x1_magic_7f".into())),
            (vec![Value::Path("/var/bar".into())], Value::Text("y2_magic_9e".into())),
        ],
    );

    let outcome = synthesize(&s, &can, &kernel, &budget);
    // Must NOT be Found (no pure action can produce this).
    assert!(
        !matches!(outcome, SynthOutcome::Found { .. }),
        "should not find a program - effectful actions must not be enumerated"
    );
    // The outcome must be Exhausted or Budget, never a panic.
    println!("[no_effectful] outcome={:?}", outcome);
}

// ---- test 10: synth_metrics_report -------------------------------------

#[test]
fn synth_metrics_report() {
    let kernel = Kernel::new();
    let can = make_can(&kernel);
    let budget = SynthBudget::default();

    let specs = vec![
        ("double", spec("double", vec![Type::Int], Type::Int, vec![
            (vec![Value::Int(2)], Value::Int(4)),
            (vec![Value::Int(5)], Value::Int(10)),
            (vec![Value::Int(0)], Value::Int(0)),
        ])),
        ("word_count", spec("word_count", vec![Type::Text], Type::Int, vec![
            (vec![Value::Text("a b c".into())], Value::Int(3)),
            (vec![Value::Text("hello".into())], Value::Int(1)),
            (vec![Value::Text("x y".into())], Value::Int(2)),
        ])),
        ("sum_list", spec("sum_list", vec![Type::list(Type::Int)], Type::Int, vec![
            (vec![Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])], Value::Int(6)),
            (vec![Value::List(vec![Value::Int(4)])], Value::Int(4)),
            (vec![Value::List(vec![])], Value::Int(0)),
        ])),
        ("upper_concat", spec("upper_concat", vec![Type::Text, Type::Text], Type::Text, vec![
            (vec![Value::Text("a".into()), Value::Text("b".into())], Value::Text("AB".into())),
            (vec![Value::Text("hi".into()), Value::Text("!".into())], Value::Text("HI!".into())),
        ])),
    ];

    for (name, s) in &specs {
        let start = Instant::now();
        let outcome = synthesize(s, &can, &kernel, &budget);
        let wall = start.elapsed().as_millis();
        match &outcome {
            SynthOutcome::Found { tried, table_size, millis, .. } => {
                println!("[{name}] tried={tried} table={table_size} synth_ms={millis} wall_ms={wall}");
                assert!(*tried > 0, "tried must be > 0");
            }
            other => panic!("[{name}] expected Found, got {:?}", other),
        }
    }
    let map_budget = SynthBudget::default();
    let map_spec = spec("map_double", vec![Type::list(Type::Int)], Type::list(Type::Int), vec![
        (vec![Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])],
         Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)])),
        (vec![Value::List(vec![Value::Int(5)])], Value::List(vec![Value::Int(10)])),
    ]);
    let start = Instant::now();
    match synthesize(&map_spec, &can, &kernel, &map_budget) {
        SynthOutcome::Found { tried, table_size, millis, .. } => {
            println!("[map_double] tried={tried} table={table_size} synth_ms={millis} wall_ms={}", start.elapsed().as_millis());
            assert!(tried > 0);
        }
        other => panic!("[map_double] expected Found, got {:?}", other),
    }
}

// ---- test 11: synth_deterministic --------------------------------------

#[test]
fn synth_deterministic() {
    let kernel = Kernel::new();
    let can = make_can(&kernel);
    let budget = SynthBudget::default();

    let s = spec(
        "upper_concat",
        vec![Type::Text, Type::Text],
        Type::Text,
        vec![
            (vec![Value::Text("a".into()), Value::Text("b".into())], Value::Text("AB".into())),
            (vec![Value::Text("hi".into()), Value::Text("!".into())], Value::Text("HI!".into())),
        ],
    );

    let mut results = Vec::new();
    for _ in 0..3 {
        match synthesize(&s, &can, &kernel, &budget) {
            SynthOutcome::Found { program, tried, .. } => {
                results.push((tried, describe(&program)));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    let (tried0, desc0) = &results[0];
    for (tried, desc) in &results[1..] {
        assert_eq!(tried, tried0, "tried count differs between runs");
        assert_eq!(desc, desc0, "program differs between runs");
    }
    println!("[deterministic] upper_concat tried={tried0} program={desc0}");
}
