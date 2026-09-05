//! JSON, and where a document stops being one.
//!
//! # The conversion boundary
//!
//! This is the one real design decision in this module, so it is worth being
//! blunt about it. Pulling a value out of a document converts JSON scalars
//! into their native concept counterparts:
//!
//! | JSON            | concept                            |
//! |-----------------|------------------------------------|
//! | number          | `Int` when it fits an i64, else `Float` |
//! | string          | `Text`                             |
//! | true / false    | `Bool`                             |
//! | object, array   | stays `Json`                       |
//! | null            | stays `Json`                       |
//!
//! Without the conversion every extracted number would arrive as an opaque
//! blob and `Add<Field<doc, "list-count">, 1>` would be a type error, which defeats
//! the point of reading a document at all.
//!
//! Objects and arrays stay JSON because there is nothing better to become:
//! Spoon has no map concept, and re-entering the document to go one level
//! deeper costs nothing. `null` stays JSON for a different reason. A present
//! null is a value, and turning it into an absence would be the same silent
//! guess that a missing field is not allowed to make.
//!
//! An unsigned integer past `i64::MAX` also stays JSON. The alternatives are
//! wrapping it negative or widening it to a float that no longer names the
//! same number, and both of those lie.

use serde_json::{Map, Value};
use spoon_concept::{Concept, Ground, SymbolId};
use spoon_eval::{Arity, Ctx, EvalError, EvalResult, NativeRegistry, native_error, type_error};

use super::{list_of, want_text};

/// How deep a concept may nest before [`to_json`] gives up. The recursion is
/// bounded so that a hand-built list a million deep is a clean error rather
/// than a blown stack.
const MAX_DEPTH: usize = 64;

/// Turn text into a JSON concept.
///
/// A parse failure carries what serde_json said, line and column included,
/// because "invalid JSON" without a position is useless to whoever has to fix
/// the document.
fn parse_json(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("json-parse", &args[0])?;
    let value: Value = serde_json::from_str(text)
        .map_err(|err| native_error("json-parse", format!("not valid JSON: {err}")))?;
    Ok(Concept::json(value))
}

/// Render a concept as JSON text.
///
/// Ground values map onto their obvious JSON forms, a `DateTime` becomes an
/// RFC 3339 string, and `List<a, b, c>` becomes an array. Anything else is an
/// error: a named concept's meaning lives in the store rather than in its
/// spelling, so writing one out as a bare string would produce a document that
/// cannot be read back into the same concept.
fn to_json(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let value = to_value("json-to-json", &args[0], 0)?;
    let text = serde_json::to_string(&value)
        .map_err(|err| native_error("json-to-json", format!("could not serialize: {err}")))?;
    Ok(Concept::text(text))
}

/// One field of a JSON object, converted at the boundary above.
///
/// A missing field is an error naming the field and the keys that are there.
/// Returning null instead would let a typo travel silently through the rest of
/// a computation and surface as nonsense somewhere unrelated.
fn field(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let object = want_object("json-field", &args[0])?;
    let name = want_text("json-field", &args[1])?;
    Ok(from_json(get_field("json-field", object, name)?))
}

/// The same field from every object in a JSON array. "Get me all the titles."
///
/// This is the operation that makes a fetched list of records useful, and it
/// is strict for the same reason [`field`] is: one record missing the field is
/// a broken assumption about the document, not a hole to paper over.
fn pluck(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let value = want_json("json-pluck", &args[0])?;
    let name = want_text("json-pluck", &args[1])?;
    let items = value
        .as_array()
        .ok_or_else(|| type_error("json-pluck", "a JSON array", &args[0]))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let object = item.as_object().ok_or_else(|| {
            type_error(
                "json-pluck",
                "a JSON array of objects",
                &Concept::json(item.clone()),
            )
        })?;
        out.push(from_json(get_field("json-pluck", object, name)?));
    }
    Ok(list_of(out))
}

/// Whether a JSON object carries a field.
///
/// Requires an object. Asking whether the number 3 has a field named "title"
/// is a mistake in the caller, and answering `false` would hide it behind a
/// plausible-looking answer.
fn has_field(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let object = want_object("json-has-field", &args[0])?;
    let name = want_text("json-has-field", &args[1])?;
    Ok(Concept::bool(object.contains_key(name)))
}

/// The keys of a JSON object, sorted, as a list of text.
///
/// Sorted rather than left in document order so that the same object always
/// produces the same list, whatever wrote it.
fn json_keys(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let object = want_object("json-keys", &args[0])?;
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    Ok(list_of(keys.into_iter().map(Concept::text).collect()))
}

/// How many elements an array has, or how many keys an object has.
///
/// Scalars are an error. The length of a number is not a question with an
/// answer, and text length belongs to the text natives.
fn json_length(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let value = want_json("json-length", &args[0])?;
    match value {
        Value::Array(items) => Ok(Concept::int(items.len() as i64)),
        Value::Object(fields) => Ok(Concept::int(fields.len() as i64)),
        _ => Err(type_error(
            "json-length",
            "a JSON array or object",
            &args[0],
        )),
    }
}

/// The shape of a JSON value: `null`, `bool`, `number`, `string`, `array`, or
/// `object`.
///
/// Text rather than a named concept, because this describes the shape of a
/// foreign document rather than anything Spoon has an opinion about, and text
/// composes with the comparison natives without minting six concepts whose
/// names would collide with real ones.
fn json_type(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let value = want_json("json-type", &args[0])?;
    let name = match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    Ok(Concept::text(name))
}

// ---- the boundary itself ----

/// JSON value to concept. See the module docs for why each arm lands where it
/// does.
pub(crate) fn from_json(value: &Value) -> Concept {
    match value {
        Value::Bool(b) => Concept::bool(*b),
        Value::String(s) => Concept::text(s),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Concept::int(i)
            } else if n.is_f64() {
                match n.as_f64() {
                    Some(f) => Concept::float(f),
                    None => Concept::json(value.clone()),
                }
            } else {
                // An unsigned integer past i64::MAX. No native form holds it
                // without lying about its value, so it stays JSON.
                Concept::json(value.clone())
            }
        }
        Value::Null | Value::Array(_) | Value::Object(_) => Concept::json(value.clone()),
    }
}

/// Concept to JSON value, the inverse of [`from_json`] as far as it goes.
fn to_value(native: &str, c: &Concept, depth: usize) -> Result<Value, EvalError> {
    if depth > MAX_DEPTH {
        return Err(native_error(
            native,
            format!("nested more than {MAX_DEPTH} deep"),
        ));
    }
    if let Some(ground) = c.as_ground() {
        return match ground {
            Ground::Bool(b) => Ok(Value::Bool(*b)),
            Ground::Int(i) => Ok(Value::from(*i)),
            Ground::Float(f) => serde_json::Number::from_f64(*f)
                .map(Value::Number)
                .ok_or_else(|| native_error(native, "JSON has no spelling for NaN or infinity")),
            Ground::Text(t) => Ok(Value::String(t.to_string())),
            Ground::DateTime(dt) => Ok(Value::String(dt.to_rfc3339())),
            Ground::Json(blob) => Ok(blob.value().clone()),
            Ground::Bytes(_) => Err(type_error(
                native,
                "a value JSON can hold; bytes are not",
                c,
            )),
        };
    }
    if c.head_symbol() == Some(SymbolId::of("list-of")) {
        let mut out = Vec::with_capacity(c.arity());
        for item in c.args() {
            out.push(to_value(native, item, depth + 1)?);
        }
        return Ok(Value::Array(out));
    }
    Err(type_error(native, "a ground value or a list", c))
}

// ---- argument helpers ----

fn want_json<'a>(native: &str, c: &'a Concept) -> Result<&'a Value, EvalError> {
    match c.as_ground() {
        Some(Ground::Json(blob)) => Ok(blob.value()),
        _ => Err(type_error(native, "a JSON value", c)),
    }
}

fn want_object<'a>(native: &str, c: &'a Concept) -> Result<&'a Map<String, Value>, EvalError> {
    want_json(native, c)?
        .as_object()
        .ok_or_else(|| type_error(native, "a JSON object", c))
}

fn get_field<'a>(
    native: &str,
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Value, EvalError> {
    object.get(name).ok_or_else(|| {
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let available = if keys.is_empty() {
            "this object has no fields".to_string()
        } else {
            format!("this object has {}", keys.join(", "))
        };
        native_error(native, format!("no field {name:?}: {available}"))
    })
}

pub fn register(registry: &mut NativeRegistry) {
    registry.pure(
        "json-parse",
        parse_json,
        Arity::Exact(1),
        "parse text into a JSON value",
    );
    registry.pure(
        "json-to-json",
        to_json,
        Arity::Exact(1),
        "render a ground value or a list as JSON text",
    );
    registry.pure(
        "json-field",
        field,
        Arity::Exact(2),
        "one field of a JSON object, as a native concept",
    );
    registry.pure(
        "json-pluck",
        pluck,
        Arity::Exact(2),
        "the same field from every object in a JSON array",
    );
    registry.pure(
        "json-has-field",
        has_field,
        Arity::Exact(2),
        "whether a JSON object carries a field",
    );
    registry.pure(
        "json-keys",
        json_keys,
        Arity::Exact(1),
        "the keys of a JSON object, sorted",
    );
    registry.pure(
        "json-length",
        json_length,
        Arity::Exact(1),
        "elements in a JSON array, or keys in a JSON object",
    );
    registry.pure(
        "json-type",
        json_type,
        Arity::Exact(1),
        "the shape of a JSON value, as text",
    );
}
