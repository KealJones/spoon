//! `ekg-exec`: the expression evaluator for the Executable Knowledge Graph.
//!
//! This crate executes procedures represented as `ekg_core::Expr` trees and
//! captures a trace of the procedure calls made along the way, so that
//! credit assignment can later replay exactly what happened during a run.

pub mod error;
pub mod eval;
pub mod trace;

pub use error::EkgError;
pub use eval::{Env, ExecResult, ExecutionBudget, Evaluator};
pub use trace::{ExecStep, ExecTrace};
