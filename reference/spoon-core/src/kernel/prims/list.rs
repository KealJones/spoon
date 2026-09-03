//! List manipulation primitives (list.*). All are Effect::Pure.

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};
use super::super::eval::apply_lambda;

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

    let la = Type::list(Type::Any);
    let fn1 = Type::func(vec![Type::Any], Type::Any);
    let fn1b = Type::func(vec![Type::Any], Type::Bool);
    let fn2 = Type::func(vec![Type::Any, Type::Any], Type::Any);

    p!("list.length",   &["length", "count"],   vec![Input::required("list", la.clone())], Type::Int, "number of elements in a list",  list_length);
    p!("list.first",    &["first"],              vec![Input::required("list", la.clone())], Type::Any, "first element, or null",         list_first);
    p!("list.last",     &["last"],               vec![Input::required("list", la.clone())], Type::Any, "last element, or null",          list_last);
    p!("list.nth",      &["nth"],                vec![Input::required("list", la.clone()), Input::required("n", Type::Int)], Type::Any, "nth element (0-based), or null", list_nth);
    cmd!("list.append",    &["append"],          vec![Input::required("list", la.clone()), Input::required("item", Type::Any)], la.clone(), "append item to end of list", list_append);
    cmd!("list.prepend",   &["prepend"],         vec![Input::required("item", Type::Any), Input::required("list", la.clone())], la.clone(), "prepend item to front of list", list_prepend);
    cmd!("list.concat",    &["concat", "concatenate"], vec![Input::required("a", la.clone()), Input::required("b", la.clone())], la.clone(), "concatenate two lists", list_concat);
    cmd!("list.reverse",   &["reverse"],         vec![Input::required("list", la.clone())], la.clone(), "reverse a list",              list_reverse);
    cmd!("list.sort",      &["sort"],            vec![Input::required("list", la.clone())], la.clone(), "sort list in natural order (numbers, then text)", list_sort);
    cmd!("list.sort_by",   &["sort-by"],         vec![Input::required("list", la.clone()), Input::required("key", fn1.clone())], la.clone(), "sort list by key function", list_sort_by);
    cmd!("list.sort_desc", &["sort-desc"],       vec![Input::required("list", la.clone())], la.clone(), "sort list in descending natural order", list_sort_desc);
    cmd!("list.unique",    &["unique", "dedupe"], vec![Input::required("list", la.clone())], la.clone(), "remove duplicate elements", list_unique);
    p!("list.contains",    &["contains"],        vec![Input::required("list", la.clone()), Input::required("item", Type::Any)], Type::Bool, "true if list contains item", list_contains);
    p!("list.index_of",    &["index-of"],        vec![Input::required("list", la.clone()), Input::required("item", Type::Any)], Type::Int, "index of first occurrence, or -1", list_index_of);
    cmd!("list.map",       &["map"],             vec![Input::required("list", la.clone()), Input::required("fn", fn1.clone())], la.clone(), "apply function to each element", list_map);
    cmd!("list.filter",    &["filter"],          vec![Input::required("list", la.clone()), Input::required("fn", fn1b.clone())], la.clone(), "keep elements where fn is true", list_filter);
    cmd!("list.fold",      &["fold", "reduce"],  vec![Input::required("list", la.clone()), Input::required("init", Type::Any), Input::required("fn", fn2.clone())], Type::Any, "fold list with accumulator function", list_fold);
    p!("list.any",         &["any"],             vec![Input::required("list", la.clone()), Input::required("fn", fn1b.clone())], Type::Bool, "true if any element satisfies fn", list_any);
    p!("list.all",         &["all"],             vec![Input::required("list", la.clone()), Input::required("fn", fn1b.clone())], Type::Bool, "true if all elements satisfy fn", list_all);
    p!("list.count_where", &["count-where"],     vec![Input::required("list", la.clone()), Input::required("fn", fn1b.clone())], Type::Int, "count elements satisfying fn", list_count_where);
    cmd!("list.zip",       &["zip"],             vec![Input::required("a", la.clone()), Input::required("b", la.clone())], la.clone(), "zip two lists into list of pairs", list_zip);
    cmd!("list.take",      &["take"],            vec![Input::required("list", la.clone()), Input::required("n", Type::Int)], la.clone(), "take first n elements", list_take);
    cmd!("list.drop",      &["drop"],            vec![Input::required("list", la.clone()), Input::required("n", Type::Int)], la.clone(), "drop first n elements", list_drop);
    cmd!("list.flatten",   &["flatten"],         vec![Input::required("list", la.clone())], la.clone(), "flatten one level of nested lists", list_flatten);
    cmd!("list.max_by",    &["max-by"],          vec![Input::required("list", la.clone()), Input::required("key", fn1.clone())], Type::Any, "element with maximum key value", list_max_by);
    cmd!("list.min_by",    &["min-by"],          vec![Input::required("list", la.clone()), Input::required("key", fn1.clone())], Type::Any, "element with minimum key value", list_min_by);
    p!("list.argmax",      &["argmax"],          vec![Input::required("list", Type::list(Type::Float))], Type::Int, "index of maximum element", list_argmax);
    p!("list.argmin",      &["argmin"],          vec![Input::required("list", Type::list(Type::Float))], Type::Int, "index of minimum element", list_argmin);
    p!("list.is_empty",    &["is-empty"],        vec![Input::required("list", la.clone())], Type::Bool, "true if list is empty", list_is_empty);
    cmd!("list.slice",     &["slice"],           vec![Input::required("list", la.clone()), Input::required("start", Type::Int), Input::required("end", Type::Int)], la.clone(), "sublist from start (inclusive) to end (exclusive)", list_slice);
    cmd!("list.enumerate", &["enumerate"],       vec![Input::required("list", la.clone())], la.clone(), "pair each element with its index", list_enumerate);
    cmd!("list.find",      &["find"],            vec![Input::required("list", la.clone()), Input::required("fn", fn1b.clone())], Type::Any, "first element satisfying fn, or null", list_find);
    cmd!("list.group_by",  &["group-by"],        vec![Input::required("list", la.clone()), Input::required("fn", fn1.clone())], Type::Json, "group elements by key function into JSON object", list_group_by);
}

// ---- helpers -----------------------------------------------------------

fn get_list<'a>(id: &str, args: &'a [Value], i: usize) -> Result<&'a [Value], EvalError> {
    args.get(i)
        .and_then(Value::as_list)
        .ok_or_else(|| EvalError::ty("list", args.get(i).unwrap_or(&Value::Null), id))
}

fn get_lambda(id: &str, args: &[Value], i: usize) -> Result<Lambda, EvalError> {
    match args.get(i) {
        Some(Value::Lambda(l)) => Ok(*l.clone()),
        other => Err(EvalError::ty("lambda", other.unwrap_or(&Value::Null), id)),
    }
}

fn natural_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.render().cmp(&b.render()),
    }
}

// ---- primitives --------------------------------------------------------

fn list_length(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Int(get_list("list.length", args, 0)?.len() as i64))
}
fn list_first(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(get_list("list.first", args, 0)?.first().cloned().unwrap_or(Value::Null))
}
fn list_last(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(get_list("list.last", args, 0)?.last().cloned().unwrap_or(Value::Null))
}
fn list_nth(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.nth", args, 0)?;
    let n = args.get(1).and_then(Value::as_int).ok_or_else(|| EvalError::ty("int", args.get(1).unwrap_or(&Value::Null), "list.nth"))?;
    Ok(list.get(n.max(0) as usize).cloned().unwrap_or(Value::Null))
}
fn list_append(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let mut list = get_list("list.append", args, 0)?.to_vec();
    let item = args.get(1).cloned().unwrap_or(Value::Null);
    list.push(item);
    Ok(Value::List(list))
}
fn list_prepend(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let item = args.first().cloned().unwrap_or(Value::Null);
    let mut list = get_list("list.prepend", args, 1)?.to_vec();
    list.insert(0, item);
    Ok(Value::List(list))
}
fn list_concat(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = get_list("list.concat", args, 0)?.to_vec();
    let b = get_list("list.concat", args, 1)?.to_vec();
    Ok(Value::List(a.into_iter().chain(b).collect()))
}
fn list_reverse(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let mut list = get_list("list.reverse", args, 0)?.to_vec();
    list.reverse();
    Ok(Value::List(list))
}
fn list_sort(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let mut list = get_list("list.sort", args, 0)?.to_vec();
    list.sort_by(natural_cmp);
    Ok(Value::List(list))
}
fn list_sort_by(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.sort_by", args, 0)?.to_vec();
    let lam = get_lambda("list.sort_by", args, 1)?;
    // Collect keys first to avoid borrow issues.
    let mut keyed: Vec<(Value, Value)> = list.into_iter()
        .map(|item| {
            let key = apply_lambda(ctx, &lam, &[item.clone()])?;
            Ok((key, item))
        })
        .collect::<Result<_, EvalError>>()?;
    keyed.sort_by(|(ka, _), (kb, _)| natural_cmp(ka, kb));
    Ok(Value::List(keyed.into_iter().map(|(_, item)| item).collect()))
}
fn list_sort_desc(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let mut list = get_list("list.sort_desc", args, 0)?.to_vec();
    list.sort_by(|a, b| natural_cmp(b, a));
    Ok(Value::List(list))
}
fn list_unique(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.unique", args, 0)?.to_vec();
    let mut seen: Vec<Value> = Vec::new();
    for item in list {
        if !seen.iter().any(|s| {
            match (s.as_f64(), item.as_f64()) {
                (Some(a), Some(b)) => a == b,
                _ => s == &item,
            }
        }) {
            seen.push(item);
        }
    }
    Ok(Value::List(seen))
}
fn list_contains(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.contains", args, 0)?;
    let item = args.get(1).unwrap_or(&Value::Null);
    let found = list.iter().any(|v| match (v.as_f64(), item.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => v == item,
    });
    Ok(Value::Bool(found))
}
fn list_index_of(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.index_of", args, 0)?;
    let item = args.get(1).unwrap_or(&Value::Null);
    let idx = list.iter().position(|v| match (v.as_f64(), item.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => v == item,
    });
    Ok(Value::Int(idx.map(|i| i as i64).unwrap_or(-1)))
}
fn list_map(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.map", args, 0)?.to_vec();
    let lam = get_lambda("list.map", args, 1)?;
    let result: Result<Vec<Value>, _> = list.into_iter()
        .map(|item| apply_lambda(ctx, &lam, &[item]))
        .collect();
    Ok(Value::List(result?))
}
fn list_filter(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.filter", args, 0)?.to_vec();
    let lam = get_lambda("list.filter", args, 1)?;
    let mut result = Vec::new();
    for item in list {
        let keep = apply_lambda(ctx, &lam, &[item.clone()])?;
        match keep {
            Value::Bool(true) => result.push(item),
            Value::Bool(false) => {}
            other => return Err(EvalError::ty("bool", &other, "list.filter fn")),
        }
    }
    Ok(Value::List(result))
}
fn list_fold(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.fold", args, 0)?.to_vec();
    let init = args.get(1).cloned().unwrap_or(Value::Null);
    let lam = get_lambda("list.fold", args, 2)?;
    let mut acc = init;
    for item in list {
        acc = apply_lambda(ctx, &lam, &[acc, item])?;
    }
    Ok(acc)
}
fn list_any(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.any", args, 0)?.to_vec();
    let lam = get_lambda("list.any", args, 1)?;
    for item in list {
        if let Value::Bool(true) = apply_lambda(ctx, &lam, &[item])? {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}
fn list_all(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.all", args, 0)?.to_vec();
    let lam = get_lambda("list.all", args, 1)?;
    for item in list {
        match apply_lambda(ctx, &lam, &[item])? {
            Value::Bool(false) => return Ok(Value::Bool(false)),
            Value::Bool(true) => {}
            other => return Err(EvalError::ty("bool", &other, "list.all fn")),
        }
    }
    Ok(Value::Bool(true))
}
fn list_count_where(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.count_where", args, 0)?.to_vec();
    let lam = get_lambda("list.count_where", args, 1)?;
    let mut count = 0i64;
    for item in list {
        if let Value::Bool(true) = apply_lambda(ctx, &lam, &[item])? {
            count += 1;
        }
    }
    Ok(Value::Int(count))
}
fn list_zip(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = get_list("list.zip", args, 0)?;
    let b = get_list("list.zip", args, 1)?;
    Ok(Value::List(a.iter().zip(b).map(|(x, y)| Value::List(vec![x.clone(), y.clone()])).collect()))
}
fn list_take(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.take", args, 0)?;
    let n = args.get(1).and_then(Value::as_int).unwrap_or(0).max(0) as usize;
    Ok(Value::List(list.iter().take(n).cloned().collect()))
}
fn list_drop(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.drop", args, 0)?;
    let n = args.get(1).and_then(Value::as_int).unwrap_or(0).max(0) as usize;
    Ok(Value::List(list.iter().skip(n).cloned().collect()))
}
fn list_flatten(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.flatten", args, 0)?;
    let mut out = Vec::new();
    for item in list {
        match item {
            Value::List(inner) => out.extend(inner.iter().cloned()),
            other => out.push(other.clone()),
        }
    }
    Ok(Value::List(out))
}
fn list_max_by(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.max_by", args, 0)?.to_vec();
    if list.is_empty() {
        return Ok(Value::Null);
    }
    let lam = get_lambda("list.max_by", args, 1)?;
    let mut best_item = list[0].clone();
    let mut best_key = apply_lambda(ctx, &lam, &[best_item.clone()])?;
    for item in list.into_iter().skip(1) {
        let key = apply_lambda(ctx, &lam, &[item.clone()])?;
        if natural_cmp(&key, &best_key) == std::cmp::Ordering::Greater {
            best_key = key;
            best_item = item;
        }
    }
    Ok(best_item)
}
fn list_min_by(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.min_by", args, 0)?.to_vec();
    if list.is_empty() {
        return Ok(Value::Null);
    }
    let lam = get_lambda("list.min_by", args, 1)?;
    let mut best_item = list[0].clone();
    let mut best_key = apply_lambda(ctx, &lam, &[best_item.clone()])?;
    for item in list.into_iter().skip(1) {
        let key = apply_lambda(ctx, &lam, &[item.clone()])?;
        if natural_cmp(&key, &best_key) == std::cmp::Ordering::Less {
            best_key = key;
            best_item = item;
        }
    }
    Ok(best_item)
}
fn list_argmax(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("list.argmax".into());
    let list = get_list("list.argmax", args, 0)?;
    if list.is_empty() {
        return Err(EvalError::runtime(&id, "argmax of empty list"));
    }
    let mut best_idx = 0usize;
    let mut best_val = list[0].as_f64().ok_or_else(|| EvalError::runtime(&id, "list contains non-number"))?;
    for (i, v) in list.iter().enumerate().skip(1) {
        let f = v.as_f64().ok_or_else(|| EvalError::runtime(&id, "list contains non-number"))?;
        if f > best_val { best_val = f; best_idx = i; }
    }
    Ok(Value::Int(best_idx as i64))
}
fn list_argmin(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("list.argmin".into());
    let list = get_list("list.argmin", args, 0)?;
    if list.is_empty() {
        return Err(EvalError::runtime(&id, "argmin of empty list"));
    }
    let mut best_idx = 0usize;
    let mut best_val = list[0].as_f64().ok_or_else(|| EvalError::runtime(&id, "list contains non-number"))?;
    for (i, v) in list.iter().enumerate().skip(1) {
        let f = v.as_f64().ok_or_else(|| EvalError::runtime(&id, "list contains non-number"))?;
        if f < best_val { best_val = f; best_idx = i; }
    }
    Ok(Value::Int(best_idx as i64))
}
fn list_is_empty(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Bool(get_list("list.is_empty", args, 0)?.is_empty()))
}
fn list_slice(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.slice", args, 0)?;
    let len = list.len() as i64;
    let start = args.get(1).and_then(Value::as_int).unwrap_or(0).max(0).min(len) as usize;
    let end = args.get(2).and_then(Value::as_int).unwrap_or(len).max(0).min(len) as usize;
    Ok(Value::List(list[start.min(end)..end].to_vec()))
}
fn list_enumerate(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.enumerate", args, 0)?;
    Ok(Value::List(
        list.iter().enumerate()
            .map(|(i, v)| Value::List(vec![Value::Int(i as i64), v.clone()]))
            .collect()
    ))
}
fn list_find(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.find", args, 0)?.to_vec();
    let lam = get_lambda("list.find", args, 1)?;
    for item in list {
        if let Value::Bool(true) = apply_lambda(ctx, &lam, &[item.clone()])? {
            return Ok(item);
        }
    }
    Ok(Value::Null)
}
fn list_group_by(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let list = get_list("list.group_by", args, 0)?.to_vec();
    let lam = get_lambda("list.group_by", args, 1)?;
    let mut map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for item in list {
        let key = apply_lambda(ctx, &lam, &[item.clone()])?.render();
        let item_json = serde_json::to_value(&item).unwrap_or(serde_json::Value::Null);
        map.entry(key)
            .or_insert_with(|| serde_json::Value::Array(vec![]))
            .as_array_mut()
            .unwrap()
            .push(item_json);
    }
    Ok(Value::Json(serde_json::Value::Object(map)))
}
