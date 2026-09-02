//! Reflective value primitives: type inspection, coercion, equality, nullability.

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};

pub fn register(k: &mut Kernel) {
    use Effect::Pure;

    macro_rules! p {
        ($id:expr, $verbs:expr, $inputs:expr, $out:expr, $eff:expr, $desc:expr, $f:expr) => {
            k.register(Action::primitive($id, $verbs, $inputs, $out, $eff, $desc), $f)
        };
    }

    p!(
        "value.to_text",
        &["render"],
        vec![Input::required("value", Type::Any)],
        Type::Text,
        Pure,
        "render any value as human-readable text",
        to_text
    );
    p!(
        "value.eq",
        &["equal", "equals"],
        vec![Input::required("a", Type::Any), Input::required("b", Type::Any)],
        Type::Bool,
        Pure,
        "structural equality of any two values",
        val_eq
    );
    p!(
        "value.type_of",
        &["type-of"],
        vec![Input::required("value", Type::Any)],
        Type::Text,
        Pure,
        "the type name of a value as text",
        type_of
    );
    p!(
        "value.is_null",
        &["is-null"],
        vec![Input::required("value", Type::Any)],
        Type::Bool,
        Pure,
        "true if the value is null",
        is_null
    );
    p!(
        "value.default",
        &["default"],
        vec![Input::required("value", Type::Any), Input::required("fallback", Type::Any)],
        Type::Any,
        Pure,
        "return value unless it is null, then return fallback",
        val_default
    );
}

fn to_text(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let v = args.first().ok_or_else(|| EvalError::Arity {
        action: ActionId("value.to_text".into()),
        expected: 1,
        got: 0,
    })?;
    Ok(Value::Text(v.render()))
}

fn val_eq(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let (a, b) = two_args("value.eq", args)?;
    // Numeric promotion: Int and Float compare by number.
    let eq = match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    };
    Ok(Value::Bool(eq))
}

fn type_of(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let v = args.first().ok_or_else(|| EvalError::Arity {
        action: ActionId("value.type_of".into()),
        expected: 1,
        got: 0,
    })?;
    Ok(Value::Text(v.type_of().to_string()))
}

fn is_null(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let v = args.first().ok_or_else(|| EvalError::Arity {
        action: ActionId("value.is_null".into()),
        expected: 1,
        got: 0,
    })?;
    Ok(Value::Bool(matches!(v, Value::Null)))
}

fn val_default(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let (a, b) = two_args("value.default", args)?;
    if matches!(a, Value::Null) { Ok(b.clone()) } else { Ok(a.clone()) }
}

// ---- helpers -----------------------------------------------------------

fn two_args<'a>(id: &str, args: &'a [Value]) -> Result<(&'a Value, &'a Value), EvalError> {
    match args {
        [a, b, ..] => Ok((a, b)),
        _ => Err(EvalError::Arity { action: ActionId(id.into()), expected: 2, got: args.len() }),
    }
}
