//! Control flow natives.

use spoon_concept::{Concept, Effect};
use spoon_eval::{
    ArgStrategy, Arity, Ctx, EvalError, EvalResult, NativeRegistry, native_error, type_error,
};

fn call(ctx: &mut dyn Ctx, f: &Concept, args: Vec<Concept>) -> EvalResult {
    if !spoon_concept::holes(f).is_empty() {
        let bound = spoon_concept::substitute_positional(f, &args);
        return ctx.eval(&bound);
    }
    ctx.eval(&Concept::apply(f.clone(), args))
}

fn want_count(native: &str, c: &Concept) -> Result<usize, EvalError> {
    let raw = c
        .as_ground()
        .and_then(|g| g.as_i64())
        .ok_or_else(|| type_error(native, "integer", c))?;
    usize::try_from(raw).map_err(|_| native_error(native, format!("negative count {raw}")))
}

/// ctrl-let<value, body> - bind value into body's ?0
fn ctrl_let(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let value = ctx.eval(&args[0])?;
    let body = &args[1];
    let substituted = spoon_concept::substitute_positional(body, &[value]);
    ctx.eval(&substituted)
}

/// ctrl-resolve<concept> - force-evaluate a concept
fn ctrl_resolve(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    ctx.eval(&args[0])
}

/// ctrl-match<value, pattern1, result1, pattern2, result2, ...>
/// Evaluates value, then checks each pattern. If pattern == value, evaluates
/// and returns the corresponding result. Falls through to error if no match.
fn ctrl_match(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let value = ctx.eval(&args[0])?;
    let pairs = &args[1..];
    if pairs.len() % 2 != 0 {
        return Err(native_error(
            "ctrl-match",
            "expected value followed by pattern-result pairs",
        ));
    }
    for chunk in pairs.chunks(2) {
        let pattern = ctx.eval(&chunk[0])?;
        if pattern == value {
            return ctx.eval(&chunk[1]);
        }
    }
    Err(native_error("ctrl-match", "no pattern matched"))
}

/// ctrl-try<body, fallback> - evaluate body, return fallback if body fails
fn ctrl_try(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    match ctx.eval(&args[0]) {
        Ok(v) => Ok(v),
        Err(_) => ctx.eval(&args[1]),
    }
}

/// ctrl-pipe<value, f1, f2, ...> - thread value through functions left to right
fn ctrl_pipe(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut value = ctx.eval(&args[0])?;
    for func in &args[1..] {
        value = call(ctx, func, vec![value])?;
    }
    Ok(value)
}

/// ctrl-compose<f, g> - return a concept that applies g then f: f(g(?0))
fn ctrl_compose(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let f = &args[0];
    let g = &args[1];
    let hole = Concept::hole(0);
    let inner = Concept::apply(g.clone(), vec![hole]);
    Ok(Concept::apply(f.clone(), vec![inner]))
}

/// ctrl-identity<x> - return x unchanged
fn ctrl_identity(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    ctx.eval(&args[0])
}

/// ctrl-constant<value, _ignored> - return value regardless of second arg
fn ctrl_constant(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    ctx.eval(&args[0])
}

/// ctrl-do-times<n, initial, func> - apply func n times starting from initial
fn ctrl_do_times(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let n = want_count("ctrl-do-times", &ctx.eval(&args[0])?)?;
    let mut acc = ctx.eval(&args[1])?;
    let func = &args[2];
    for _ in 0..n {
        acc = call(ctx, func, vec![acc])?;
    }
    Ok(acc)
}

/// ctrl-apply<func, args_list> - apply a function to a list of arguments
fn ctrl_apply(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let func = &args[0];
    let arg_list = ctx.eval(&args[1])?;
    let a = arg_list.args();
    let items: Vec<Concept> = if a.is_empty() {
        vec![arg_list]
    } else {
        a.to_vec()
    };
    call(ctx, func, items)
}

pub fn register(registry: &mut NativeRegistry) {
    registry.register(
        "ctrl-let",
        ctrl_let,
        Arity::Exact(2),
        ArgStrategy::Lazy,
        Effect::Pure,
        "bind a value into a body expression, replacing ?0",
    );
    registry.register(
        "ctrl-resolve",
        ctrl_resolve,
        Arity::Exact(1),
        ArgStrategy::Lazy,
        Effect::Pure,
        "force-evaluate a concept to its result",
    );
    registry.register(
        "ctrl-match",
        ctrl_match,
        Arity::AtLeast(3),
        ArgStrategy::Lazy,
        Effect::Pure,
        "match a value against pattern-result pairs, returning the first match",
    );
    registry.register(
        "ctrl-try",
        ctrl_try,
        Arity::Exact(2),
        ArgStrategy::Lazy,
        Effect::Pure,
        "evaluate the first argument, fall back to the second if it fails",
    );
    registry.register(
        "ctrl-pipe",
        ctrl_pipe,
        Arity::AtLeast(2),
        ArgStrategy::Lazy,
        Effect::Pure,
        "thread a value through a chain of functions left to right",
    );
    registry.register(
        "ctrl-compose",
        ctrl_compose,
        Arity::Exact(2),
        ArgStrategy::Lazy,
        Effect::Pure,
        "compose two functions: ctrl-compose<f, g> gives f(g(?0))",
    );
    registry.register(
        "ctrl-identity",
        ctrl_identity,
        Arity::Exact(1),
        ArgStrategy::Lazy,
        Effect::Pure,
        "return the argument unchanged",
    );
    registry.register(
        "ctrl-constant",
        ctrl_constant,
        Arity::Exact(2),
        ArgStrategy::Lazy,
        Effect::Pure,
        "return the first argument, ignoring the second",
    );
    registry.register(
        "ctrl-do-times",
        ctrl_do_times,
        Arity::Exact(3),
        ArgStrategy::Selective(0b001),
        Effect::Pure,
        "apply a function n times to an accumulator: ctrl-do-times<n, initial, func>",
    );
    registry.register(
        "ctrl-apply",
        ctrl_apply,
        Arity::Exact(2),
        ArgStrategy::Selective(0b01),
        Effect::Pure,
        "apply a function to a list of arguments",
    );
}
