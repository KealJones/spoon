//! Evaluation: term rewriting over concepts.
//!
//! A compound concept is reduced by finding a realization for its head,
//! applying it, and reducing the result. "Lowering" is that loop run until it
//! reaches natives. "Inference" is that loop where the realizations happen to
//! be rewrite rules. "Planning" is that loop where they happen to be search
//! procedures. There is no second engine anywhere.
//!
//! The contract this implements is `docs/EVALUATION.md`.

mod budget;
mod ctx;
mod error;
mod eval;
mod native;
mod select;
mod trace;

pub use budget::{Budget, BudgetState};
pub use ctx::Ctx;
pub use error::{EvalError, EvalResult, Gap, GapReason, Limit, Outcome};
pub use eval::{Evaluator, ExternalRunner, NeuralSeat, PermissionMode};
pub use native::{
    ArgStrategy, Arity, NativeEntry, NativeFn, NativeRegistry, native_error, type_error,
};
pub use select::{FitKind, Rng, Scored, context_fit, rank, score};
pub use trace::{Note, Step, StepOutcome, Trace};
