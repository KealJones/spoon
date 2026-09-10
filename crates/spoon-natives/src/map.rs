//! Maps: JSON objects, addressed by key.
//!
//! There is no separate map concept. A `Ground::Json` object already is one,
//! and `data/json.rs` already knows how to read one; this module is the
//! read-write half that treats an object as a value to build rather than only
//! a document to inspect. Values pulled out cross the same JSON/concept
//! boundary `data/json.rs::from_json` defines: scalars convert, objects and
//! arrays stay `Json`.

use serde_json::{Map, Value};
use spoon_concept::{Concept, Ground};
use spoon_eval::{
    ArgStrategy, Arity, Ctx, EvalError, EvalResult, NativeRegistry, native_error, type_error,
};

use crate::collections::{make_list, want_list};
use crate::data::json::from_json;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn want_object<'a>(native: &str, c: &'a Concept) -> Result<&'a Map<String, Value>, EvalError> {
    match c.as_ground() {
        Some(Ground::Json(blob)) => blob
            .value()
            .as_object()
            .ok_or_else(|| type_error(native, "a JSON object", c)),
        _ => Err(type_error(native, "a JSON object", c)),
    }
}

fn want_key(native: &str, c: &Concept) -> Result<String, EvalError> {
    c.as_ground()
        .and_then(Ground::as_str)
        .map(str::to_string)
        .ok_or_else(|| type_error(native, "a text key", c))
}

/// Concept to JSON value. A ground value maps onto its obvious JSON form, and
/// a list becomes an array. Anything else is refused: a named or otherwise
/// compound concept's meaning lives in the store, not in a JSON document.
fn to_value(native: &str, c: &Concept) -> Result<Value, EvalError> {
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
    if let Ok(items) = want_list(native, c) {
        let mut out = Vec::with_capacity(items.len());
        for item in &items {
            out.push(to_value(native, item)?);
        }
        return Ok(Value::Array(out));
    }
    Err(type_error(native, "a ground value or a list", c))
}

fn object_concept(map: Map<String, Value>) -> Concept {
    Concept::json(Value::Object(map))
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Sorted, like `json-keys`, so the same object always reports its keys in
/// the same order regardless of what wrote it.
fn keys(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let object = want_object("map-keys", &args[0])?;
    let mut ks: Vec<&str> = object.keys().map(String::as_str).collect();
    ks.sort_unstable();
    Ok(make_list(ks.into_iter().map(Concept::text).collect()))
}

/// Values in the same key order `map-keys` reports, so zipping the two lists
/// back together reconstructs the pairing.
fn values(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let object = want_object("map-values", &args[0])?;
    let mut ks: Vec<&str> = object.keys().map(String::as_str).collect();
    ks.sort_unstable();
    Ok(make_list(
        ks.into_iter().map(|k| from_json(&object[k])).collect(),
    ))
}

fn entries(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let object = want_object("map-entries", &args[0])?;
    let mut ks: Vec<&str> = object.keys().map(String::as_str).collect();
    ks.sort_unstable();
    Ok(make_list(
        ks.into_iter()
            .map(|k| make_list(vec![Concept::text(k), from_json(&object[k])]))
            .collect(),
    ))
}

fn from_entries(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let pairs = want_list("map-from-entries", &args[0])?;
    let mut object = Map::new();
    for pair in &pairs {
        let items = want_list("map-from-entries", pair)?;
        if items.len() != 2 {
            return Err(native_error(
                "map-from-entries",
                format!(
                    "each entry must be a pair of key and value, got {} elements",
                    items.len()
                ),
            ));
        }
        let key = want_key("map-from-entries", &items[0])?;
        object.insert(key, to_value("map-from-entries", &items[1])?);
    }
    Ok(object_concept(object))
}

fn get(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let object = want_object("map-get", &args[0])?;
    let key = want_key("map-get", &args[1])?;
    object.get(&key).map(from_json).ok_or_else(|| {
        let mut ks: Vec<&str> = object.keys().map(String::as_str).collect();
        ks.sort_unstable();
        let available = if ks.is_empty() {
            "this map has no keys".to_string()
        } else {
            format!("this map has {}", ks.join(", "))
        };
        native_error("map-get", format!("no key {key:?}: {available}"))
    })
}

fn has_key(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let object = want_object("map-has-key", &args[0])?;
    let key = want_key("map-has-key", &args[1])?;
    Ok(Concept::bool(object.contains_key(&key)))
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

fn set(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut object = want_object("map-set", &args[0])?.clone();
    let key = want_key("map-set", &args[1])?;
    object.insert(key, to_value("map-set", &args[2])?);
    Ok(object_concept(object))
}

/// `b` wins on a shared key, matching how `map-set` treats a later write as
/// authoritative over an earlier one.
fn merge(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut object = want_object("map-merge", &args[0])?.clone();
    let b = want_object("map-merge", &args[1])?;
    for (k, v) in b {
        object.insert(k.clone(), v.clone());
    }
    Ok(object_concept(object))
}

fn pick(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let object = want_object("map-pick", &args[0])?;
    let wanted = want_list("map-pick", &args[1])?;
    let mut out = Map::new();
    for key in &wanted {
        let key = want_key("map-pick", key)?;
        if let Some(v) = object.get(&key) {
            out.insert(key, v.clone());
        }
    }
    Ok(object_concept(out))
}

fn omit(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let object = want_object("map-omit", &args[0])?;
    let unwanted = want_list("map-omit", &args[1])?
        .iter()
        .map(|k| want_key("map-omit", k))
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Map::new();
    for (k, v) in object {
        if !unwanted.contains(k) {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(object_concept(out))
}

/// Apply a function to the value at one key, leaving the rest of the object
/// untouched. The function argument is not reduced up front (`Selective`
/// leaves bit 2 unset) for the same reason `list-map`'s function argument
/// isn't: it is applied, not evaluated as a call with no arguments.
fn update(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut object = want_object("map-update", &args[0])?.clone();
    let key = want_key("map-update", &args[1])?;
    let current = object.get(&key).map(from_json).ok_or_else(|| {
        let mut ks: Vec<&str> = object.keys().map(String::as_str).collect();
        ks.sort_unstable();
        let available = if ks.is_empty() {
            "this map has no keys".to_string()
        } else {
            format!("this map has {}", ks.join(", "))
        };
        native_error("map-update", format!("no key {key:?}: {available}"))
    })?;
    let f = &args[2];
    let updated = if !spoon_concept::holes(f).is_empty() {
        let bound = spoon_concept::substitute_positional(f, &[current]);
        ctx.eval(&bound)?
    } else {
        ctx.eval(&Concept::apply(f.clone(), vec![current]))?
    };
    object.insert(key, to_value("map-update", &updated)?);
    Ok(object_concept(object))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register(registry: &mut NativeRegistry) {
    registry.pure(
        "map-keys",
        keys,
        Arity::Exact(1),
        "the keys of a map, sorted",
    );
    registry.pure(
        "map-values",
        values,
        Arity::Exact(1),
        "the values of a map, ordered by their sorted keys",
    );
    registry.pure(
        "map-entries",
        entries,
        Arity::Exact(1),
        "a map as a list of key, value pairs, ordered by sorted key",
    );
    registry.pure(
        "map-from-entries",
        from_entries,
        Arity::Exact(1),
        "a map built from a list of key, value pairs",
    );
    registry.pure(
        "map-get",
        get,
        Arity::Exact(2),
        "the value at a key in a map; an error when the key is absent",
    );
    registry.pure(
        "map-set",
        set,
        Arity::Exact(3),
        "a map with a key set to a value, added or replaced",
    );
    registry.pure(
        "map-has-key",
        has_key,
        Arity::Exact(2),
        "whether a map carries a key",
    );
    registry.pure(
        "map-merge",
        merge,
        Arity::Exact(2),
        "two maps combined; the second wins on a shared key",
    );
    registry.pure(
        "map-pick",
        pick,
        Arity::Exact(2),
        "a map holding only the given keys",
    );
    registry.pure(
        "map-omit",
        omit,
        Arity::Exact(2),
        "a map with the given keys removed",
    );
    registry.register(
        "map-update",
        update,
        Arity::Exact(3),
        ArgStrategy::Selective(0b011),
        spoon_concept::Effect::Pure,
        "a map with a function applied to the value at one key",
    );
}
