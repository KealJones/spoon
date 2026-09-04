//! Lists and the operations over them.
//!
//! A list is `List<a, b, c>`: an ordinary compound whose head is `List`. There
//! is no separate collection type, because there is no separate anything.
//!
//! # Higher-order operations need no machinery
//!
//! A function argument is just a concept. Applying it to an element means
//! building `f<element>` and handing that back to the evaluator, which then
//! runs whatever realization wins for `f`. A `Native`, a learned `Composed`, or
//! a `Rule` all work as a mapper without this module knowing which it got.
//!
//! # Argument order
//!
//! The collection comes first: `Map<List<..>, f>`, `Reduce<List<..>, f, init>`.
//! That matches how the design docs write `Map<Y, X>` and
//! `MaxBy<collection, property>`, and it keeps the reading order of a pipeline
//! the same as the order the data flows.

use std::cmp::Ordering;

use spoon_concept::{Concept, Effect, Ground, SymbolId};
use spoon_eval::{
    ArgStrategy, Arity, Ctx, EvalError, EvalResult, NativeRegistry, native_error, type_error,
};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Read the elements of a list concept.
pub(crate) fn want_list(native: &str, c: &Concept) -> Result<Vec<Concept>, spoon_eval::EvalError> {
    if let Some(head) = c.head_symbol()
        && head == SymbolId::of("list")
    {
        return Ok(c.args().to_vec());
    }
    // A JSON array is a list that happens to have arrived from outside.
    // Refusing it would mean every fetch is followed by a conversion step that
    // exists only to satisfy a type distinction the user never made, and
    // `first<fetch-json<url>>` is exactly what anyone would try first.
    if let Some(spoon_concept::Ground::Json(blob)) = c.as_ground()
        && let Some(items) = blob.value().as_array()
    {
        return Ok(items.iter().map(crate::data::json::from_json).collect());
    }
    Err(type_error(native, "a list", c))
}

/// Build a list concept from elements.
pub(crate) fn make_list(items: Vec<Concept>) -> Concept {
    Concept::call("list", items)
}

/// A non-negative index. Negative indices are rejected rather than wrapped
/// around, because "the -1st element" is a convention, not a meaning, and
/// guessing which convention the caller had in mind is exactly the kind of
/// invention this module avoids.
fn want_index(native: &str, c: &Concept) -> Result<usize, EvalError> {
    let raw = c
        .as_ground()
        .and_then(Ground::as_i64)
        .ok_or_else(|| type_error(native, "an integer index", c))?;
    usize::try_from(raw).map_err(|_| native_error(native, format!("negative index {raw}")))
}

fn want_bool(native: &str, c: &Concept) -> Result<bool, EvalError> {
    c.as_ground()
        .and_then(Ground::as_bool)
        .ok_or_else(|| type_error(native, "a boolean", c))
}

fn out_of_range(native: &str, index: usize, len: usize) -> EvalError {
    native_error(
        native,
        format!("index {index} is out of range for a list of length {len}"),
    )
}

/// Apply a concept to arguments and hand the result back to the evaluator.
///
/// This is the whole of the higher-order machinery. The evaluator does
/// realization selection, budget charging, and effect accounting on the way
/// through, so a mapper that writes to the store is gated exactly where the
/// write happens rather than being pre-approved here.
///
/// A function argument carrying holes is a lambda over them, and its arguments
/// are substituted rather than applied. Without that, the only functions that
/// can be passed are ones with names, so "keep the ones equal to r" is
/// inexpressible: `filter<chars<"strawberry">, eq<?0, "r">>` is the obvious way
/// to write it and applying `eq<?0, "r">` to an element would produce a
/// compound with a compound head, which means nothing.
///
/// This is not a new idea in the system. A `Composed` realization is already a
/// body whose holes bind positionally to the call's arguments, so a concept
/// with holes already means a function everywhere else. Treating it as one here
/// removes a special case rather than adding one.
fn call(ctx: &mut dyn Ctx, f: &Concept, args: Vec<Concept>) -> EvalResult {
    if !spoon_concept::holes(f).is_empty() {
        let bound = spoon_concept::substitute_positional(f, &args);
        return ctx.eval(&bound);
    }
    ctx.eval(&Concept::apply(f.clone(), args))
}

// ---------------------------------------------------------------------------
// Construction and access
// ---------------------------------------------------------------------------

fn list(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(make_list(args.to_vec()))
}

fn first(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("first", &args[0])?;
    items
        .first()
        .cloned()
        .ok_or_else(|| out_of_range("first", 0, 0))
}

fn last(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("last", &args[0])?;
    items
        .last()
        .cloned()
        .ok_or_else(|| out_of_range("last", 0, 0))
}

fn nth(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("nth", &args[0])?;
    let index = want_index("nth", &args[1])?;
    items
        .get(index)
        .cloned()
        .ok_or_else(|| out_of_range("nth", index, items.len()))
}

fn count(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::int(want_list("count", &args[0])?.len() as i64))
}

fn is_empty(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(want_list("is-empty", &args[0])?.is_empty()))
}

fn append(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut items = want_list("append", &args[0])?.to_vec();
    items.extend(args[1..].iter().cloned());
    Ok(make_list(items))
}

/// Extra arguments keep their written order at the front, so
/// `Prepend<List<3>, 1, 2>` is `List<1, 2, 3>` rather than the reversal a naive
/// push-one-at-a-time loop would give.
fn prepend(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut items: Vec<Concept> = args[1..].to_vec();
    items.extend(want_list("prepend", &args[0])?.iter().cloned());
    Ok(make_list(items))
}

fn concat_lists(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut items = Vec::new();
    for a in args {
        items.extend(want_list("concat-lists", a)?.iter().cloned());
    }
    Ok(make_list(items))
}

/// Reversing text gives back text.
///
/// `reverse` was list-only, so "reverse the word banana" got as far as
/// `reverse<"banana">` and stopped, and going through `chars` gave back
/// `list<"a", "n", ...>` unless something remembered to join it. Both are the
/// same failure: the answer to a question about a word is a word.
///
/// This is not a special case for strings. It is one native that knows the
/// shape of its argument, the same way `want_list` already accepts a JSON
/// array because refusing it would mean a conversion step nobody asked for.
///
/// Reversed by character, not by byte, so a word with an accent in it comes
/// back as a word rather than as broken bytes.
fn reverse(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    if let Some(text) = args[0].as_ground().and_then(|g| g.as_str()) {
        return Ok(Concept::text(text.chars().rev().collect::<String>()));
    }
    let mut items = want_list("reverse", &args[0])?.to_vec();
    items.reverse();
    Ok(make_list(items))
}

/// Bounds are checked rather than clamped. A slice that silently shrinks to fit
/// hands back a shorter list than the caller asked for and says nothing, which
/// is how an off-by-one survives to production.
fn slice(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("slice", &args[0])?;
    let start = want_index("slice", &args[1])?;
    let end = want_index("slice", &args[2])?;
    if start > items.len() {
        return Err(out_of_range("slice", start, items.len()));
    }
    if end > items.len() {
        return Err(out_of_range("slice", end, items.len()));
    }
    if start > end {
        return Err(native_error(
            "slice",
            format!("start {start} is past end {end}"),
        ));
    }
    Ok(make_list(items[start..end].to_vec()))
}

fn contains(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("contains", &args[0])?;
    Ok(Concept::bool(items.contains(&args[1])))
}

/// Absence is an error, not `-1`. A sentinel index is a value the caller can
/// forget to check and then use, and `Contains` already answers the question
/// "is it in there" without needing one.
fn index_of(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("index-of", &args[0])?;
    match items.iter().position(|item| item == &args[1]) {
        Some(index) => Ok(Concept::int(index as i64)),
        None => Err(native_error(
            "index-of",
            format!("no matching element in a list of length {}", items.len()),
        )),
    }
}

fn unique(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut seen: Vec<Concept> = Vec::new();
    for item in &want_list("unique", &args[0])? {
        if !seen.contains(item) {
            seen.push(item.clone());
        }
    }
    Ok(make_list(seen))
}

/// One level, deliberately. A recursive flatten cannot be undone and cannot be
/// asked for partially, whereas one level composes: apply it twice for two.
fn flatten(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut items = Vec::new();
    for item in &want_list("flatten", &args[0])? {
        match want_list("flatten", item) {
            Ok(inner) => items.extend(inner.iter().cloned()),
            Err(_) => items.push(item.clone()),
        }
    }
    Ok(make_list(items))
}

// ---------------------------------------------------------------------------
// Higher-order
// ---------------------------------------------------------------------------

fn map(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("map", &args[0])?.to_vec();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(call(ctx, &args[1], vec![item])?);
    }
    Ok(make_list(out))
}

fn filter(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("filter", &args[0])?.to_vec();
    let mut out = Vec::new();
    for item in items {
        let verdict = call(ctx, &args[1], vec![item.clone()])?;
        if want_bool("filter", &verdict)? {
            out.push(item);
        }
    }
    Ok(make_list(out))
}

/// The accumulator leads: `f<acc, item>`. An explicit initial value is required
/// rather than defaulting to the first element, because a fold with no seed has
/// no answer for an empty list and would have to invent one.
fn reduce(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("reduce", &args[0])?.to_vec();
    let mut acc = args[2].clone();
    for item in items {
        acc = call(ctx, &args[1], vec![acc, item])?;
    }
    Ok(acc)
}

fn find(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("find", &args[0])?.to_vec();
    let len = items.len();
    for item in items {
        let verdict = call(ctx, &args[1], vec![item.clone()])?;
        if want_bool("find", &verdict)? {
            return Ok(item);
        }
    }
    Err(native_error(
        "find",
        format!("no element satisfied the predicate in a list of length {len}"),
    ))
}

/// Vacuously true on an empty list. "Every element satisfies it" is a claim
/// about elements, and there are none to violate it.
fn all(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("all", &args[0])?.to_vec();
    for item in items {
        let verdict = call(ctx, &args[1], vec![item])?;
        if !want_bool("all", &verdict)? {
            return Ok(Concept::bool(false));
        }
    }
    Ok(Concept::bool(true))
}

fn any(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("any", &args[0])?.to_vec();
    for item in items {
        let verdict = call(ctx, &args[1], vec![item])?;
        if want_bool("any", &verdict)? {
            return Ok(Concept::bool(true));
        }
    }
    Ok(Concept::bool(false))
}

/// A key that can be put in a total order.
///
/// Ordering across kinds is not defined: there is no honest answer to whether
/// `3` sorts before `"apple"`, so a mixed key set is an error rather than an
/// arbitrary-but-stable ordering that would look correct until it mattered.
#[derive(Debug, Clone, PartialEq)]
enum SortKey {
    Bool(bool),
    Num(f64),
    Text(String),
    Time(i64),
}

impl SortKey {
    fn kind(&self) -> &'static str {
        match self {
            SortKey::Bool(_) => "boolean",
            SortKey::Num(_) => "number",
            SortKey::Text(_) => "text",
            SortKey::Time(_) => "datetime",
        }
    }

    fn compare(&self, other: &SortKey) -> Ordering {
        match (self, other) {
            (SortKey::Bool(a), SortKey::Bool(b)) => a.cmp(b),
            (SortKey::Num(a), SortKey::Num(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (SortKey::Text(a), SortKey::Text(b)) => a.cmp(b),
            (SortKey::Time(a), SortKey::Time(b)) => a.cmp(b),
            // Unreachable once the keys have been checked for a single kind.
            // Treating it as a tie keeps the sort total instead of panicking.
            _ => Ordering::Equal,
        }
    }
}

fn sort_key(native: &str, c: &Concept) -> Result<SortKey, EvalError> {
    let ground = c
        .as_ground()
        .ok_or_else(|| type_error(native, "a comparable ground value", c))?;
    match ground {
        Ground::Bool(b) => Ok(SortKey::Bool(*b)),
        Ground::Int(i) => Ok(SortKey::Num(*i as f64)),
        Ground::Float(f) => {
            if f.is_nan() {
                Err(native_error(
                    native,
                    "a key evaluated to NaN, which has no place in an order",
                ))
            } else {
                Ok(SortKey::Num(*f))
            }
        }
        Ground::Text(t) => Ok(SortKey::Text(t.to_string())),
        Ground::DateTime(d) => Ok(SortKey::Time(d.timestamp_nanos_opt().unwrap_or(0))),
        Ground::Bytes(_) | Ground::Json(_) => {
            Err(type_error(native, "a comparable ground value", c))
        }
    }
}

/// Sorting by the elements themselves.
///
/// `sort-by<xs, ?0>` already said this, and expecting everyone to know that is
/// how "sort 18, 28, 11, 38" came back unsorted: the ears reached for the name
/// anybody would reach for, found nothing, and produced a `sort-by` whose key
/// function was wrong. A name people actually use is worth a native.
fn sort(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("sort", &args[0])?.to_vec();
    let mut keyed: Vec<(SortKey, Concept)> = Vec::with_capacity(items.len());
    for item in items {
        keyed.push((sort_key("sort", &item)?, item));
    }
    if let Some((first, _)) = keyed.first() {
        let kind = first.kind();
        if let Some((odd, _)) = keyed.iter().find(|(k, _)| k.kind() != kind) {
            return Err(native_error(
                "sort",
                format!(
                    "values mix {kind} and {}, which have no shared order",
                    odd.kind()
                ),
            ));
        }
    }
    keyed.sort_by(|(a, _), (b, _)| a.compare(b));
    Ok(make_list(keyed.into_iter().map(|(_, item)| item).collect()))
}

/// Stable and deterministic. Keys are computed once up front rather than inside
/// the comparator, so a key function with a trace or a store read runs exactly
/// once per element and the sort cannot depend on comparison order.
fn sort_by(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("sort-by", &args[0])?.to_vec();
    let mut keyed: Vec<(SortKey, Concept)> = Vec::with_capacity(items.len());
    for item in items {
        let key = call(ctx, &args[1], vec![item.clone()])?;
        keyed.push((sort_key("sort-by", &key)?, item));
    }
    if let Some((first, _)) = keyed.first() {
        let kind = first.kind();
        if let Some((odd, _)) = keyed.iter().find(|(k, _)| k.kind() != kind) {
            return Err(native_error(
                "sort-by",
                format!(
                    "keys mix {kind} and {}, which have no shared order",
                    odd.kind()
                ),
            ));
        }
    }
    keyed.sort_by(|(a, _), (b, _)| a.compare(b));
    Ok(make_list(keyed.into_iter().map(|(_, item)| item).collect()))
}

/// Groups keep first-appearance order, and so do the members inside each group.
/// The shape is `List<Group<key, List<..>>, ..>`: a group is a compound like
/// anything else, so it can be mapped over and taken apart with `Nth`.
fn group_by(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("group-by", &args[0])?.to_vec();
    let mut groups: Vec<(Concept, Vec<Concept>)> = Vec::new();
    for item in items {
        let key = call(ctx, &args[1], vec![item.clone()])?;
        match groups.iter_mut().find(|(k, _)| k == &key) {
            Some((_, members)) => members.push(item),
            None => groups.push((key, vec![item])),
        }
    }
    Ok(make_list(
        groups
            .into_iter()
            .map(|(key, members)| Concept::call("group", [key, make_list(members)]))
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// Numeric folds
// ---------------------------------------------------------------------------

/// Read a list of numbers as a widened `f64` view, plus the exact integer view
/// when every element was an `Int`.
///
/// Int and Float stay distinct identities everywhere else; only arithmetic
/// looks past the difference, and it does so by widening: all-Int gives Int,
/// one Float anywhere gives Float. The integer view is what keeps `Sum` of
/// large integers exact instead of rounding through `f64`.
type Numbers = (Vec<f64>, Option<Vec<i64>>);

fn want_numbers(native: &str, c: &Concept) -> Result<Numbers, EvalError> {
    let items = want_list(native, c)?;
    let mut floats = Vec::with_capacity(items.len());
    let mut ints = Vec::with_capacity(items.len());
    let mut any_float = false;
    for item in items.iter() {
        match item.as_ground() {
            Some(Ground::Int(i)) => {
                ints.push(*i);
                floats.push(*i as f64);
            }
            Some(Ground::Float(f)) => {
                any_float = true;
                floats.push(*f);
            }
            _ => return Err(type_error(native, "a list of numbers", item)),
        }
    }
    Ok((floats, if any_float { None } else { Some(ints) }))
}

/// Empty sums to `0`, the additive identity, because that is the only value
/// that leaves every other sum unchanged when the empty list is concatenated in.
fn sum(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let (floats, ints) = want_numbers("sum", &args[0])?;
    let Some(ints) = ints else {
        return Ok(Concept::float(floats.iter().sum()));
    };
    let mut total = 0i64;
    for v in ints {
        total = total
            .checked_add(v)
            .ok_or_else(|| native_error("sum", "integer overflow"))?;
    }
    Ok(Concept::int(total))
}

/// Empty multiplies to `1`, for the same reason `Sum<List<>>` is `0`.
fn product(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let (floats, ints) = want_numbers("product", &args[0])?;
    let Some(ints) = ints else {
        return Ok(Concept::float(floats.iter().product()));
    };
    let mut total = 1i64;
    for v in ints {
        total = total
            .checked_mul(v)
            .ok_or_else(|| native_error("product", "integer overflow"))?;
    }
    Ok(Concept::int(total))
}

/// `min-of` and `max-of` share everything but the comparison.
///
/// An empty list is an error rather than zero or infinity: there is no smallest
/// element of nothing, and any value returned would be a lie the caller cannot
/// distinguish from a real answer.
fn extremum(native: &str, args: &[Concept], want_greater: bool) -> EvalResult {
    let (floats, ints) = want_numbers(native, &args[0])?;
    if floats.is_empty() {
        return Err(native_error(native, "an empty list has no extreme value"));
    }
    if floats.iter().any(|v| v.is_nan()) {
        return Err(native_error(native, "NaN has no place in an order"));
    }
    let mut best = 0usize;
    for (i, v) in floats.iter().enumerate() {
        let better = if want_greater {
            *v > floats[best]
        } else {
            *v < floats[best]
        };
        if better {
            best = i;
        }
    }
    match ints {
        Some(ints) => Ok(Concept::int(ints[best])),
        None => Ok(Concept::float(floats[best])),
    }
}

fn min_of(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    extremum("min-of", args, false)
}

fn max_of(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    extremum("max-of", args, true)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// The integers from one bound to another.
///
/// Without this there is no way to say "do something for each number", because
/// every collection operation needs a list to start from and nothing in the
/// language produces one from a count. `Map` over a range is how iteration is
/// spelled here; there is no loop construct and there does not need to be.
///
/// The upper bound is inclusive, because `range<1, 100>` is what someone asking
/// for one to a hundred means, and off-by-one at the language level is a tax on
/// every use.
fn range(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let (from, to) = match args.len() {
        1 => (1i64, want_int_arg("range", &args[0])?),
        _ => (
            want_int_arg("range", &args[0])?,
            want_int_arg("range", &args[1])?,
        ),
    };
    if to < from {
        return Ok(make_list(Vec::new()));
    }
    // A range is materialized, so an unbounded one is a memory exhaustion the
    // evaluator's node budget cannot see: the whole list is built inside one
    // native before any budget is charged.
    const MAX: i64 = 1_000_000;
    if to - from >= MAX {
        return Err(spoon_eval::native_error(
            "range",
            format!("{} values is over the {MAX} cap", to - from + 1),
        ));
    }
    Ok(make_list((from..=to).map(Concept::int).collect()))
}

fn want_int_arg(native: &str, c: &Concept) -> Result<i64, spoon_eval::EvalError> {
    c.as_ground()
        .and_then(spoon_concept::Ground::as_i64)
        .ok_or_else(|| type_error(native, "an integer", c))
}

pub fn register(registry: &mut NativeRegistry) {
    registry.pure(
        "range",
        range,
        Arity::Between(1, 2),
        "the integers between two bounds, both included",
    );
    registry.pure(
        "list",
        list,
        Arity::Any,
        "build a list from the given elements",
    );
    registry.pure(
        "first",
        first,
        Arity::Exact(1),
        "the first element of a list; for the first letters of a word use substring",
    );
    registry.pure("last", last, Arity::Exact(1), "the last element of a list");
    registry.pure(
        "nth",
        nth,
        Arity::Exact(2),
        "the element of a list at a zero-based index",
    );
    registry.pure(
        "count",
        count,
        Arity::Exact(1),
        "number of elements in a list",
    );
    registry.pure(
        "is-empty",
        is_empty,
        Arity::Exact(1),
        "whether a list has no elements",
    );
    registry.pure(
        "append",
        append,
        Arity::AtLeast(1),
        "a list with elements added at the end",
    );
    registry.pure(
        "prepend",
        prepend,
        Arity::AtLeast(1),
        "a list with elements added at the front, in the order given",
    );
    registry.pure(
        "concat-lists",
        concat_lists,
        Arity::Any,
        "several lists joined into one",
    );
    registry.pure(
        "reverse",
        reverse,
        Arity::Exact(1),
        "a list, or a piece of text, in the opposite order; text gives back text",
    );
    registry.pure(
        "sort",
        sort,
        Arity::Exact(1),
        "a list in ascending order",
    );
    registry.pure(
        "slice",
        slice,
        Arity::Exact(3),
        "part of a list, from a start index up to but not including an end index; gives back a list",
    );
    registry.pure(
        "contains",
        contains,
        Arity::Exact(2),
        "whether a list holds a given element",
    );
    registry.pure(
        "index-of",
        index_of,
        Arity::Exact(2),
        "the position of an element in a list, or an error when it is absent",
    );
    registry.pure(
        "unique",
        unique,
        Arity::Exact(1),
        "a list with duplicates removed, keeping first appearances",
    );
    registry.pure(
        "flatten",
        flatten,
        Arity::Exact(1),
        "a list with any inner lists spliced in, one level deep",
    );

    registry.register(
        "map",
        map,
        Arity::Exact(2),
        ArgStrategy::Selective(0b001),
        Effect::Pure,
        "apply a function to every element of a list",
    );
    registry.register(
        "filter",
        filter,
        Arity::Exact(2),
        ArgStrategy::Selective(0b001),
        Effect::Pure,
        "the elements of a list for which a predicate holds",
    );
    registry.register(
        "reduce",
        reduce,
        Arity::Exact(3),
        ArgStrategy::Selective(0b101),
        Effect::Pure,
        "fold a list into one value with a function and an initial value",
    );
    registry.register(
        "sort-by",
        sort_by,
        Arity::Exact(2),
        ArgStrategy::Selective(0b001),
        Effect::Pure,
        "a list ordered by a key function, stably and deterministically",
    );
    registry.register(
        "find",
        find,
        Arity::Exact(2),
        ArgStrategy::Selective(0b001),
        Effect::Pure,
        "the first element of a list satisfying a predicate",
    );
    registry.register(
        "all",
        all,
        Arity::Exact(2),
        ArgStrategy::Selective(0b001),
        Effect::Pure,
        "whether every element satisfies a predicate",
    );
    registry.register(
        "any",
        any,
        Arity::Exact(2),
        ArgStrategy::Selective(0b001),
        Effect::Pure,
        "whether some element satisfies a predicate",
    );
    registry.pure(
        "sum",
        sum,
        Arity::Exact(1),
        "the total of a list of numbers",
    );
    registry.pure(
        "product",
        product,
        Arity::Exact(1),
        "the product of a list of numbers",
    );
    registry.pure(
        "min-of",
        min_of,
        Arity::Exact(1),
        "the smallest of a list of numbers",
    );
    registry.pure(
        "max-of",
        max_of,
        Arity::Exact(1),
        "the largest of a list of numbers",
    );
    registry.register(
        "group-by",
        group_by,
        Arity::Exact(2),
        ArgStrategy::Selective(0b001),
        Effect::Pure,
        "a list of Group<key, List<..>> collecting elements by a key function",
    );
}
