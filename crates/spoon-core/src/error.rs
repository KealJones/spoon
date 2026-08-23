use thiserror::Error;

use crate::value::Value;

#[derive(Debug, Error)]
pub enum SpoonError {
    #[error("type error: expected {expected}, got {got}")]
    TypeError { expected: String, got: String },

    #[error("undefined variable: {0}")]
    UndefinedVar(String),

    #[error("undefined procedure: {0}")]
    UndefinedProcedure(String),

    #[error("arity mismatch: {name} expects {expected} args, got {got}")]
    ArityMismatch {
        name: String,
        expected: usize,
        got: usize,
    },

    #[error("contract violation: {0}")]
    ContractViolation(String),

    #[error("division by zero")]
    DivisionByZero,

    #[error("arithmetic overflow during {operation}")]
    ArithmeticOverflow { operation: String },

    #[error("index out of bounds: {index} in collection of length {length}")]
    IndexOutOfBounds { index: i64, length: usize },

    #[error("field not found: {0}")]
    FieldNotFound(String),

    #[error("execution budget exceeded")]
    BudgetExceeded,

    #[error("execution timed out")]
    Timeout,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("{0}")]
    Other(String),
}

impl SpoonError {
    pub fn type_error(expected: &str, got: &Value) -> Self {
        Self::TypeError {
            expected: expected.to_string(),
            got: got.type_name().to_string(),
        }
    }
}
