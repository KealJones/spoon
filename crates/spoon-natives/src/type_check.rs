//! Runtime type inspection and coercion.
//!
//! Spoon has no static type system: a concept's shape is discovered by asking
//! it, not declared ahead of time. These natives are that asking. `type-of`
//! names the shape, `type-is-*` answers a yes/no about one shape, and
//! `type-to-*` / `type-coerce` cross from one ground shape to another when the
//! crossing has an honest answer.
//!
//! Lists are `List<..>` compounds, not a `Ground` variant, so `type-of` and
//! `type-is-list` check the compound head the same way `collections.rs` does
//! rather than reaching into `Ground`. A named or otherwise-headed compound is
//! reported as `"concept"`: it is not ground, and it is not a list either.

use spoon_concept::{Concept, Ground, SymbolId};
use spoon_eval::{Arity, Ctx, EvalResult, NativeRegistry, native_error, type_error};

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

fn is_list(c: &Concept) -> bool {
    c.head_symbol() == Some(SymbolId::of("list-of"))
        || matches!(c.as_ground(), Some(Ground::Json(blob)) if blob.value().is_array())
}

/// The type name Spoon uses for a concept. See the module docs for the table.
fn type_name(c: &Concept) -> &'static str {
    if is_list(c) {
        return "list";
    }
    match c.as_ground() {
        Some(Ground::Text(_)) => "text",
        Some(Ground::Int(_)) => "int",
        Some(Ground::Float(_)) => "float",
        Some(Ground::Bool(_)) => "bool",
        Some(Ground::Json(_)) => "json",
        Some(Ground::Bytes(_)) | Some(Ground::DateTime(_)) => "json",
        None => "concept",
    }
}

fn type_of(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::text(type_name(&args[0])))
}

fn is_text(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(matches!(args[0].as_ground(), Some(Ground::Text(_)))))
}

fn is_number(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(matches!(
        args[0].as_ground(),
        Some(Ground::Int(_)) | Some(Ground::Float(_))
    )))
}

fn is_list_native(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(is_list(&args[0])))
}

fn is_bool(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(matches!(args[0].as_ground(), Some(Ground::Bool(_)))))
}

/// True for a named or compound concept that carries meaning through the
/// store rather than through its own content: not ground, and not a list.
fn is_concept(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(!args[0].is_ground() && !is_list(&args[0])))
}

// ---------------------------------------------------------------------------
// Coercion
// ---------------------------------------------------------------------------

/// Parse text into a number. Integers parse as `Int`; anything with a decimal
/// point or exponent, or that does not fit an `i64`, parses as `Float`. There
/// is no silent zero on failure: unparseable input is an error, the same rule
/// `text-parse-int` and `text-parse-float` already follow.
fn to_number(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = args[0]
        .as_ground()
        .and_then(Ground::as_str)
        .ok_or_else(|| type_error("type-to-number", "a text value", &args[0]))?;
    let trimmed = text.trim();
    if let Ok(i) = trimmed.parse::<i64>() {
        return Ok(Concept::int(i));
    }
    trimmed
        .parse::<f64>()
        .map(Concept::float)
        .map_err(|e| native_error("type-to-number", format!("{text:?} is not a number: {e}")))
}

/// Truthiness by coercion rather than by identity. `0`, `0.0`, `""`, and the
/// text `"false"` (case-insensitively) are false; every other ground value,
/// including an empty list, is true.
///
/// An empty list is deliberately true: it already has an honest, unambiguous
/// answer to "is it empty" through `list-is-empty`, so overloading truthiness
/// to mean the same thing would give two names for one question and no name
/// for "is this concept present at all", which is what this native actually
/// answers.
fn to_bool(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let truthy = match args[0].as_ground() {
        Some(Ground::Bool(b)) => *b,
        Some(Ground::Int(i)) => *i != 0,
        Some(Ground::Float(f)) => *f != 0.0,
        Some(Ground::Text(t)) => {
            let trimmed = t.trim();
            !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("false")
        }
        _ => true,
    };
    Ok(Concept::bool(truthy))
}

/// Truncation toward zero, matching Rust's `as` cast, because "truncate" means
/// drop the fraction rather than round it.
fn to_int(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    match args[0].as_ground() {
        Some(Ground::Int(i)) => Ok(Concept::int(*i)),
        Some(Ground::Float(f)) => Ok(Concept::int(*f as i64)),
        _ => Err(type_error("type-to-int", "a number", &args[0])),
    }
}

fn to_float(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    match args[0].as_ground() {
        Some(Ground::Int(i)) => Ok(Concept::float(*i as f64)),
        Some(Ground::Float(f)) => Ok(Concept::float(*f)),
        _ => Err(type_error("type-to-float", "a number", &args[0])),
    }
}

/// Coerce a value to a named target type. This is the general form; the
/// dedicated `type-to-*` natives exist because "coerce to int" is common
/// enough to deserve a name that does not require spelling out the target.
fn coerce(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let target = args[0 + 1]
        .as_ground()
        .and_then(Ground::as_str)
        .ok_or_else(|| type_error("type-coerce", "a text type name", &args[1]))?;
    match target {
        "text" => to_text(ctx, args),
        "int" => to_int(ctx, &[args[0].clone()]),
        "float" => to_float(ctx, &[args[0].clone()]),
        "bool" => to_bool(ctx, &[args[0].clone()]),
        "number" => {
            if let Some(t) = args[0].as_ground().and_then(Ground::as_str) {
                return to_number(ctx, &[Concept::text(t)]);
            }
            to_float(ctx, &[args[0].clone()])
        }
        other => Err(native_error(
            "type-coerce",
            format!("no coercion to {other:?}; try text, int, float, bool, or number"),
        )),
    }
}

/// Render any ground value as text, for `type-coerce`'s `"text"` target.
/// Mirrors `text-to-text` without depending on `text.rs`.
fn to_text(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let ground = args[0]
        .as_ground()
        .ok_or_else(|| type_error("type-coerce", "a ground value", &args[0]))?;
    let out = match ground {
        Ground::Bool(b) => b.to_string(),
        Ground::Int(i) => i.to_string(),
        Ground::Float(f) => f.to_string(),
        Ground::Text(t) => t.to_string(),
        Ground::Bytes(b) => b.iter().map(|byte| format!("{byte:02x}")).collect(),
        Ground::DateTime(d) => d.to_rfc3339(),
        Ground::Json(j) => serde_json::to_string(j.value())
            .map_err(|e| native_error("type-coerce", format!("json will not render: {e}")))?,
    };
    Ok(Concept::text(out))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register(registry: &mut NativeRegistry) {
    registry.pure(
        "type-of",
        type_of,
        Arity::Exact(1),
        "the type name of a concept: text, int, float, bool, list, json, or concept",
    );
    registry.pure(
        "type-is-text",
        is_text,
        Arity::Exact(1),
        "whether a concept is text",
    );
    registry.pure(
        "type-is-number",
        is_number,
        Arity::Exact(1),
        "whether a concept is an int or a float",
    );
    registry.pure(
        "type-is-list",
        is_list_native,
        Arity::Exact(1),
        "whether a concept is a list",
    );
    registry.pure(
        "type-is-bool",
        is_bool,
        Arity::Exact(1),
        "whether a concept is a boolean",
    );
    registry.pure(
        "type-is-concept",
        is_concept,
        Arity::Exact(1),
        "whether a concept is named or compound rather than ground or a list",
    );
    registry.pure(
        "type-to-number",
        to_number,
        Arity::Exact(1),
        "parse text into an int or a float",
    );
    registry.pure(
        "type-to-bool",
        to_bool,
        Arity::Exact(1),
        "coerce a ground value to a boolean: 0, 0.0, empty text, and \"false\" are false",
    );
    registry.pure(
        "type-to-int",
        to_int,
        Arity::Exact(1),
        "coerce a number to an int, truncating a float toward zero",
    );
    registry.pure(
        "type-to-float",
        to_float,
        Arity::Exact(1),
        "coerce a number to a float, widening an int",
    );
    registry.pure(
        "type-coerce",
        coerce,
        Arity::Exact(2),
        "coerce a value to a named type: text, int, float, bool, or number",
    );
}
