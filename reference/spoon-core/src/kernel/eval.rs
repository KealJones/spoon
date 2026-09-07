//! IR evaluator. Implements the three public entry points and the private
//! recursive expression evaluator.
//!
//! Recursion depth is tracked as an explicit parameter so that mutual
//! recursion between Program-impl actions is also bounded.

use std::collections::BTreeMap;

use super::{Ctx, EvalError};
use crate::types::*;

const MAX_DEPTH: usize = 64;

// ---- public API --------------------------------------------------------

pub fn eval_program(ctx: &mut Ctx<'_>, program: &Program, args: &[Value]) -> Result<Value, EvalError> {
    if args.len() != program.params.len() {
        return Err(EvalError::Arity {
            action: ActionId("program".into()),
            expected: program.params.len(),
            got: args.len(),
        });
    }
    type_check_args(ctx, &program.params, args, "program")?;
    eval_expr(ctx, &program.body, args, None, 0)
}

pub fn call_action(ctx: &mut Ctx<'_>, id: &ActionId, args: &[Value]) -> Result<Value, EvalError> {
    call_action_at(ctx, id, args, 0)
}

pub fn apply_lambda(ctx: &mut Ctx<'_>, lambda: &Lambda, args: &[Value]) -> Result<Value, EvalError> {
    if args.len() != lambda.params.len() {
        return Err(EvalError::Arity {
            action: ActionId("lambda".into()),
            expected: lambda.params.len(),
            got: args.len(),
        });
    }
    // Lambdas do not see outer program params; params slice is empty here.
    eval_expr(ctx, &lambda.body, &[], Some(args), 0)
}

// ---- internal helpers --------------------------------------------------

fn type_check_args(ctx: &mut Ctx<'_>, param_types: &[Type], args: &[Value], loc: &str) -> Result<(), EvalError> {
    for (i, (param_ty, arg)) in param_types.iter().zip(args).enumerate() {
        let arg_ty = arg.type_of();
        // An empty list is assignable to any List type or Any.
        let ok = match arg {
            Value::List(items) if items.is_empty() => {
                matches!(param_ty, Type::List(_) | Type::Any)
                    || ctx.can.assignable(param_ty, &Type::list(Type::Any))
            }
            _ => ctx.can.assignable(param_ty, &arg_ty),
        };
        if !ok {
            return Err(EvalError::Type {
                expected: param_ty.to_string(),
                got: arg_ty.to_string(),
                at: format!("{loc} param {i}"),
            });
        }
    }
    Ok(())
}

fn call_action_at(ctx: &mut Ctx<'_>, id: &ActionId, args: &[Value], depth: usize) -> Result<Value, EvalError> {
    // Kernel primitive takes priority over CAN lookup.
    if ctx.kernel.has(id) {
        // Check arity against the registered action definition.
        if let Some(def) = ctx.kernel.actions().iter().find(|a| &a.id == id) {
            check_arity(id, &def.inputs, args)?;
        }
        return ctx.kernel.call(ctx, id, args);
    }
    let action = ctx.can.action(id).ok_or_else(|| EvalError::UnknownAction(id.clone()))?.clone();
    check_arity(id, &action.inputs, args)?;
    match action.imp {
        Impl::Primitive => ctx.kernel.call(ctx, id, args),
        Impl::Program { program } => {
            if action.effect > ctx.max_effect {
                return Err(EvalError::Permission { action: id.clone(), effect: action.effect });
            }
            eval_program_at(ctx, &program, args, depth)
        }
    }
}

fn check_arity(id: &ActionId, inputs: &[Input], args: &[Value]) -> Result<(), EvalError> {
    let required = inputs.iter().filter(|i| i.required).count();
    let max = inputs.len();
    if args.len() < required || args.len() > max {
        return Err(EvalError::Arity {
            action: id.clone(),
            expected: max,
            got: args.len(),
        });
    }
    Ok(())
}

fn eval_program_at(ctx: &mut Ctx<'_>, program: &Program, args: &[Value], depth: usize) -> Result<Value, EvalError> {
    if args.len() != program.params.len() {
        return Err(EvalError::Arity {
            action: ActionId("program".into()),
            expected: program.params.len(),
            got: args.len(),
        });
    }
    type_check_args(ctx, &program.params, args, "program")?;
    eval_expr(ctx, &program.body, args, None, depth)
}

fn eval_expr(
    ctx: &mut Ctx<'_>,
    expr: &Expr,
    params: &[Value],
    lambda_args: Option<&[Value]>,
    depth: usize,
) -> Result<Value, EvalError> {
    ctx.budget.tick()?;
    if depth > MAX_DEPTH {
        return Err(EvalError::Other(format!(
            "recursion depth limit ({MAX_DEPTH}) exceeded"
        )));
    }
    match expr {
        Expr::Const { value } => Ok(value.clone()),

        Expr::Param { index } => params
            .get(*index)
            .cloned()
            .ok_or_else(|| EvalError::Other(format!("param index {index} out of range"))),

        Expr::LambdaParam { index } => {
            let la = lambda_args.ok_or_else(|| {
                EvalError::Other("LambdaParam used outside lambda context".into())
            })?;
            la.get(*index)
                .cloned()
                .ok_or_else(|| EvalError::Other(format!("lambda param index {index} out of range")))
        }

        Expr::Call { action, args } => {
            let mut evaled = Vec::with_capacity(args.len());
            for a in args {
                evaled.push(eval_expr(ctx, a, params, lambda_args, depth + 1)?);
            }
            call_action_at(ctx, action, &evaled, depth + 1)
        }

        Expr::If { cond, then, otherwise } => {
            let c = eval_expr(ctx, cond, params, lambda_args, depth + 1)?;
            match c {
                Value::Bool(true) => eval_expr(ctx, then, params, lambda_args, depth + 1),
                Value::Bool(false) => eval_expr(ctx, otherwise, params, lambda_args, depth + 1),
                other => Err(EvalError::ty("bool", &other, "if condition")),
            }
        }

        Expr::Lambda { lambda } => Ok(Value::Lambda(lambda.clone())),

        Expr::Struct { concept, fields } => {
            let mut map = BTreeMap::new();
            for (name, e) in fields {
                map.insert(name.clone(), eval_expr(ctx, e, params, lambda_args, depth + 1)?);
            }
            Ok(Value::Struct { concept: concept.clone(), fields: map })
        }

        Expr::Field { of, name } => {
            let val = eval_expr(ctx, of, params, lambda_args, depth + 1)?;
            match val {
                Value::Struct { ref fields, .. } => fields
                    .get(name)
                    .cloned()
                    .ok_or_else(|| EvalError::Other(format!("field '{name}' not found in struct"))),
                Value::Json(j) => {
                    Ok(Value::Json(j.get(name.as_str()).cloned().unwrap_or(serde_json::Value::Null)))
                }
                other => Err(EvalError::ty("struct or json", &other, format!("field .{name}"))),
            }
        }

        Expr::ListLit { items } => {
            let mut vals = Vec::with_capacity(items.len());
            for e in items {
                vals.push(eval_expr(ctx, e, params, lambda_args, depth + 1)?);
            }
            Ok(Value::List(vals))
        }
    }
}
