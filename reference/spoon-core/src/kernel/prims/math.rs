//! Arithmetic and comparison primitives (math.*) and boolean logic (logic.*).
//! All are Effect::Pure.

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};

pub fn register(k: &mut Kernel) {
    use Effect::Pure;
    use Role::Function;

    macro_rules! p {
        ($id:expr, $verbs:expr, $inputs:expr, $out:expr, $desc:expr, $f:expr) => {
            k.register(
                Action::primitive($id, $verbs, $inputs, $out, Pure, $desc).with_role(Function),
                $f,
            )
        };
    }
    macro_rules! cmd {
        ($id:expr, $verbs:expr, $inputs:expr, $out:expr, $desc:expr, $f:expr) => {
            k.register(Action::primitive($id, $verbs, $inputs, $out, Pure, $desc), $f)
        };
    }

    let nn = || vec![Input::required("a", Type::Float), Input::required("b", Type::Float)];
    let n1 = || vec![Input::required("n", Type::Float)];

    p!("math.add",  &["add", "plus"],              nn(), Type::Float,  "add two numbers",                     add);
    p!("math.sub",  &["subtract", "minus", "deduct"], nn(), Type::Float, "subtract b from a",                sub);
    p!("math.mul",  &["multiply", "times"],        nn(), Type::Float,  "multiply two numbers",                mul);
    p!("math.div",  &["divide"],                   nn(), Type::Float,  "divide a by b",                       div);
    p!("math.mod",  &["mod"],                      nn(), Type::Int,    "remainder of a divided by b",         modulo);
    p!("math.pow",  &["power", "raise"],           nn(), Type::Float,  "raise a to the power b",              pow);
    p!("math.neg",  &["negate"],                   n1(), Type::Float,  "negate a number",                     neg);
    p!("math.abs",  &["absolute-value"],           n1(), Type::Float,  "absolute value of a number",          abs);
    p!(
        "math.min",
        &["min"],
        nn(),
        Type::Float,
        "smaller of two numbers",
        min
    );
    p!(
        "math.max",
        &["max"],
        nn(),
        Type::Float,
        "larger of two numbers",
        max
    );
    p!("math.round", &["round"],                   n1(), Type::Float,  "round to nearest integer",            round);
    p!("math.floor", &["floor"],                   n1(), Type::Float,  "round down to integer",               floor);
    p!("math.ceil",  &["ceil", "ceiling"],         n1(), Type::Float,  "round up to integer",                 ceil);
    p!("math.sqrt",  &["sqrt", "square-root"],     n1(), Type::Float,  "square root",                        sqrt);

    p!(
        "math.sum",
        &["sum"],
        vec![Input::required("list", Type::list(Type::Float))],
        Type::Float,
        "sum of a list of numbers",
        sum
    );
    p!(
        "math.avg",
        &["average", "mean"],
        vec![Input::required("list", Type::list(Type::Float))],
        Type::Float,
        "arithmetic mean of a list of numbers",
        avg
    );
    p!(
        "math.product",
        &["product"],
        vec![Input::required("list", Type::list(Type::Float))],
        Type::Float,
        "product of a list of numbers",
        product
    );
    p!(
        "math.range",
        &["range"],
        vec![Input::required("start", Type::Int), Input::required("end", Type::Int)],
        Type::list(Type::Int),
        "half-open integer range [start, end)",
        range
    );

    // Comparisons - declared as (Float, Float) but eq/ne also work on any Values.
    cmd!("math.eq",  &["eq", "equal"],       vec![Input::required("a", Type::Any), Input::required("b", Type::Any)], Type::Bool, "equality of two values (numeric-aware)", eq);
    cmd!("math.ne",  &["ne", "not-equal"],   vec![Input::required("a", Type::Any), Input::required("b", Type::Any)], Type::Bool, "inequality of two values", ne);
    cmd!("math.lt",  &["lt", "less-than"],   nn(), Type::Bool, "a is less than b",                lt);
    cmd!("math.gt",  &["gt", "greater-than"], nn(), Type::Bool, "a is greater than b",            gt);
    cmd!("math.le",  &["le", "at-most"],     nn(), Type::Bool, "a is less than or equal to b",   le);
    cmd!("math.ge",  &["ge", "at-least"],    nn(), Type::Bool, "a is greater than or equal to b", ge);

    // Conversions
    p!(
        "math.to_float",
        &["to-float"],
        vec![Input::required("n", Type::Any)],
        Type::Float,
        "convert to float",
        to_float
    );
    p!(
        "math.to_int",
        &["to-int", "truncate"],
        vec![Input::required("n", Type::Any)],
        Type::Int,
        "truncate to integer",
        to_int
    );
    p!(
        "math.parse_number",
        &["parse-number"],
        vec![Input::required("text", Type::Text)],
        Type::Float,
        "parse a number from text",
        parse_number
    );

    // Logic
    let b2 = || vec![Input::required("a", Type::Bool), Input::required("b", Type::Bool)];
    let b1 = || vec![Input::required("a", Type::Bool)];
    k.register(Action::primitive("logic.and", &["and"], b2(), Type::Bool, Pure, "logical AND"), logic_and);
    k.register(Action::primitive("logic.or",  &["or"],  b2(), Type::Bool, Pure, "logical OR"),  logic_or);
    k.register(Action::primitive("logic.not", &["not"], b1(), Type::Bool, Pure, "logical NOT"), logic_not);
}

// ---- numeric helpers ---------------------------------------------------

fn num(id: &str, args: &[Value], i: usize) -> Result<f64, EvalError> {
    args.get(i)
        .and_then(Value::as_f64)
        .ok_or_else(|| EvalError::ty("number", args.get(i).unwrap_or(&Value::Null), id))
}

fn int_arg(id: &str, args: &[Value], i: usize) -> Result<i64, EvalError> {
    args.get(i)
        .and_then(Value::as_int)
        .ok_or_else(|| EvalError::ty("int", args.get(i).unwrap_or(&Value::Null), id))
}

/// Promote float result to Int when both inputs were integers and the result
/// is whole.
fn promote(a: &Value, b: &Value, v: f64) -> Value {
    let both_int = matches!(a, Value::Int(_)) && matches!(b, Value::Int(_));
    if both_int && v.fract() == 0.0 && v.abs() < i64::MAX as f64 {
        Value::Int(v as i64)
    } else {
        Value::Float(v)
    }
}

// ---- primitives --------------------------------------------------------

fn add(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = num("math.add", args, 0)?;
    let b = num("math.add", args, 1)?;
    Ok(promote(&args[0], &args[1], a + b))
}
fn sub(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = num("math.sub", args, 0)?;
    let b = num("math.sub", args, 1)?;
    Ok(promote(&args[0], &args[1], a - b))
}
fn mul(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = num("math.mul", args, 0)?;
    let b = num("math.mul", args, 1)?;
    Ok(promote(&args[0], &args[1], a * b))
}
fn div(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("math.div".into());
    let a = num("math.div", args, 0)?;
    let b = num("math.div", args, 1)?;
    if b == 0.0 {
        return Err(EvalError::runtime(&id, "division by zero"));
    }
    Ok(Value::Float(a / b))
}
fn modulo(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("math.mod".into());
    let a = int_arg("math.mod", args, 0)?;
    let b = int_arg("math.mod", args, 1)?;
    if b == 0 {
        return Err(EvalError::runtime(&id, "division by zero"));
    }
    Ok(Value::Int(a % b))
}
fn pow(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = num("math.pow", args, 0)?;
    let b = num("math.pow", args, 1)?;
    Ok(Value::Float(a.powf(b)))
}
fn neg(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let n = num("math.neg", args, 0)?;
    match args.first() {
        Some(Value::Int(i)) => Ok(Value::Int(-i)),
        _ => Ok(Value::Float(-n)),
    }
}
fn abs(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let n = num("math.abs", args, 0)?;
    match args.first() {
        Some(Value::Int(i)) => Ok(Value::Int(i.abs())),
        _ => Ok(Value::Float(n.abs())),
    }
}
fn min(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = num("math.min", args, 0)?;
    let b = num("math.min", args, 1)?;
    Ok(promote(&args[0], &args[1], a.min(b)))
}
fn max(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = num("math.max", args, 0)?;
    let b = num("math.max", args, 1)?;
    Ok(promote(&args[0], &args[1], a.max(b)))
}
fn round(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let n = num("math.round", args, 0)?;
    Ok(Value::Int(n.round() as i64))
}
fn floor(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let n = num("math.floor", args, 0)?;
    Ok(Value::Int(n.floor() as i64))
}
fn ceil(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let n = num("math.ceil", args, 0)?;
    Ok(Value::Int(n.ceil() as i64))
}
fn sqrt(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("math.sqrt".into());
    let n = num("math.sqrt", args, 0)?;
    if n < 0.0 {
        return Err(EvalError::runtime(&id, "sqrt of negative number"));
    }
    Ok(Value::Float(n.sqrt()))
}

fn sum(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("math.sum".into());
    let list = args.first().and_then(Value::as_list).ok_or_else(|| EvalError::ty("list", args.first().unwrap_or(&Value::Null), "math.sum"))?;
    let mut acc = 0.0f64;
    for v in list {
        acc += v.as_f64().ok_or_else(|| EvalError::runtime(&id, "list contains non-number"))?;
    }
    Ok(Value::Float(acc))
}

fn avg(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("math.avg".into());
    let list = args.first().and_then(Value::as_list).ok_or_else(|| EvalError::ty("list", args.first().unwrap_or(&Value::Null), "math.avg"))?.to_vec();
    if list.is_empty() {
        return Err(EvalError::runtime(&id, "average of empty list"));
    }
    let mut acc = 0.0f64;
    for v in &list {
        acc += v.as_f64().ok_or_else(|| EvalError::runtime(&id, "list contains non-number"))?;
    }
    Ok(Value::Float(acc / list.len() as f64))
}

fn product(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("math.product".into());
    let list = args.first().and_then(Value::as_list).ok_or_else(|| EvalError::ty("list", args.first().unwrap_or(&Value::Null), "math.product"))?;
    let mut acc = 1.0f64;
    for v in list {
        acc *= v.as_f64().ok_or_else(|| EvalError::runtime(&id, "list contains non-number"))?;
    }
    Ok(Value::Float(acc))
}

fn range(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("math.range".into());
    let start = int_arg("math.range", args, 0)?;
    let end = int_arg("math.range", args, 1)?;
    if end < start {
        return Ok(Value::List(vec![]));
    }
    let len = (end - start) as usize;
    if len > 10_000_000 {
        return Err(EvalError::runtime(&id, "range too large"));
    }
    Ok(Value::List((start..end).map(Value::Int).collect()))
}

fn eq(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let (a, b) = match args {
        [a, b, ..] => (a, b),
        _ => return Err(EvalError::Arity { action: ActionId("math.eq".into()), expected: 2, got: args.len() }),
    };
    let eq = match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    };
    Ok(Value::Bool(eq))
}
fn ne(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    if let Ok(Value::Bool(b)) = eq(_ctx, args) { Ok(Value::Bool(!b)) } else { eq(_ctx, args) }
}
fn lt(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = num("math.lt", args, 0)?;
    let b = num("math.lt", args, 1)?;
    Ok(Value::Bool(a < b))
}
fn gt(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = num("math.gt", args, 0)?;
    let b = num("math.gt", args, 1)?;
    Ok(Value::Bool(a > b))
}
fn le(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = num("math.le", args, 0)?;
    let b = num("math.le", args, 1)?;
    Ok(Value::Bool(a <= b))
}
fn ge(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = num("math.ge", args, 0)?;
    let b = num("math.ge", args, 1)?;
    Ok(Value::Bool(a >= b))
}

fn to_float(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let v = args.first().ok_or_else(|| EvalError::Arity { action: ActionId("math.to_float".into()), expected: 1, got: 0 })?;
    match v.as_f64() {
        Some(f) => Ok(Value::Float(f)),
        None => {
            if let Some(s) = v.as_str() {
                s.parse::<f64>().map(Value::Float)
                    .map_err(|_| EvalError::runtime(&ActionId("math.to_float".into()), format!("cannot convert to float: {s}")))
            } else {
                Err(EvalError::ty("number or text", v, "math.to_float"))
            }
        }
    }
}

fn to_int(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let v = args.first().ok_or_else(|| EvalError::Arity { action: ActionId("math.to_int".into()), expected: 1, got: 0 })?;
    match v {
        Value::Int(i) => Ok(Value::Int(*i)),
        _ => match v.as_f64() {
            Some(f) => Ok(Value::Int(f as i64)),
            None => {
                if let Some(s) = v.as_str() {
                    s.parse::<i64>().map(Value::Int)
                        .or_else(|_| s.parse::<f64>().map(|f| Value::Int(f as i64)))
                        .map_err(|_| EvalError::runtime(&ActionId("math.to_int".into()), format!("cannot convert to int: {s}")))
                } else {
                    Err(EvalError::ty("number or text", v, "math.to_int"))
                }
            }
        },
    }
}

fn parse_number(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("math.parse_number".into());
    let s = args.first().and_then(Value::as_str).ok_or_else(|| EvalError::ty("text", args.first().unwrap_or(&Value::Null), "math.parse_number"))?;
    s.trim().parse::<f64>()
        .map(Value::Float)
        .map_err(|_| EvalError::runtime(&id, format!("cannot parse number from: {s}")))
}

fn logic_and(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = bool_arg("logic.and", args, 0)?;
    let b = bool_arg("logic.and", args, 1)?;
    Ok(Value::Bool(a && b))
}
fn logic_or(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = bool_arg("logic.or", args, 0)?;
    let b = bool_arg("logic.or", args, 1)?;
    Ok(Value::Bool(a || b))
}
fn logic_not(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = bool_arg("logic.not", args, 0)?;
    Ok(Value::Bool(!a))
}

fn bool_arg(id: &str, args: &[Value], i: usize) -> Result<bool, EvalError> {
    args.get(i)
        .and_then(Value::as_bool)
        .ok_or_else(|| EvalError::ty("bool", args.get(i).unwrap_or(&Value::Null), id))
}
