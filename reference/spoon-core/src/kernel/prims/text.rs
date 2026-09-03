//! Text manipulation primitives (text.*). All are Effect::Pure.

use regex::Regex;

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

    let t = || Type::Text;
    let i = || Type::Int;
    let lt = || Type::list(Type::Text);

    cmd!("text.concat",     &["concat", "concatenate"],         vec![Input::required("a", t()), Input::required("b", t())], t(), "concatenate two strings",             concat);
    p!("text.upper",        &["upper", "uppercase"],            vec![Input::required("text", t())], t(), "convert to uppercase",                    upper);
    p!("text.lower",        &["lower", "lowercase"],            vec![Input::required("text", t())], t(), "convert to lowercase",                    lower);
    p!("text.trim",         &["trim"],                          vec![Input::required("text", t())], t(), "remove leading and trailing whitespace",  trim);
    p!("text.length",       &["length", "len"],                 vec![Input::required("text", t())], i(), "character length of text",               length);
    p!("text.contains",     &["contains"],                      vec![Input::required("text", t()), Input::required("substr", t())], Type::Bool, "true if text contains substr", contains);
    p!("text.starts_with",  &["starts-with"],                   vec![Input::required("text", t()), Input::required("prefix", t())], Type::Bool, "true if text starts with prefix", starts_with);
    p!("text.ends_with",    &["ends-with"],                     vec![Input::required("text", t()), Input::required("suffix", t())], Type::Bool, "true if text ends with suffix", ends_with);
    cmd!("text.split",      &["split"],                         vec![Input::required("text", t()), Input::required("delim", t())], lt(), "split text by delimiter",         split);
    cmd!("text.join",       &["join"],                          vec![Input::required("list", lt()), Input::required("sep", t())], t(), "join list of strings with separator", join);
    cmd!("text.replace",    &["replace"],                       vec![Input::required("text", t()), Input::required("from", t()), Input::required("to", t())], t(), "replace all occurrences of from with to", replace);
    p!("text.slice",        &["slice"],                         vec![Input::required("text", t()), Input::required("start", i()), Input::required("end", i())], t(), "substring from start (inclusive) to end (exclusive)", slice);
    p!("text.index_of",     &["index-of"],                      vec![Input::required("text", t()), Input::required("substr", t())], i(), "index of first occurrence, or -1", index_of);
    p!("text.reverse",      &["reverse"],                       vec![Input::required("text", t())], t(), "reverse a string",                       reverse);
    cmd!("text.words",      &["words"],                         vec![Input::required("text", t())], lt(), "split into words (whitespace)",          words);
    cmd!("text.lines",      &["lines"],                         vec![Input::required("text", t())], lt(), "split into lines",                       lines);
    p!("text.repeat",       &["repeat"],                        vec![Input::required("text", t()), Input::required("times", i())], t(), "repeat a string N times",          text_repeat);
    p!("text.char_at",      &["char-at"],                       vec![Input::required("text", t()), Input::required("index", i())], t(), "character at index (0-based)",     char_at);
    p!("text.parse_int",    &["parse-int"],                     vec![Input::required("text", t())], i(), "parse integer from text",                parse_int);
    p!("text.parse_float",  &["parse-float"],                   vec![Input::required("text", t())], Type::Float, "parse float from text",          parse_float);
    p!("text.regex_is_match", &["regex-match"],                 vec![Input::required("text", t()), Input::required("pattern", t())], Type::Bool, "true if text matches regex pattern", regex_is_match);
    cmd!("text.regex_find_all", &["regex-find-all"],            vec![Input::required("text", t()), Input::required("pattern", t())], lt(), "all non-overlapping regex matches", regex_find_all);
    cmd!("text.regex_replace", &["regex-replace"],              vec![Input::required("text", t()), Input::required("pattern", t()), Input::required("replacement", t())], t(), "replace regex matches", regex_replace);
    p!("text.title_case",   &["title-case"],                    vec![Input::required("text", t())], t(), "convert to title case",                  title_case);
    p!("text.is_empty",     &["is-empty"],                      vec![Input::required("text", t())], Type::Bool, "true if text is empty or only whitespace", is_empty);
    p!("text.count_occurrences", &["count-occurrences"],        vec![Input::required("text", t()), Input::required("substr", t())], i(), "count non-overlapping occurrences of substr", count_occurrences);
    cmd!("text.to_name",    &["to-name"],                       vec![Input::required("text", t())], Type::Name, "convert text to a Name",           to_name);
    cmd!("text.from_name",  &["from-name"],                     vec![Input::required("name", Type::Name)], t(), "convert a Name to text",           from_name);
}

// ---- helpers -----------------------------------------------------------

fn text_arg<'a>(id: &str, args: &'a [Value], i: usize) -> Result<&'a str, EvalError> {
    args.get(i)
        .and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.get(i).unwrap_or(&Value::Null), id))
}
fn int_arg(id: &str, args: &[Value], i: usize) -> Result<i64, EvalError> {
    args.get(i)
        .and_then(Value::as_int)
        .ok_or_else(|| EvalError::ty("int", args.get(i).unwrap_or(&Value::Null), id))
}

// ---- primitives --------------------------------------------------------

fn concat(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = text_arg("text.concat", args, 0)?;
    let b = text_arg("text.concat", args, 1)?;
    Ok(Value::Text(format!("{a}{b}")))
}
fn upper(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Text(text_arg("text.upper", args, 0)?.to_uppercase()))
}
fn lower(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Text(text_arg("text.lower", args, 0)?.to_lowercase()))
}
fn trim(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Text(text_arg("text.trim", args, 0)?.trim().to_string()))
}
fn length(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.length", args, 0)?;
    Ok(Value::Int(s.chars().count() as i64))
}
fn contains(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.contains", args, 0)?;
    let sub = text_arg("text.contains", args, 1)?;
    Ok(Value::Bool(s.contains(sub)))
}
fn starts_with(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.starts_with", args, 0)?;
    let pre = text_arg("text.starts_with", args, 1)?;
    Ok(Value::Bool(s.starts_with(pre)))
}
fn ends_with(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.ends_with", args, 0)?;
    let suf = text_arg("text.ends_with", args, 1)?;
    Ok(Value::Bool(s.ends_with(suf)))
}
fn split(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.split", args, 0)?;
    let d = text_arg("text.split", args, 1)?;
    Ok(Value::List(s.split(d).map(|p| Value::Text(p.to_string())).collect()))
}
fn join(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("text.join".into());
    let list = args.first().and_then(Value::as_list).ok_or_else(|| EvalError::ty("list", args.first().unwrap_or(&Value::Null), "text.join"))?.to_vec();
    let sep = text_arg("text.join", args, 1)?;
    let parts: Result<Vec<&str>, _> = list.iter()
        .map(|v| v.as_str().ok_or_else(|| EvalError::runtime(&id, "list contains non-text element")))
        .collect();
    Ok(Value::Text(parts?.join(sep)))
}
fn replace(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.replace", args, 0)?.to_string();
    let from = text_arg("text.replace", args, 1)?;
    let to = text_arg("text.replace", args, 2)?;
    Ok(Value::Text(s.replace(from, to)))
}
fn slice(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.slice", args, 0)?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let start = int_arg("text.slice", args, 1)?.max(0).min(len) as usize;
    let end = int_arg("text.slice", args, 2)?.max(0).min(len) as usize;
    Ok(Value::Text(chars[start.min(end)..end].iter().collect()))
}
fn index_of(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.index_of", args, 0)?;
    let sub = text_arg("text.index_of", args, 1)?;
    let idx = s.find(sub).map(|b| s[..b].chars().count() as i64).unwrap_or(-1);
    Ok(Value::Int(idx))
}
fn reverse(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.reverse", args, 0)?;
    Ok(Value::Text(s.chars().rev().collect()))
}
fn words(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.words", args, 0)?;
    Ok(Value::List(s.split_whitespace().map(|w| Value::Text(w.to_string())).collect()))
}
fn lines(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.lines", args, 0)?;
    Ok(Value::List(s.lines().map(|l| Value::Text(l.to_string())).collect()))
}
fn text_repeat(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.repeat", args, 0)?;
    let n = int_arg("text.repeat", args, 1)?.max(0) as usize;
    Ok(Value::Text(s.repeat(n)))
}
fn char_at(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("text.char_at".into());
    let s = text_arg("text.char_at", args, 0)?;
    let i = int_arg("text.char_at", args, 1)?;
    let c = s.chars().nth(i.max(0) as usize)
        .ok_or_else(|| EvalError::runtime(&id, format!("index {i} out of bounds")))?;
    Ok(Value::Text(c.to_string()))
}
fn parse_int(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("text.parse_int".into());
    let s = text_arg("text.parse_int", args, 0)?.trim();
    s.parse::<i64>().map(Value::Int).map_err(|_| EvalError::runtime(&id, format!("cannot parse int: {s}")))
}
fn parse_float(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("text.parse_float".into());
    let s = text_arg("text.parse_float", args, 0)?.trim();
    s.parse::<f64>().map(Value::Float).map_err(|_| EvalError::runtime(&id, format!("cannot parse float: {s}")))
}
fn regex_is_match(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("text.regex_is_match".into());
    let s = text_arg("text.regex_is_match", args, 0)?;
    let pat = text_arg("text.regex_is_match", args, 1)?;
    let re = Regex::new(pat).map_err(|e| EvalError::runtime(&id, format!("invalid regex: {e}")))?;
    Ok(Value::Bool(re.is_match(s)))
}
fn regex_find_all(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("text.regex_find_all".into());
    let s = text_arg("text.regex_find_all", args, 0)?.to_string();
    let pat = text_arg("text.regex_find_all", args, 1)?;
    let re = Regex::new(pat).map_err(|e| EvalError::runtime(&id, format!("invalid regex: {e}")))?;
    Ok(Value::List(re.find_iter(&s).map(|m| Value::Text(m.as_str().to_string())).collect()))
}
fn regex_replace(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("text.regex_replace".into());
    let s = text_arg("text.regex_replace", args, 0)?.to_string();
    let pat = text_arg("text.regex_replace", args, 1)?;
    let rep = text_arg("text.regex_replace", args, 2)?.to_string();
    let re = Regex::new(pat).map_err(|e| EvalError::runtime(&id, format!("invalid regex: {e}")))?;
    Ok(Value::Text(re.replace_all(&s, rep.as_str()).into_owned()))
}
fn title_case(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.title_case", args, 0)?;
    let out: String = s.split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(Value::Text(out))
}
fn is_empty(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.is_empty", args, 0)?;
    Ok(Value::Bool(s.trim().is_empty()))
}
fn count_occurrences(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.count_occurrences", args, 0)?;
    let sub = text_arg("text.count_occurrences", args, 1)?;
    if sub.is_empty() {
        return Ok(Value::Int(0));
    }
    let mut count = 0i64;
    let mut start = 0;
    while let Some(pos) = s[start..].find(sub) {
        count += 1;
        start += pos + sub.len();
    }
    Ok(Value::Int(count))
}
fn to_name(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = text_arg("text.to_name", args, 0)?;
    Ok(Value::Name(s.to_string()))
}
fn from_name(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("name", args.first().unwrap_or(&Value::Null), "text.from_name"))?;
    Ok(Value::Text(s.to_string()))
}
