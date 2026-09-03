//! Arithmetic, comparison, and logic.
//!
//! # The widening rule
//!
//! `Int` and `Float` are distinct concept identities. `42` is not `42.0`, they
//! digest differently, and `Eq<42, 42.0>` is false. Arithmetic still has to
//! work across the two, so every numeric native here follows one rule:
//!
//! > If every argument is an `Int`, the result is an `Int`. If any argument is
//! > a `Float`, the operation widens to `f64` and the result is a `Float`.
//!
//! The rule is applied per operation, not per expression, so `Add<1, 2.5, 3>`
//! widens at the first float and stays widened: `6.5`. Two integers divide to
//! an integer, which means `Div<7, 2>` is `3`. Ask for `Div<7.0, 2>` when you
//! want `3.5`. Comparisons follow the same widening for their operands but
//! always produce a `Bool`.
//!
//! # Failure is asymmetric between integers and floats
//!
//! Integer arithmetic uses the `checked_*` family throughout. Overflow, and
//! integer division or modulo by zero, are returned as a `native_error`. They
//! are never wrapped and never panic, because a silently wrapped answer is a
//! wrong answer Spoon would then go on to reason from.
//!
//! Floats are different, and deliberately so. IEEE 754 already defines what
//! `1.0 / 0.0` and `0.0 / 0.0` mean, `Ground::Float` hashes and serializes
//! `inf` and `NaN` without losing them, and every comparison here treats `NaN`
//! as unordered rather than pretending it sorts. So float division by zero is
//! not an error: it produces `inf` or `NaN` and those flow onward as ordinary
//! concepts. Integers have no such value to produce, which is the whole of the
//! asymmetry.

use std::cmp::Ordering;

use spoon_concept::{Concept, Effect, Ground};
use spoon_eval::{
    ArgStrategy, Arity, Ctx, EvalError, EvalResult, NativeRegistry, native_error, type_error,
};

// ---------------------------------------------------------------------------
// Reading arguments
// ---------------------------------------------------------------------------

/// Pull an integer out of a concept, or say precisely what was wrong.
pub(crate) fn want_int(native: &str, c: &Concept) -> Result<i64, EvalError> {
    c.as_ground()
        .and_then(Ground::as_i64)
        .ok_or_else(|| type_error(native, "an integer", c))
}

/// Numbers widen to f64 for comparison and division. `Int` and `Float` remain
/// distinct identities: 42 is not 42.0, and only the arithmetic looks past that.
pub(crate) fn want_number(native: &str, c: &Concept) -> Result<f64, EvalError> {
    c.as_ground()
        .and_then(Ground::as_f64)
        .ok_or_else(|| type_error(native, "a number", c))
}

pub(crate) fn want_bool(native: &str, c: &Concept) -> Result<bool, EvalError> {
    c.as_ground()
        .and_then(Ground::as_bool)
        .ok_or_else(|| type_error(native, "a boolean", c))
}

/// A number that still remembers which kind of number it was.
///
/// [`want_number`] throws that away, which is right for a predicate that only
/// needs a magnitude and wrong for an operation whose result kind depends on
/// its inputs. Everything that has to honour the widening rule reads its
/// arguments through here instead.
#[derive(Debug, Clone, Copy)]
enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    fn of(native: &str, c: &Concept) -> Result<Num, EvalError> {
        match c.as_ground() {
            Some(Ground::Int(i)) => Ok(Num::Int(*i)),
            Some(Ground::Float(f)) => Ok(Num::Float(*f)),
            _ => Err(type_error(native, "a number", c)),
        }
    }

    fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Num::Float(_))
    }

    fn widen(self) -> Num {
        Num::Float(self.as_f64())
    }

    fn into_concept(self) -> Concept {
        match self {
            Num::Int(i) => Concept::int(i),
            Num::Float(f) => Concept::float(f),
        }
    }
}

/// The evaluator checks arity before it calls a native, so a missing argument
/// here means a realization was stored with an arity that disagrees with the
/// registry. That is a bug worth reporting, not worth panicking over: every
/// argument access in this module goes through `one` or `pair`.
fn one<'a>(native: &str, args: &'a [Concept]) -> Result<&'a Concept, EvalError> {
    args.first()
        .ok_or_else(|| native_error(native, "expected one argument"))
}

fn pair<'a>(native: &str, args: &'a [Concept]) -> Result<(&'a Concept, &'a Concept), EvalError> {
    match (args.first(), args.get(1)) {
        (Some(a), Some(b)) => Ok((a, b)),
        _ => Err(native_error(native, "expected two arguments")),
    }
}

// ---------------------------------------------------------------------------
// The widening rule, in one place
// ---------------------------------------------------------------------------

/// Combine two numbers under the widening rule.
///
/// `on_ints` runs only when both sides are integers and reports its own failure
/// text, because "integer overflow" and "division by zero" are different
/// stories and the caller should get the right one. Any float on either side
/// sends the pair down `on_floats`, where IEEE semantics apply and nothing can
/// fail.
fn combine(
    native: &str,
    a: Num,
    b: Num,
    on_ints: fn(i64, i64) -> Result<i64, &'static str>,
    on_floats: fn(f64, f64) -> f64,
) -> Result<Num, EvalError> {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => on_ints(x, y)
            .map(Num::Int)
            .map_err(|why| native_error(native, why)),
        _ => Ok(Num::Float(on_floats(a.as_f64(), b.as_f64()))),
    }
}

/// Left fold for the variadic operations. There is no identity element to seed
/// with: `Add<>` would have to invent `0` as an `Int` and silently break the
/// widening rule for a float-only call, so the arity requires an argument and
/// the first one is the seed.
fn fold(
    native: &str,
    args: &[Concept],
    on_ints: fn(i64, i64) -> Result<i64, &'static str>,
    on_floats: fn(f64, f64) -> f64,
) -> EvalResult {
    let mut acc = Num::of(native, one(native, args)?)?;
    for arg in args.iter().skip(1) {
        acc = combine(native, acc, Num::of(native, arg)?, on_ints, on_floats)?;
    }
    Ok(acc.into_concept())
}

fn binary(
    native: &str,
    args: &[Concept],
    on_ints: fn(i64, i64) -> Result<i64, &'static str>,
    on_floats: fn(f64, f64) -> f64,
) -> EvalResult {
    let (a, b) = pair(native, args)?;
    let combined = combine(
        native,
        Num::of(native, a)?,
        Num::of(native, b)?,
        on_ints,
        on_floats,
    )?;
    Ok(combined.into_concept())
}

const OVERFLOW: &str = "integer overflow";

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

/// Variadic because summing a list is the common case and nesting `Add` two at
/// a time makes a deeper term for the evaluator to walk for no gain.
fn add(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    fold(
        "add",
        args,
        |x, y| x.checked_add(y).ok_or(OVERFLOW),
        |x, y| x + y,
    )
}

/// Binary, unlike `add`. Subtraction is not associative, so a variadic form
/// would have to pick a grouping and then quietly mean something the caller did
/// not write. `Neg` covers the unary case.
fn sub(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    binary(
        "sub",
        args,
        |x, y| x.checked_sub(y).ok_or(OVERFLOW),
        |x, y| x - y,
    )
}

fn mul(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    fold(
        "mul",
        args,
        |x, y| x.checked_mul(y).ok_or(OVERFLOW),
        |x, y| x * y,
    )
}

/// Two integers divide to an integer, truncating toward zero, because the
/// widening rule says an all-integer call produces an integer. Dividing by a
/// zero integer is an error; dividing by a zero float is `inf`, which IEEE
/// already defines and `Ground::Float` round-trips.
///
/// `checked_div` also catches `i64::MIN / -1`, whose true answer does not fit
/// in an `i64`.
fn div(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    binary(
        "div",
        args,
        |x, y| {
            if y == 0 {
                Err("division by zero")
            } else {
                x.checked_div(y).ok_or(OVERFLOW)
            }
        },
        |x, y| x / y,
    )
}

/// Remainder, whose sign follows the dividend: `Modulo<-7, 3>` is `-1`. That is
/// the rule `%` uses in Rust, C, Java, and Go, and matching float `%` keeps the
/// two paths of the widening rule agreeing on the same answer. Euclidean
/// modulo, which would give `2`, is a different operation and would deserve a
/// different concept.
fn modulo(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    binary(
        "modulo",
        args,
        |x, y| {
            if y == 0 {
                Err("modulo by zero")
            } else {
                x.checked_rem(y).ok_or(OVERFLOW)
            }
        },
        |x, y| x % y,
    )
}

/// `checked_neg` matters here: `-i64::MIN` has no `i64` answer, and negating it
/// with `-` would wrap straight back to `i64::MIN`.
fn neg(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let value = match Num::of("neg", one("neg", args)?)? {
        Num::Int(i) => Num::Int(
            i.checked_neg()
                .ok_or_else(|| native_error("neg", OVERFLOW))?,
        ),
        Num::Float(f) => Num::Float(-f),
    };
    Ok(value.into_concept())
}

/// Same overflow story as `neg`: `abs(i64::MIN)` does not fit.
fn abs(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let value = match Num::of("abs", one("abs", args)?)? {
        Num::Int(i) => Num::Int(
            i.checked_abs()
                .ok_or_else(|| native_error("abs", OVERFLOW))?,
        ),
        Num::Float(f) => Num::Float(f.abs()),
    };
    Ok(value.into_concept())
}

/// A negative exponent on two integers is refused rather than quietly widened.
/// `Pow<2, -1>` is `0.5`, which is not an integer, and the widening rule
/// promises an all-integer call an integer answer. Breaking that promise
/// silently would be worse than saying what to do instead.
fn pow(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let (base, exponent) = pair("pow", args)?;
    let base = Num::of("pow", base)?;
    let exponent = Num::of("pow", exponent)?;
    match (base, exponent) {
        (Num::Int(b), Num::Int(e)) => {
            if e < 0 {
                return Err(native_error(
                    "pow",
                    "a negative exponent has no integer answer; make the base a float",
                ));
            }
            let e = u32::try_from(e).map_err(|_| native_error("pow", OVERFLOW))?;
            b.checked_pow(e)
                .map(Concept::int)
                .ok_or_else(|| native_error("pow", OVERFLOW))
        }
        _ => Ok(Concept::float(base.as_f64().powf(exponent.as_f64()))),
    }
}

/// `min` and `max` differ only in which side of the comparison wins, so they
/// share a body.
///
/// Two integers compare as integers. Anything else compares as `f64`, and if
/// that comparison is undefined the answer is `NaN`: a set containing `NaN` has
/// no smallest and no largest member, and returning one of the other elements
/// would be inventing an ordering that does not exist. `f64::min` makes the
/// opposite choice and ignores `NaN`; that is convenient and it is a lie.
///
/// The widening happens once at the end so that `Min<2.5, 1>` is `1.0` and not
/// `1`, even though the integer is the one that won.
fn extremum(native: &str, args: &[Concept], want_smaller: bool) -> EvalResult {
    let mut acc = Num::of(native, one(native, args)?)?;
    let mut saw_float = acc.is_float();
    for arg in args.iter().skip(1) {
        let next = Num::of(native, arg)?;
        saw_float |= next.is_float();
        acc = match (acc, next) {
            (Num::Int(a), Num::Int(b)) => Num::Int(if (a < b) == want_smaller { a } else { b }),
            _ => match acc.as_f64().partial_cmp(&next.as_f64()) {
                Some(Ordering::Less) => {
                    if want_smaller {
                        acc
                    } else {
                        next
                    }
                }
                Some(_) => {
                    if want_smaller {
                        next
                    } else {
                        acc
                    }
                }
                None => Num::Float(f64::NAN),
            },
        };
    }
    let result = if saw_float { acc.widen() } else { acc };
    Ok(result.into_concept())
}

fn min(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    extremum("min", args, true)
}

fn max(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    extremum("max", args, false)
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// Identity, not numeric equality.
///
/// Two concepts are equal when they are the same concept, which is exactly what
/// `content_id` decides and exactly what the store indexes on. So
/// `Eq<Greg, Greg>` is true, `Eq<List<1>, List<1>>` is true, and
/// `Eq<42, 42.0>` is false: an `Int` and a `Float` are different concepts, and
/// the widening rule applies to arithmetic, not to identity. Use `Lte` and
/// `Gte` together when you want "same number, either spelling".
fn eq(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let (a, b) = pair("eq", args)?;
    Ok(Concept::bool(a.content_id() == b.content_id()))
}

fn ne(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let (a, b) = pair("ne", args)?;
    Ok(Concept::bool(a.content_id() != b.content_id()))
}

/// Ordering is numeric only. `Lt<Greg, Keal>` is a type error rather than an
/// alphabetical guess, because Spoon should say it does not understand instead
/// of picking an ordering the caller never asked for.
///
/// Two integers compare as integers instead of widening: past 2^53 an `f64`
/// cannot tell adjacent integers apart, and `Lt` returning false for two
/// genuinely different numbers would be a quiet wrong answer.
///
/// `NaN` is unordered, so every ordering comparison against it is false,
/// including `Lte<NaN, NaN>`. That is IEEE behaviour and it is why the
/// undefined case maps to false rather than to an error.
fn compare(native: &str, args: &[Concept], accept: fn(Ordering) -> bool) -> EvalResult {
    let (a, b) = pair(native, args)?;
    let a = Num::of(native, a)?;
    let b = Num::of(native, b)?;
    let ordering = match (a, b) {
        (Num::Int(x), Num::Int(y)) => Some(x.cmp(&y)),
        _ => a.as_f64().partial_cmp(&b.as_f64()),
    };
    Ok(Concept::bool(ordering.is_some_and(accept)))
}

fn lt(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    compare("lt", args, Ordering::is_lt)
}

fn gt(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    compare("gt", args, Ordering::is_gt)
}

fn lte(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    compare("lte", args, Ordering::is_le)
}

fn gte(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    compare("gte", args, Ordering::is_ge)
}

// ---------------------------------------------------------------------------
// Logic
// ---------------------------------------------------------------------------

/// Lazy, and it has to be. `And<false, Boom<>>` is false, and it can only be
/// false if `Boom` never runs. Registering this eagerly would have the
/// evaluator reduce every argument first, so the short circuit would be a
/// comment rather than a behaviour.
///
/// Variadic and stops at the first false. Each argument is checked for being a
/// boolean as it is reduced, which means a non-boolean sitting after a decisive
/// false is never looked at. That follows from short-circuiting rather than
/// contradicting the type check: an argument that did not run has no type.
fn and(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    for arg in args {
        let value = ctx.eval(arg)?;
        if !want_bool("and", &value)? {
            return Ok(Concept::bool(false));
        }
    }
    Ok(Concept::bool(true))
}

/// The mirror of `and`: lazy, variadic, stops at the first true.
fn or(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    for arg in args {
        let value = ctx.eval(arg)?;
        if want_bool("or", &value)? {
            return Ok(Concept::bool(true));
        }
    }
    Ok(Concept::bool(false))
}

fn not(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(!want_bool("not", one("not", args)?)?))
}

/// Eager, unlike `and` and `or`. Exclusive or has no decisive argument: both
/// sides are needed before the answer is known, so laziness would buy nothing
/// and only make the reduction order harder to reason about.
fn xor(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let (a, b) = pair("xor", args)?;
    Ok(Concept::bool(want_bool("xor", a)? != want_bool("xor", b)?))
}

/// Reduces the condition and exactly one branch.
///
/// The untaken branch is never passed to `ctx.eval`, so it costs no budget and
/// cannot fail. That is the whole point: `If<true, 7, Boom<>>` is 7.
///
/// A non-boolean condition is a type error, not a truthiness coercion. Deciding
/// that a non-empty list, or `0`, or `Greg` "means true" is a guess dressed up
/// as a rule, and it silently sends evaluation down a branch nobody chose.
/// Spoon should say it does not understand.
fn conditional(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let condition = ctx.eval(one("if", args)?)?;
    let taken = if want_bool("if", &condition)? {
        args.get(1)
    } else {
        args.get(2)
    };
    let taken = taken.ok_or_else(|| native_error("if", "expected a condition and two branches"))?;
    ctx.eval(taken)
}

// ---------------------------------------------------------------------------
// Numeric predicates
// ---------------------------------------------------------------------------

/// These three only need a magnitude, so they read through `want_number` and
/// accept either kind of number. `IsZero<0>` and `IsZero<0.0>` are both true
/// even though `0` and `0.0` are different concepts, because this asks about
/// the value rather than the identity.
///
/// The comparisons are strict, so zero is neither positive nor negative and
/// `NaN` is neither. Nothing here can fail on a number.
fn is_zero(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(
        want_number("is-zero", one("is-zero", args)?)? == 0.0,
    ))
}

fn is_positive(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(
        want_number("is-positive", one("is-positive", args)?)? > 0.0,
    ))
}

fn is_negative(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(
        want_number("is-negative", one("is-negative", args)?)? < 0.0,
    ))
}

/// Integers only. Parity is a property of integers, and `IsEven<2.0>` would
/// have to decide what to do with `2.5` first. Refusing the float is clearer
/// than picking a rounding rule on the caller's behalf.
///
/// Sign does not affect the answer: `-3 % 2` is `-1`, which is still not zero,
/// so `-3` is odd.
fn is_even(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(
        want_int("is-even", one("is-even", args)?)? % 2 == 0,
    ))
}

fn is_odd(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(
        want_int("is-odd", one("is-odd", args)?)? % 2 != 0,
    ))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register(registry: &mut NativeRegistry) {
    // Arithmetic. All pure and eager: nothing here reads the world, and every
    // argument is needed.
    registry.pure(
        "add",
        add,
        Arity::AtLeast(1),
        "sum of numbers; all integers give an integer, any float widens the result to a float",
    );
    registry.pure(
        "sub",
        sub,
        Arity::Exact(2),
        "the first number minus the second",
    );
    registry.pure("mul", mul, Arity::AtLeast(1), "product of numbers");
    registry.pure(
        "div",
        div,
        Arity::Exact(2),
        "the first number divided by the second; two integers divide to an integer",
    );
    registry.pure(
        "modulo",
        modulo,
        Arity::Exact(2),
        "remainder after dividing the first number by the second, signed like the first",
    );
    registry.pure(
        "neg",
        neg,
        Arity::Exact(1),
        "the number with its sign flipped",
    );
    registry.pure("abs", abs, Arity::Exact(1), "how far a number is from zero");
    registry.pure(
        "pow",
        pow,
        Arity::Exact(2),
        "the first number raised to the power of the second",
    );
    registry.pure("min", min, Arity::AtLeast(1), "the smallest of the numbers");
    registry.pure("max", max, Arity::AtLeast(1), "the largest of the numbers");

    // Comparison. Equality is about identity and takes anything; ordering is
    // numeric.
    registry.pure(
        "eq",
        eq,
        Arity::Exact(2),
        "true when the two arguments are the same concept",
    );
    registry.pure(
        "ne",
        ne,
        Arity::Exact(2),
        "true when the two arguments are different concepts",
    );
    registry.pure(
        "lt",
        lt,
        Arity::Exact(2),
        "true when the first number is less than the second",
    );
    registry.pure(
        "gt",
        gt,
        Arity::Exact(2),
        "true when the first number is greater than the second",
    );
    registry.pure(
        "lte",
        lte,
        Arity::Exact(2),
        "true when the first number is less than or equal to the second",
    );
    registry.pure(
        "gte",
        gte,
        Arity::Exact(2),
        "true when the first number is greater than or equal to the second",
    );

    // Logic. `and`, `or`, and `if` are lazy because their whole job is to not
    // evaluate something.
    registry.register(
        "and",
        and,
        Arity::AtLeast(1),
        ArgStrategy::Lazy,
        Effect::Pure,
        "true when every argument is true; stops at the first false",
    );
    registry.register(
        "or",
        or,
        Arity::AtLeast(1),
        ArgStrategy::Lazy,
        Effect::Pure,
        "true when any argument is true; stops at the first true",
    );
    registry.pure("not", not, Arity::Exact(1), "the opposite of a boolean");
    registry.register(
        "if",
        conditional,
        Arity::Exact(3),
        ArgStrategy::Lazy,
        Effect::Pure,
        "the second argument when the condition is true, otherwise the third; only the taken branch runs",
    );
    registry.pure(
        "xor",
        xor,
        Arity::Exact(2),
        "true when exactly one of the two booleans is true",
    );

    // Numeric predicates.
    registry.pure(
        "is-zero",
        is_zero,
        Arity::Exact(1),
        "true when the number is zero",
    );
    registry.pure(
        "is-positive",
        is_positive,
        Arity::Exact(1),
        "true when the number is greater than zero",
    );
    registry.pure(
        "is-negative",
        is_negative,
        Arity::Exact(1),
        "true when the number is less than zero",
    );
    registry.pure(
        "is-even",
        is_even,
        Arity::Exact(1),
        "true when the integer divides evenly by two",
    );
    registry.pure(
        "is-odd",
        is_odd,
        Arity::Exact(1),
        "true when the integer does not divide evenly by two",
    );
}
