//! Integration tests for the Stage 0 kernel.
//!
//! Covers eval_program, call_action, apply_lambda, all primitive modules,
//! budget/permission enforcement, sandbox checks, and meta-invariants.

use std::collections::HashSet;

use spoon_core::can::Can;
use spoon_core::kernel::eval::{apply_lambda, call_action, eval_program};
use spoon_core::kernel::{Budget, Ctx, EvalError, Kernel, NoHost};
use spoon_core::types::*;

// ---- helpers -----------------------------------------------------------

fn setup() -> (Kernel, Can) {
    let k = Kernel::new();
    let mut can = Can::new();
    for c in k.concepts() {
        can.add_concept(c.clone());
    }
    for a in k.actions() {
        can.add_action(a.clone());
    }
    (k, can)
}

fn ctx<'a>(can: &'a Can, k: &'a Kernel) -> Ctx<'a> {
    Ctx::new(can, k, &NoHost)
}

fn call(can: &Can, k: &Kernel, id: &str, args: &[Value]) -> Result<Value, EvalError> {
    let mut c = ctx(can, k);
    call_action(&mut c, &ActionId(id.into()), args)
}

fn prog(params: Vec<Type>, ret: Type, body: Expr) -> Program {
    Program::new(params, ret, body)
}

fn lambda(params: Vec<Type>, ret: Type, body: Expr) -> Value {
    Value::Lambda(Box::new(Lambda { params, ret, body }))
}

// ---- arithmetic --------------------------------------------------------

#[test]
fn test_mul_7_times_6_equals_42() {
    let (k, can) = setup();
    let program = prog(
        vec![Type::Int],
        Type::Int,
        Expr::Call {
            action: ActionId("math.mul".into()),
            args: vec![Expr::Param { index: 0 }, Expr::Const { value: Value::Int(6) }],
        },
    );
    let mut c = ctx(&can, &k);
    let result = eval_program(&mut c, &program, &[Value::Int(7)]).unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_nested_calls() {
    // add(mul(3, 4), 5) = 17
    let (k, can) = setup();
    let body = Expr::Call {
        action: ActionId("math.add".into()),
        args: vec![
            Expr::Call {
                action: ActionId("math.mul".into()),
                args: vec![Expr::Const { value: Value::Int(3) }, Expr::Const { value: Value::Int(4) }],
            },
            Expr::Const { value: Value::Int(5) },
        ],
    };
    let program = prog(vec![], Type::Int, body);
    let mut c = ctx(&can, &k);
    let result = eval_program(&mut c, &program, &[]).unwrap();
    assert_eq!(result, Value::Int(17));
}

#[test]
fn test_if_true_branch() {
    let (k, can) = setup();
    let body = Expr::If {
        cond: Box::new(Expr::Const { value: Value::Bool(true) }),
        then: Box::new(Expr::Const { value: Value::Int(1) }),
        otherwise: Box::new(Expr::Const { value: Value::Int(2) }),
    };
    let program = prog(vec![], Type::Int, body);
    let mut c = ctx(&can, &k);
    let result = eval_program(&mut c, &program, &[]).unwrap();
    assert_eq!(result, Value::Int(1));
}

#[test]
fn test_if_false_branch() {
    let (k, can) = setup();
    let body = Expr::If {
        cond: Box::new(Expr::Const { value: Value::Bool(false) }),
        then: Box::new(Expr::Const { value: Value::Int(1) }),
        otherwise: Box::new(Expr::Const { value: Value::Int(99) }),
    };
    let program = prog(vec![], Type::Int, body);
    let mut c = ctx(&can, &k);
    let result = eval_program(&mut c, &program, &[]).unwrap();
    assert_eq!(result, Value::Int(99));
}

// ---- lambdas and higher-order list ops ---------------------------------

#[test]
fn test_list_map_double() {
    // map([1,2,3], x -> mul(x, 2)) = [2, 4, 6]
    let (k, can) = setup();
    let lam = lambda(
        vec![Type::Any],
        Type::Any,
        Expr::Call {
            action: ActionId("math.mul".into()),
            args: vec![
                Expr::LambdaParam { index: 0 },
                Expr::Const { value: Value::Int(2) },
            ],
        },
    );
    let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    let mut c = ctx(&can, &k);
    let result = call_action(&mut c, &ActionId("list.map".into()), &[list, lam]).unwrap();
    assert_eq!(result, Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)]));
}

#[test]
fn test_list_filter_gt_2() {
    // filter([1,2,3,4], x -> x > 2) = [3, 4]
    let (k, can) = setup();
    let lam = lambda(
        vec![Type::Any],
        Type::Bool,
        Expr::Call {
            action: ActionId("math.gt".into()),
            args: vec![
                Expr::LambdaParam { index: 0 },
                Expr::Const { value: Value::Float(2.0) },
            ],
        },
    );
    let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)]);
    let mut c = ctx(&can, &k);
    let result = call_action(&mut c, &ActionId("list.filter".into()), &[list, lam]).unwrap();
    assert_eq!(result, Value::List(vec![Value::Int(3), Value::Int(4)]));
}

#[test]
fn test_fold_sum() {
    // fold([1,2,3,4,5], 0, (acc, x) -> add(acc, x)) = 15
    let (k, can) = setup();
    let lam = lambda(
        vec![Type::Any, Type::Any],
        Type::Any,
        Expr::Call {
            action: ActionId("math.add".into()),
            args: vec![Expr::LambdaParam { index: 0 }, Expr::LambdaParam { index: 1 }],
        },
    );
    let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4), Value::Int(5)]);
    let mut c = ctx(&can, &k);
    let result = call_action(&mut c, &ActionId("list.fold".into()), &[list, Value::Int(0), lam]).unwrap();
    assert_eq!(result, Value::Int(15));
}

#[test]
fn test_apply_lambda_directly() {
    let (k, can) = setup();
    let lam = Lambda {
        params: vec![Type::Int],
        ret: Type::Int,
        body: Expr::Call {
            action: ActionId("math.add".into()),
            args: vec![Expr::LambdaParam { index: 0 }, Expr::Const { value: Value::Int(10) }],
        },
    };
    let mut c = ctx(&can, &k);
    let result = apply_lambda(&mut c, &lam, &[Value::Int(5)]).unwrap();
    assert_eq!(result, Value::Int(15));
}

// ---- text ops ----------------------------------------------------------

#[test]
fn test_text_upper_lower() {
    let (k, can) = setup();
    assert_eq!(call(&can, &k, "text.upper", &[Value::text("hello")]).unwrap(), Value::text("HELLO"));
    assert_eq!(call(&can, &k, "text.lower", &[Value::text("WORLD")]).unwrap(), Value::text("world"));
}

#[test]
fn test_text_concat_trim_length() {
    let (k, can) = setup();
    assert_eq!(call(&can, &k, "text.concat", &[Value::text("foo"), Value::text("bar")]).unwrap(), Value::text("foobar"));
    assert_eq!(call(&can, &k, "text.trim", &[Value::text("  hi  ")]).unwrap(), Value::text("hi"));
    assert_eq!(call(&can, &k, "text.length", &[Value::text("hello")]).unwrap(), Value::Int(5));
}

#[test]
fn test_text_split_join() {
    let (k, can) = setup();
    let split = call(&can, &k, "text.split", &[Value::text("a,b,c"), Value::text(",")]).unwrap();
    assert_eq!(split, Value::List(vec![Value::text("a"), Value::text("b"), Value::text("c")]));

    let joined = call(&can, &k, "text.join", &[split, Value::text("-")]).unwrap();
    assert_eq!(joined, Value::text("a-b-c"));
}

#[test]
fn test_text_contains_and_starts_ends() {
    let (k, can) = setup();
    assert_eq!(call(&can, &k, "text.contains", &[Value::text("hello world"), Value::text("world")]).unwrap(), Value::Bool(true));
    assert_eq!(call(&can, &k, "text.starts_with", &[Value::text("hello"), Value::text("he")]).unwrap(), Value::Bool(true));
    assert_eq!(call(&can, &k, "text.ends_with", &[Value::text("hello"), Value::text("lo")]).unwrap(), Value::Bool(true));
}

#[test]
fn test_text_replace_reverse() {
    let (k, can) = setup();
    assert_eq!(
        call(&can, &k, "text.replace", &[Value::text("hello world"), Value::text("world"), Value::text("spoon")]).unwrap(),
        Value::text("hello spoon")
    );
    assert_eq!(
        call(&can, &k, "text.reverse", &[Value::text("abc")]).unwrap(),
        Value::text("cba")
    );
}

#[test]
fn test_text_regex() {
    let (k, can) = setup();
    let is_match = call(&can, &k, "text.regex_is_match", &[Value::text("hello123"), Value::text(r"\d+")]).unwrap();
    assert_eq!(is_match, Value::Bool(true));

    let finds = call(&can, &k, "text.regex_find_all", &[Value::text("abc 123 def 456"), Value::text(r"\d+")]).unwrap();
    assert_eq!(finds, Value::List(vec![Value::text("123"), Value::text("456")]));
}

// ---- json ops ----------------------------------------------------------

#[test]
fn test_json_get_nested() {
    let (k, can) = setup();
    let json_str = r#"{"user":{"name":"Alice","age":30},"scores":[10,20,30]}"#;
    let parsed = call(&can, &k, "json.parse", &[Value::text(json_str)]).unwrap();

    let name = call(&can, &k, "json.get", &[parsed.clone(), Value::text("user.name")]).unwrap();
    assert_eq!(name, Value::Json(serde_json::Value::String("Alice".into())));

    let score = call(&can, &k, "json.get", &[parsed, Value::text("scores[1]")]).unwrap();
    assert_eq!(score, Value::Json(serde_json::Value::Number(20.into())));
}

#[test]
fn test_json_stringify_roundtrip() {
    let (k, can) = setup();
    let original = r#"{"x":1}"#;
    let parsed = call(&can, &k, "json.parse", &[Value::text(original)]).unwrap();
    let stringified = call(&can, &k, "json.stringify", &[parsed]).unwrap();
    assert!(matches!(stringified, Value::Text(_)));
}

#[test]
fn test_json_keys_and_has() {
    let (k, can) = setup();
    let j = Value::Json(serde_json::json!({"a": 1, "b": 2}));
    let keys = call(&can, &k, "json.keys", &[j.clone()]).unwrap();
    if let Value::List(ks) = keys {
        let strs: Vec<String> = ks.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        assert!(strs.contains(&"a".to_string()));
        assert!(strs.contains(&"b".to_string()));
    } else {
        panic!("expected list");
    }
    let has = call(&can, &k, "json.has", &[j, Value::text("a")]).unwrap();
    assert_eq!(has, Value::Bool(true));
}

// ---- Program-impl action calls another Program-impl action ------------

#[test]
fn test_program_calling_program() {
    let (k, mut can) = setup();

    // "double" = program(n: Int) -> mul(n, 2)
    let double_prog = Program::new(
        vec![Type::Int],
        Type::Int,
        Expr::Call {
            action: ActionId("math.mul".into()),
            args: vec![Expr::Param { index: 0 }, Expr::Const { value: Value::Int(2) }],
        },
    );
    let double_action = Action {
        id: ActionId("test.double".into()),
        inputs: vec![Input::required("n", Type::Int)],
        output: Type::Int,
        effect: Effect::Pure,
        imp: Impl::Program { program: double_prog },
        role: Role::Function,
        verbs: vec!["double".into()],
        phrasings: vec![],
        description: "double a number".into(),
        tier: Tier::Provisional,
        provenance: Provenance::Kernel,
        stats: Stats::default(),
    };
    can.add_action(double_action);

    // "quadruple" = program(n: Int) -> double(double(n))
    let quadruple_prog = Program::new(
        vec![Type::Int],
        Type::Int,
        Expr::Call {
            action: ActionId("test.double".into()),
            args: vec![Expr::Call {
                action: ActionId("test.double".into()),
                args: vec![Expr::Param { index: 0 }],
            }],
        },
    );
    let mut c = ctx(&can, &k);
    let result = eval_program(&mut c, &quadruple_prog, &[Value::Int(5)]).unwrap();
    assert_eq!(result, Value::Int(20));
}

// ---- budget exhaustion -------------------------------------------------

#[test]
fn test_budget_exhaustion() {
    let (k, can) = setup();
    // range(0, 100_000) then map each element through math.add - exhausts budget.
    let body = Expr::Call {
        action: ActionId("list.map".into()),
        args: vec![
            Expr::Call {
                action: ActionId("math.range".into()),
                args: vec![
                    Expr::Const { value: Value::Int(0) },
                    Expr::Const { value: Value::Int(100_000) },
                ],
            },
            Expr::Lambda {
                lambda: Box::new(Lambda {
                    params: vec![Type::Any],
                    ret: Type::Any,
                    body: Expr::Call {
                        action: ActionId("math.add".into()),
                        args: vec![
                            Expr::LambdaParam { index: 0 },
                            Expr::Const { value: Value::Int(1) },
                        ],
                    },
                }),
            },
        ],
    };
    let program = prog(vec![], Type::list(Type::Int), body);
    let mut c = Ctx {
        can: &can,
        kernel: &k,
        host: &NoHost,
        budget: Budget::tiny(),
        max_effect: Effect::Shell,
    };
    let result = eval_program(&mut c, &program, &[]);
    assert!(matches!(result, Err(EvalError::Budget(_))));
}

// ---- permission enforcement --------------------------------------------

#[test]
fn test_pure_ctx_blocks_fs_read() {
    let (k, can) = setup();
    let mut c = Ctx::pure(&can, &k, &NoHost, Budget::generous());
    let result = call_action(&mut c, &ActionId("fs.read".into()), &[Value::Path("/tmp/test.txt".into())]);
    assert!(matches!(result, Err(EvalError::Permission { .. })));
}

// ---- fs round-trip -----------------------------------------------------

#[test]
fn test_fs_write_read_roundtrip() {
    let (k, can) = setup();
    let path = format!("/tmp/spoon_test_{}.txt", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());

    // Write
    let write_result = call(&can, &k, "fs.write", &[Value::Path(path.clone()), Value::text("hello spoon")]).unwrap();
    assert_eq!(write_result, Value::Bool(true));

    // Read back
    let read_result = call(&can, &k, "fs.read", &[Value::Path(path.clone())]).unwrap();
    assert_eq!(read_result, Value::text("hello spoon"));

    // Clean up
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_sandbox_denies_etc_passwd() {
    let (k, can) = setup();
    let result = call(&can, &k, "fs.read", &[Value::Path("/etc/passwd".into())]);
    assert!(
        matches!(&result, Err(EvalError::Runtime { message, .. }) if message.contains("not allowed")),
        "expected path-not-allowed error, got {:?}",
        result
    );
}

// ---- shell -------------------------------------------------------------

#[test]
fn test_shell_run_echo() {
    let (k, can) = setup();
    let result = call(&can, &k, "shell.run", &[Value::text("echo hi")]).unwrap();
    if let Value::Text(s) = result {
        assert!(s.contains("hi"), "expected 'hi' in output, got: {s}");
    } else {
        panic!("expected Text");
    }
}

// ---- dialog ------------------------------------------------------------

#[test]
fn test_dialog_greet_deserializes() {
    let (k, can) = setup();
    let result = call(&can, &k, "dialog.greet", &[Value::Bool(false)]).unwrap();
    if let Value::Json(j) = result {
        let m: Move = serde_json::from_value(j).expect("should deserialize to Move");
        assert!(matches!(m, Move::Greet { returning: false }));
    } else {
        panic!("expected Json");
    }
}

#[test]
fn test_dialog_ack_and_refuse() {
    let (k, can) = setup();
    let ack = call(&can, &k, "dialog.ack", &[Value::text("stored the fact")]).unwrap();
    if let Value::Json(j) = ack {
        let m: Move = serde_json::from_value(j).unwrap();
        assert!(matches!(m, Move::Ack { .. }));
    }
    let refuse = call(&can, &k, "dialog.refuse", &[Value::text("out of scope")]).unwrap();
    if let Value::Json(j) = refuse {
        let m: Move = serde_json::from_value(j).unwrap();
        assert!(matches!(m, Move::Refuse { .. }));
    }
}

// ---- math / logic meta -------------------------------------------------

#[test]
fn test_math_div_by_zero() {
    let (k, can) = setup();
    let result = call(&can, &k, "math.div", &[Value::Float(5.0), Value::Float(0.0)]);
    assert!(matches!(result, Err(EvalError::Runtime { .. })));
}

#[test]
fn test_math_range_and_sum() {
    let (k, can) = setup();
    let r = call(&can, &k, "math.range", &[Value::Int(1), Value::Int(6)]).unwrap();
    assert_eq!(r, Value::List((1..6).map(Value::Int).collect()));

    let s = call(&can, &k, "math.sum", &[r]).unwrap();
    assert_eq!(s, Value::Float(15.0));
}

#[test]
fn test_logic_and_or_not() {
    let (k, can) = setup();
    assert_eq!(call(&can, &k, "logic.and", &[Value::Bool(true), Value::Bool(false)]).unwrap(), Value::Bool(false));
    assert_eq!(call(&can, &k, "logic.or",  &[Value::Bool(false), Value::Bool(true)]).unwrap(), Value::Bool(true));
    assert_eq!(call(&can, &k, "logic.not", &[Value::Bool(false)]).unwrap(), Value::Bool(true));
}

// ---- time --------------------------------------------------------------

#[test]
fn test_time_now_and_format() {
    let (k, can) = setup();
    let now = call(&can, &k, "time.now", &[]).unwrap();
    assert!(matches!(now, Value::DateTime(_)));
    let formatted = call(&can, &k, "time.format", &[now, Value::text("%Y")]).unwrap();
    assert!(matches!(formatted, Value::Text(s) if s.len() == 4));
}

#[test]
fn test_time_parse_and_components() {
    let (k, can) = setup();
    let dt = call(&can, &k, "time.parse_datetime", &[Value::text("2024-06-15T12:30:00Z")]).unwrap();
    assert_eq!(call(&can, &k, "time.year",  &[dt.clone()]).unwrap(), Value::Int(2024));
    assert_eq!(call(&can, &k, "time.month", &[dt.clone()]).unwrap(), Value::Int(6));
    assert_eq!(call(&can, &k, "time.day",   &[dt.clone()]).unwrap(), Value::Int(15));
    assert_eq!(call(&can, &k, "time.hour",  &[dt.clone()]).unwrap(), Value::Int(12));
}

#[test]
fn test_duration_arithmetic() {
    let (k, can) = setup();
    let d = call(&can, &k, "time.duration_hours", &[Value::Float(24.0)]).unwrap();
    let days = call(&can, &k, "time.in_days", &[d]).unwrap();
    assert_eq!(days, Value::Float(1.0));
}

// ---- value primitives --------------------------------------------------

#[test]
fn test_value_eq_and_type_of() {
    let (k, can) = setup();
    assert_eq!(call(&can, &k, "value.eq", &[Value::Int(3), Value::Float(3.0)]).unwrap(), Value::Bool(true));
    assert_eq!(call(&can, &k, "value.is_null", &[Value::Null]).unwrap(), Value::Bool(true));
    assert_eq!(call(&can, &k, "value.is_null", &[Value::Int(0)]).unwrap(), Value::Bool(false));
    let ty = call(&can, &k, "value.type_of", &[Value::Bool(true)]).unwrap();
    assert_eq!(ty, Value::text("bool"));
}

// ---- meta invariants ---------------------------------------------------

fn dummy_for_type(ty: &Type) -> Value {
    match ty {
        Type::Null => Value::Null,
        Type::Bool => Value::Bool(false),
        Type::Int => Value::Int(0),
        Type::Float => Value::Float(0.0),
        Type::Text | Type::Name => Value::Text("test".into()),
        Type::DateTime => Value::DateTime(0),
        Type::Duration => Value::Duration(0.0),
        Type::Path => Value::Path("/tmp/spoon_dummy".into()),
        Type::Url => Value::Url("http://example.com".into()),
        Type::Json => Value::Json(serde_json::Value::Null),
        Type::List(_) => Value::List(vec![]),
        Type::Func(params, ret) => Value::Lambda(Box::new(Lambda {
            params: params.clone(),
            ret: *ret.clone(),
            body: Expr::Const { value: Value::Null },
        })),
        Type::Any => Value::Null,
        Type::Concept(_) => Value::Null,
    }
}

#[test]
fn test_all_actions_have_verb_and_description() {
    let k = Kernel::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    for action in k.actions() {
        assert!(
            !action.verbs.is_empty(),
            "action {} has no verbs",
            action.id.0
        );
        assert!(
            !action.description.is_empty(),
            "action {} has empty description",
            action.id.0
        );
        assert!(
            seen_ids.insert(action.id.0.clone()),
            "duplicate action id: {}",
            action.id.0
        );
    }
}

#[test]
fn test_pure_primitives_arity_matches_registration() {
    let k = Kernel::new();
    let mut can = Can::new();
    for c in k.concepts() {
        can.add_concept(c.clone());
    }
    for a in k.actions() {
        can.add_action(a.clone());
    }

    let pure_actions: Vec<Action> = k.actions()
        .iter()
        .filter(|a| a.effect == Effect::Pure && matches!(a.imp, Impl::Primitive))
        .cloned()
        .collect();

    for action in &pure_actions {
        let dummy_args: Vec<Value> = action.inputs.iter()
            .map(|i| dummy_for_type(&i.ty))
            .collect();
        let mut c = Ctx::new(&can, &k, &NoHost);
        let result = call_action(&mut c, &action.id, &dummy_args);
        // Must NOT be an Arity error - any other error (type mismatch, runtime, etc.) is acceptable.
        assert!(
            !matches!(result, Err(EvalError::Arity { .. })),
            "action {} returned Arity error with {} dummy args",
            action.id.0,
            dummy_args.len()
        );
    }
}

#[test]
fn test_arity_error_on_wrong_arg_count() {
    let (k, can) = setup();
    let result = call(&can, &k, "math.add", &[Value::Int(1)]);
    assert!(matches!(result, Err(EvalError::Arity { .. })));
}

#[test]
fn test_list_sort_and_unique() {
    let (k, can) = setup();
    let list = Value::List(vec![Value::Int(3), Value::Int(1), Value::Int(2), Value::Int(1)]);
    let sorted = call(&can, &k, "list.sort", &[list]).unwrap();
    assert_eq!(sorted, Value::List(vec![Value::Int(1), Value::Int(1), Value::Int(2), Value::Int(3)]));

    let list2 = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(1)]);
    let unique = call(&can, &k, "list.unique", &[list2]).unwrap();
    assert_eq!(unique, Value::List(vec![Value::Int(1), Value::Int(2)]));
}

#[test]
fn test_list_zip_and_enumerate() {
    let (k, can) = setup();
    let a = Value::List(vec![Value::text("x"), Value::text("y")]);
    let b = Value::List(vec![Value::Int(1), Value::Int(2)]);
    let zipped = call(&can, &k, "list.zip", &[a, b]).unwrap();
    assert_eq!(zipped, Value::List(vec![
        Value::List(vec![Value::text("x"), Value::Int(1)]),
        Value::List(vec![Value::text("y"), Value::Int(2)]),
    ]));

    let list = Value::List(vec![Value::text("a"), Value::text("b")]);
    let enum_result = call(&can, &k, "list.enumerate", &[list]).unwrap();
    assert_eq!(enum_result, Value::List(vec![
        Value::List(vec![Value::Int(0), Value::text("a")]),
        Value::List(vec![Value::Int(1), Value::text("b")]),
    ]));
}

#[test]
fn test_text_title_case_and_regex_replace() {
    let (k, can) = setup();
    let tc = call(&can, &k, "text.title_case", &[Value::text("hello world")]).unwrap();
    assert_eq!(tc, Value::text("Hello World"));

    let replaced = call(&can, &k, "text.regex_replace", &[
        Value::text("abc 123"),
        Value::text(r"\d+"),
        Value::text("NUM"),
    ]).unwrap();
    assert_eq!(replaced, Value::text("abc NUM"));
}

#[test]
fn test_struct_and_field_expr() {
    let (k, can) = setup();
    let program = prog(
        vec![],
        Type::Any,
        Expr::Field {
            of: Box::new(Expr::Struct {
                concept: ConceptId("Thing".into()),
                fields: vec![("name".to_string(), Expr::Const { value: Value::text("Alice") })],
            }),
            name: "name".to_string(),
        },
    );
    let mut c = ctx(&can, &k);
    let result = eval_program(&mut c, &program, &[]).unwrap();
    assert_eq!(result, Value::text("Alice"));
}

#[test]
fn test_json_field_expr() {
    // Field expr on Json value returns Value::Json of the field
    let (k, can) = setup();
    let j = Value::Json(serde_json::json!({"color": "blue"}));
    let program = prog(
        vec![],
        Type::Any,
        Expr::Field {
            of: Box::new(Expr::Const { value: j }),
            name: "color".to_string(),
        },
    );
    let mut c = ctx(&can, &k);
    let result = eval_program(&mut c, &program, &[]).unwrap();
    assert_eq!(result, Value::Json(serde_json::Value::String("blue".into())));
}

// ---- network tests (skipped unless SPOON_NET_TESTS=1) ------------------

#[test]
fn test_wikidata_search_skipped_unless_enabled() {
    if std::env::var("SPOON_NET_TESTS").as_deref() != Ok("1") {
        return;
    }
    let (k, can) = setup();
    let result = call(&can, &k, "know.wikidata_search", &[Value::Name("Douglas Adams".into())]).unwrap();
    assert!(matches!(result, Value::Json(serde_json::Value::Array(_))));
}
