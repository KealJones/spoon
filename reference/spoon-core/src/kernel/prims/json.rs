//! JSON manipulation primitives (json.*). All are Effect::Pure.

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};

pub fn register(k: &mut Kernel) {
    use Effect::Pure;

    macro_rules! p {
        ($id:expr, $verbs:expr, $inputs:expr, $out:expr, $desc:expr, $f:expr) => {
            k.register(Action::primitive($id, $verbs, $inputs, $out, Pure, $desc), $f)
        };
    }

    let j = || Type::Json;
    let t = || Type::Text;
    let lt = || Type::list(Type::Text);
    let lj = || Type::list(Type::Json);

    p!("json.parse",       &["parse-json"],     vec![Input::required("text", t())], j(), "parse JSON text into a value", json_parse);
    p!("json.stringify",   &["stringify"],       vec![Input::required("json", j())], t(), "serialize JSON value to text", json_stringify);
    p!("json.get",         &["get"],             vec![Input::required("json", j()), Input::required("path", t())], j(), "get value at dot-path (e.g. a.b[0])", json_get);
    p!("json.set",         &["set"],             vec![Input::required("json", j()), Input::required("path", t()), Input::required("value", j())], j(), "set value at dot-path", json_set);
    p!("json.keys",        &["keys"],            vec![Input::required("json", j())], lt(), "keys of a JSON object", json_keys);
    p!("json.values",      &["values"],          vec![Input::required("json", j())], lj(), "values of a JSON object", json_values);
    p!("json.has",         &["has"],             vec![Input::required("json", j()), Input::required("key", t())], Type::Bool, "true if JSON object has key", json_has);
    p!("json.length",      &["length"],          vec![Input::required("json", j())], Type::Int, "length of JSON array or number of keys in object", json_length);
    p!("json.pluck",       &["pluck"],           vec![Input::required("json", j()), Input::required("key", t())], lj(), "value at key from every element of a JSON array", json_pluck);
    p!("json.to_list",     &["to-list"],         vec![Input::required("json", j())], lj(), "convert JSON array to list of JSON values", json_to_list);
    p!("json.from_list",   &["from-list"],       vec![Input::required("list", lj())], j(), "convert list of JSON values to JSON array", json_from_list);
    p!("json.to_value",    &["to-value"],        vec![Input::required("json", j())], Type::Any, "convert JSON to best-fit Value", json_to_value);
    p!("json.from_value",  &["from-value"],      vec![Input::required("value", Type::Any)], j(), "convert any Value to JSON", json_from_value);
}

// ---- path traversal ----------------------------------------------------

fn get_path(v: &serde_json::Value, path: &str) -> serde_json::Value {
    let mut current = v.clone();
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        // Handle "key[N]" array indexing within a segment.
        if let Some(bracket) = segment.find('[') {
            let key = &segment[..bracket];
            let rest = &segment[bracket..];
            if !key.is_empty() {
                current = current.get(key).cloned().unwrap_or(serde_json::Value::Null);
            }
            // Process all consecutive "[N]" groups.
            let mut s = rest;
            while s.starts_with('[') {
                if let Some(close) = s.find(']') {
                    let idx_str = &s[1..close];
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        current = current.get(idx).cloned().unwrap_or(serde_json::Value::Null);
                    } else {
                        current = serde_json::Value::Null;
                    }
                    s = &s[close + 1..];
                } else {
                    break;
                }
            }
        } else {
            current = current.get(segment).cloned().unwrap_or(serde_json::Value::Null);
        }
    }
    current
}

fn set_path(v: serde_json::Value, path: &str, new_val: serde_json::Value) -> serde_json::Value {
    let parts: Vec<&str> = path.splitn(2, '.').collect();
    let key = parts[0];
    let rest = parts.get(1).copied();

    let mut obj = match v {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };

    if let Some(rest_path) = rest {
        let child = obj.get(key).cloned().unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        obj.insert(key.to_string(), set_path(child, rest_path, new_val));
    } else {
        obj.insert(key.to_string(), new_val);
    }
    serde_json::Value::Object(obj)
}

// ---- primitives --------------------------------------------------------

fn json_parse(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("json.parse".into());
    let s = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.first().unwrap_or(&Value::Null), "json.parse"))?;
    let v: serde_json::Value = serde_json::from_str(s)
        .map_err(|e| EvalError::runtime(&id, format!("invalid JSON: {e}")))?;
    Ok(Value::Json(v))
}

fn json_stringify(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("json.stringify".into());
    match args.first() {
        Some(Value::Json(j)) => serde_json::to_string(j)
            .map(Value::Text)
            .map_err(|e| EvalError::runtime(&id, e.to_string())),
        other => Err(EvalError::ty("json", other.unwrap_or(&Value::Null), "json.stringify")),
    }
}

fn json_get(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let j = match args.first() {
        Some(Value::Json(j)) => j.clone(),
        other => return Err(EvalError::ty("json", other.unwrap_or(&Value::Null), "json.get")),
    };
    let path = args.get(1).and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.get(1).unwrap_or(&Value::Null), "json.get"))?;
    Ok(Value::Json(get_path(&j, path)))
}

fn json_set(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let j = match args.first() {
        Some(Value::Json(j)) => j.clone(),
        other => return Err(EvalError::ty("json", other.unwrap_or(&Value::Null), "json.set")),
    };
    let path = args.get(1).and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.get(1).unwrap_or(&Value::Null), "json.set"))?
        .to_string();
    let new_val = match args.get(2) {
        Some(Value::Json(jv)) => jv.clone(),
        other => serde_json::to_value(other.unwrap_or(&Value::Null)).unwrap_or(serde_json::Value::Null),
    };
    Ok(Value::Json(set_path(j, &path, new_val)))
}

fn json_keys(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("json.keys".into());
    match args.first() {
        Some(Value::Json(serde_json::Value::Object(m))) => {
            Ok(Value::List(m.keys().map(|k| Value::Text(k.clone())).collect()))
        }
        other => Err(EvalError::runtime(&id, format!("expected JSON object, got {:?}", other))),
    }
}

fn json_values(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("json.values".into());
    match args.first() {
        Some(Value::Json(serde_json::Value::Object(m))) => {
            Ok(Value::List(m.values().map(|v| Value::Json(v.clone())).collect()))
        }
        other => Err(EvalError::runtime(&id, format!("expected JSON object, got {:?}", other))),
    }
}

fn json_has(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let key = args.get(1).and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.get(1).unwrap_or(&Value::Null), "json.has"))?;
    let found = matches!(args.first(), Some(Value::Json(serde_json::Value::Object(m))) if m.contains_key(key));
    Ok(Value::Bool(found))
}

fn json_length(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("json.length".into());
    match args.first() {
        Some(Value::Json(serde_json::Value::Array(a))) => Ok(Value::Int(a.len() as i64)),
        Some(Value::Json(serde_json::Value::Object(m))) => Ok(Value::Int(m.len() as i64)),
        _other => Err(EvalError::runtime(&id, "expected JSON array or object".to_string())),
    }
}

/// One property across a collection. SCE has no lambda, so "the titles of X"
/// is a pluck, not a map: every element that has `key` contributes its value
/// and the rest are skipped.
fn json_pluck(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let key = args.get(1).and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.get(1).unwrap_or(&Value::Null), "json.pluck"))?;
    match args.first() {
        Some(Value::Json(serde_json::Value::Array(a))) => Ok(Value::List(
            a.iter().filter_map(|v| v.get(key).cloned().map(Value::Json)).collect(),
        )),
        Some(Value::Json(serde_json::Value::Object(m))) => Ok(Value::List(
            m.get(key).cloned().map(Value::Json).into_iter().collect(),
        )),
        other => Err(EvalError::ty("json array or object", other.unwrap_or(&Value::Null), "json.pluck")),
    }
}

fn json_to_list(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("json.to_list".into());
    match args.first() {
        Some(Value::Json(serde_json::Value::Array(a))) => {
            Ok(Value::List(a.iter().map(|v| Value::Json(v.clone())).collect()))
        }
        _other => Err(EvalError::runtime(&id, "expected JSON array")),
    }
}

fn json_from_list(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("json.from_list".into());
    match args.first() {
        Some(Value::List(items)) => {
            let arr: Vec<serde_json::Value> = items.iter().map(|v| {
                if let Value::Json(j) = v { j.clone() }
                else { serde_json::to_value(v).unwrap_or(serde_json::Value::Null) }
            }).collect();
            Ok(Value::Json(serde_json::Value::Array(arr)))
        }
        _other => Err(EvalError::runtime(&id, "expected list")),
    }
}

fn json_to_value(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    match args.first() {
        Some(Value::Json(j)) => Ok(json_value_to_value(j)),
        other => Err(EvalError::ty("json", other.unwrap_or(&Value::Null), "json.to_value")),
    }
}

fn json_value_to_value(j: &serde_json::Value) -> Value {
    match j {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Array(a) => {
            Value::List(a.iter().map(json_value_to_value).collect())
        }
        serde_json::Value::Object(_) => Value::Json(j.clone()),
    }
}

fn json_from_value(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let v = args.first().unwrap_or(&Value::Null);
    let j = serde_json::to_value(v).unwrap_or(serde_json::Value::Null);
    Ok(Value::Json(j))
}
