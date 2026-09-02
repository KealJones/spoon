//! IR evaluator. TO BE IMPLEMENTED by the kernel subagent.
//!
//! Contract:
//! - `eval_program(ctx, program, args)`: type-check arity, bind params, eval body.
//! - `call_action(ctx, id, args)`: dispatch to the kernel primitive or, for
//!   `Impl::Program` actions in `ctx.can`, recursively `eval_program`. Records
//!   nothing (the executor does stats).
//! - `eval_expr` handles every `Expr` variant. `LambdaParam` resolves against
//!   the innermost lambda frame passed down as `lambda_args`.
//! - Every node evaluation calls `ctx.budget.tick()`.
//! - Higher-order primitives receive `Value::Lambda` and call
//!   `apply_lambda(ctx, &lambda, &[args])`.

use super::{Ctx, EvalError};
use crate::types::*;

pub fn eval_program(ctx: &mut Ctx<'_>, program: &Program, args: &[Value]) -> Result<Value, EvalError> {
    let _ = (ctx, program, args);
    unimplemented!("kernel subagent")
}

pub fn call_action(ctx: &mut Ctx<'_>, id: &ActionId, args: &[Value]) -> Result<Value, EvalError> {
    let _ = (ctx, id, args);
    unimplemented!("kernel subagent")
}

pub fn apply_lambda(ctx: &mut Ctx<'_>, lambda: &Lambda, args: &[Value]) -> Result<Value, EvalError> {
    let _ = (ctx, lambda, args);
    unimplemented!("kernel subagent")
}
