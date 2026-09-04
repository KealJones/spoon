//! Text operations.
//!
//! # Everything is counted in characters
//!
//! Every length, index, and slice boundary in this module is a Unicode
//! character (a `char`, that is, a scalar value), never a byte. `Text-Length`
//! of an emoji is 1, and `Substring` of a string of them cuts between them.
//!
//! That is not merely a nicety. Rust's byte slicing panics when an index lands
//! inside a multibyte character, so a byte-indexed implementation would be one
//! accented letter away from taking the evaluator down. Going through
//! `chars()` makes the split-a-character case impossible to express rather
//! than merely unlikely to happen, so there is nothing left to check for.
//!
//! # Sizes are capped here, not by the evaluator
//!
//! The evaluator's `max_result_size` counts concept nodes. A one-gigabyte
//! string is a single `Atomic(Ground(Text(..)))` node, so it sails past that
//! budget untouched. `Repeat` is the one native here that can manufacture
//! length out of a small input, so it carries its own cap.

use spoon_concept::{Concept, Ground};
use spoon_eval::{Arity, Ctx, EvalError, EvalResult, NativeRegistry, native_error, type_error};

use crate::collections::{make_list, want_list};

/// Characters `Repeat` will produce before it refuses.
///
/// One million is comfortably more than any legitimate use and far short of
/// anything that threatens the process. The number matters less than the fact
/// that there is one.
const MAX_REPEAT_CHARS: usize = 1_000_000;

// ---------------------------------------------------------------------------
// Reading arguments
// ---------------------------------------------------------------------------

pub(crate) fn want_text<'a>(native: &str, c: &'a Concept) -> Result<&'a str, EvalError> {
    c.as_ground()
        .and_then(Ground::as_str)
        .ok_or_else(|| type_error(native, "text", c))
}

/// A non-negative index or count. Negative values are refused outright rather
/// than being read as an offset from the end, because that convention is a
/// guess about intent and this module does not guess.
fn want_count(native: &str, c: &Concept) -> Result<usize, EvalError> {
    let raw = c
        .as_ground()
        .and_then(Ground::as_i64)
        .ok_or_else(|| type_error(native, "a non-negative integer", c))?;
    usize::try_from(raw).map_err(|_| native_error(native, format!("negative count {raw}")))
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

fn out_of_range(native: &str, index: usize, len: usize) -> EvalError {
    native_error(
        native,
        format!("index {index} is out of range for text of length {len}"),
    )
}

// ---------------------------------------------------------------------------
// Building and taking apart
// ---------------------------------------------------------------------------

/// Text only, no coercion. `Concat<"n = ", 42>` looks like it should work, and
/// silently rendering the `42` would make `To-Text` optional in a way that
/// hides the moment a concept stopped being the shape the caller thought.
fn concat(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut out = String::new();
    for a in args {
        out.push_str(want_text("text-concat", a)?);
    }
    Ok(Concept::text(out))
}

/// An empty separator is refused. Rust would happily split on it and hand back
/// empty strings at both ends, which is a result nobody asks for on purpose.
fn split(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-split", &args[0])?;
    let sep = want_text("text-split", &args[1])?;
    if sep.is_empty() {
        return Err(native_error("text-split", "an empty separator has no meaning"));
    }
    Ok(make_list(text.split(sep).map(Concept::text).collect()))
}

/// Elements must already be text. `Join<Map<List<1, 2>, to-text>, ",">` is the
/// spelling for anything else, and it says what it is doing.
fn join(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("text-join", &args[0])?;
    let sep = want_text("text-join", &args[1])?;
    let mut parts = Vec::with_capacity(items.len());
    for item in items.iter() {
        parts.push(want_text("text-join", item)?);
    }
    Ok(Concept::text(parts.join(sep)))
}

/// Splits on `\n` and drops a trailing `\r`, so a file written on Windows reads
/// the same as one written anywhere else. A final newline does not produce a
/// trailing empty line.
fn lines(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-lines", &args[0])?;
    Ok(make_list(text.lines().map(Concept::text).collect()))
}

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

/// Unicode-aware, so `"straße"` uppercases to `"STRASSE"` and grows by a
/// character. Case mapping is not a per-character substitution in general,
/// which is why this cannot be done by indexing.
fn upper(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::text(want_text("text-upper", &args[0])?.to_uppercase()))
}

fn lower(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::text(want_text("text-lower", &args[0])?.to_lowercase()))
}

fn trim(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::text(want_text("text-trim", &args[0])?.trim()))
}

/// An empty needle is refused: replacing nothing with something inserts the
/// replacement between every character, which is never what was meant.
fn replace(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-replace", &args[0])?;
    let from = want_text("text-replace", &args[1])?;
    let to = want_text("text-replace", &args[2])?;
    if from.is_empty() {
        return Err(native_error(
            "text-replace",
            "an empty search string has no meaning",
        ));
    }
    Ok(Concept::text(text.replace(from, to)))
}

fn text_length(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::int(
        char_count(want_text("text-length", &args[0])?) as i64,
    ))
}

fn starts_with(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-starts-with", &args[0])?;
    let prefix = want_text("text-starts-with", &args[1])?;
    Ok(Concept::bool(text.starts_with(prefix)))
}

fn ends_with(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-ends-with", &args[0])?;
    let suffix = want_text("text-ends-with", &args[1])?;
    Ok(Concept::bool(text.ends_with(suffix)))
}

/// Named `text-contains` rather than `contains` so the list operation keeps the
/// short name. Two natives cannot share one concept name, and a list is the
/// thing people reach for `Contains` about first.
fn text_contains(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-contains", &args[0])?;
    let needle = want_text("text-contains", &args[1])?;
    Ok(Concept::bool(text.contains(needle)))
}

/// Character indices, end exclusive. Out-of-range bounds are an error rather
/// than a clamp, for the same reason `Slice` refuses to quietly shorten a list.
fn substring(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-substring", &args[0])?;
    let start = want_count("text-substring", &args[1])?;
    let end = want_count("text-substring", &args[2])?;
    let len = char_count(text);
    if start > len {
        return Err(out_of_range("text-substring", start, len));
    }
    if end > len {
        return Err(out_of_range("text-substring", end, len));
    }
    if start > end {
        return Err(native_error(
            "text-substring",
            format!("start {start} is past end {end}"),
        ));
    }
    let out: String = text.chars().skip(start).take(end - start).collect();
    Ok(Concept::text(out))
}

fn char_at(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-char-at", &args[0])?;
    let index = want_count("text-char-at", &args[1])?;
    match text.chars().nth(index) {
        Some(c) => Ok(Concept::text(c.to_string())),
        None => Err(out_of_range("text-char-at", index, char_count(text))),
    }
}

/// The padding is one character wide so that the result is exactly `width`
/// characters. A multi-character pad would either overshoot or need truncating,
/// and truncating a pad can cut a grapheme in half.
fn pad(native: &str, args: &[Concept], on_left: bool) -> EvalResult {
    let text = want_text(native, &args[0])?;
    let width = want_count(native, &args[1])?;
    let fill = match args.get(2) {
        Some(c) => {
            let s = want_text(native, c)?;
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(one), None) => one,
                _ => {
                    return Err(native_error(
                        native,
                        "the padding must be exactly one character",
                    ));
                }
            }
        }
        None => ' ',
    };
    let len = char_count(text);
    if len >= width {
        return Ok(Concept::text(text));
    }
    if width > MAX_REPEAT_CHARS {
        return Err(native_error(
            native,
            format!("width {width} exceeds the {MAX_REPEAT_CHARS} character cap"),
        ));
    }
    let padding: String = std::iter::repeat_n(fill, width - len).collect();
    Ok(Concept::text(if on_left {
        format!("{padding}{text}")
    } else {
        format!("{text}{padding}")
    }))
}

fn pad_left(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    pad("text-pad-left", args, true)
}

fn pad_right(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    pad("text-pad-right", args, false)
}

/// Capped before anything is allocated. `Repeat<"ab", 10000000000>` is a single
/// small concept that asks for twenty gigabytes, and the evaluator's node
/// budget cannot see it coming because the result is one node either way.
fn repeat(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-repeat", &args[0])?;
    let count = want_count("text-repeat", &args[1])?;
    let total = char_count(text)
        .checked_mul(count)
        .ok_or_else(|| native_error("text-repeat", "requested length overflows"))?;
    if total > MAX_REPEAT_CHARS {
        return Err(native_error(
            "text-repeat",
            format!("{total} characters exceeds the {MAX_REPEAT_CHARS} character cap"),
        ));
    }
    Ok(Concept::text(text.repeat(count)))
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

/// Renders a ground value. A compound is refused, because there is no single
/// right way to flatten `Employment<Greg, Workiva>` into a string: the mouth
/// makes that choice with context this native does not have.
fn to_text(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let ground = args[0]
        .as_ground()
        .ok_or_else(|| type_error("text-to-text", "a ground value", &args[0]))?;
    let out = match ground {
        Ground::Bool(b) => b.to_string(),
        Ground::Int(i) => i.to_string(),
        Ground::Float(f) => f.to_string(),
        Ground::Text(t) => t.to_string(),
        Ground::Bytes(b) => b.iter().map(|byte| format!("{byte:02x}")).collect(),
        Ground::DateTime(d) => d.to_rfc3339(),
        Ground::Json(j) => serde_json::to_string(j.value())
            .map_err(|e| native_error("text-to-text", format!("json will not render: {e}")))?,
    };
    Ok(Concept::text(out))
}

/// Unparseable input is an error, not a zero. `Parse-Int<"abc">` returning `0`
/// would put a number Spoon then reasons from where there was never one.
/// Surrounding whitespace is tolerated, since it carries no meaning either way.
fn parse_int(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-parse-int", &args[0])?;
    text.trim()
        .parse::<i64>()
        .map(Concept::int)
        .map_err(|e| native_error("text-parse-int", format!("{text:?} is not an integer: {e}")))
}

fn parse_float(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-parse-float", &args[0])?;
    text.trim()
        .parse::<f64>()
        .map(Concept::float)
        .map_err(|e| native_error("text-parse-float", format!("{text:?} is not a number: {e}")))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// A string as a list of its characters.
///
/// Characters, not bytes, so a multibyte string comes apart into the pieces a
/// reader would call characters rather than into fragments that cannot be put
/// back together.
fn chars(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("text-chars", &args[0])?;
    Ok(Concept::call(
        "list-list",
        text.chars()
            .map(|c| Concept::text(c.to_string()))
            .collect::<Vec<_>>(),
    ))
}

pub fn register(registry: &mut NativeRegistry) {
    registry.pure(
        "text-chars",
        chars,
        Arity::Exact(1),
        "a string taken apart into a list of one-character strings; use it only when the answer really is a list",
    );
    registry.pure(
        "text-concat",
        concat,
        Arity::Any,
        "several strings joined end to end",
    );
    registry.pure(
        "text-split",
        split,
        Arity::Exact(2),
        "a string cut apart at every separator; gives back a list of strings",
    );
    registry.pure(
        "text-join",
        join,
        Arity::Exact(2),
        "a list of strings run together with a separator between them; gives back a string, so it is how a list of characters becomes a word again",
    );
    registry.pure(
        "text-lines",
        lines,
        Arity::Exact(1),
        "a list of the lines of a string",
    );
    registry.pure("text-upper", upper, Arity::Exact(1), "a string in upper case");
    registry.pure("text-lower", lower, Arity::Exact(1), "a string in lower case");
    registry.pure(
        "text-trim",
        trim,
        Arity::Exact(1),
        "a string without leading or trailing whitespace",
    );
    registry.pure(
        "text-replace",
        replace,
        Arity::Exact(3),
        "a string with every occurrence of one substring swapped for another",
    );
    registry.pure(
        "text-length",
        text_length,
        Arity::Exact(1),
        "how many characters a string has; gives back a number",
    );
    registry.pure(
        "text-starts-with",
        starts_with,
        Arity::Exact(2),
        "whether a string begins with another",
    );
    registry.pure(
        "text-ends-with",
        ends_with,
        Arity::Exact(2),
        "whether a string ends with another",
    );
    registry.pure(
        "text-contains",
        text_contains,
        Arity::Exact(2),
        "whether a string holds another anywhere inside it",
    );
    registry.pure(
        "text-substring",
        substring,
        Arity::Exact(3),
        "part of a string, from a start index up to but not including an end index; gives back a string",
    );
    registry.pure(
        "text-char-at",
        char_at,
        Arity::Exact(2),
        "the character of a string at a zero-based index",
    );
    registry.pure(
        "text-pad-left",
        pad_left,
        Arity::Between(2, 3),
        "a string padded at the front to a width, with spaces or a given character",
    );
    registry.pure(
        "text-pad-right",
        pad_right,
        Arity::Between(2, 3),
        "a string padded at the end to a width, with spaces or a given character",
    );
    registry.pure(
        "text-repeat",
        repeat,
        Arity::Exact(2),
        "a string repeated a number of times",
    );
    registry.pure(
        "text-to-text",
        to_text,
        Arity::Exact(1),
        "the text form of a ground value",
    );
    registry.pure(
        "text-parse-int",
        parse_int,
        Arity::Exact(1),
        "an integer read from a string",
    );
    registry.pure(
        "text-parse-float",
        parse_float,
        Arity::Exact(1),
        "a float read from a string",
    );
}
