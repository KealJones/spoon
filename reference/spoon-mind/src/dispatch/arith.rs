//! Arithmetic: compile ArithExpr -> IR Expr, then evaluate via the kernel.
//! No LLM involved - pure Rust evaluation of typed IR.

use spoon_core::kernel::eval::eval_program;
use spoon_core::kernel::{Ctx, EvalError};
use spoon_core::types::{ArithExpr, ArithOp, Expr, Program, Type, Value};

use super::DispatchCtx;

/// Compile an ArithExpr to an IR Expr (no evaluation yet).
/// Variables become Param{index:0} as a best-effort placeholder;
/// in practice arithmetic commands supply only literals.
pub fn compile(e: &ArithExpr) -> Expr {
    match e {
        ArithExpr::Num { value } => Expr::Const { value: Value::Float(*value) },
        ArithExpr::Ref { .. } => Expr::Param { index: 0 },
        ArithExpr::Neg { of } => Expr::Call { action: "math.neg".into(), args: vec![compile(of)] },
        ArithExpr::Bin { op, lhs, rhs } => {
            let action_id = match op {
                ArithOp::Add => "math.add",
                ArithOp::Sub => "math.sub",
                ArithOp::Mul => "math.mul",
                ArithOp::Div => "math.div",
                ArithOp::Pow => "math.pow",
                ArithOp::Mod => "math.mod",
            };
            Expr::Call {
                action: action_id.into(),
                args: vec![compile(lhs), compile(rhs)],
            }
        }
    }
}

/// Compile and evaluate an ArithExpr using the kernel.
pub fn eval(ctx: &mut DispatchCtx<'_>, e: &ArithExpr) -> Result<Value, EvalError> {
    let expr = compile(e);
    let program = Program { params: vec![], ret: Type::Float, body: expr };
    let mut kctx = Ctx::new(ctx.can, ctx.kernel, ctx.host);
    eval_program(&mut kctx, &program, &[])
}
