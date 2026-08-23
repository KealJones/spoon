use serde::{Deserialize, Serialize};

use crate::evidence::Confidence;
use crate::expr::Expr;

/// A procedure's declaration of the conditions under which it applies
/// and what it promises. Without a contract, a procedure is a landmine
/// in a library. (section 7)
///
/// Contracts do four jobs:
/// 1. Make composition typed (search over things that fit together)
/// 2. Make credit local (which contract was violated?)
/// 3. Make cost visible (how much does each move cost?)
/// 4. Make scope explicit (where defeasibility lives)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Contract {
    pub requires: Vec<Condition>,
    pub promises: Vec<Condition>,
    pub fails_when: Vec<Condition>,
    pub costs: CostEstimate,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    pub description: String,
    /// An executable check, if available. Makes contract verification
    /// automatic rather than advisory.
    pub check: Option<Expr>,
}

impl Condition {
    pub fn described(desc: impl Into<String>) -> Self {
        Self {
            description: desc.into(),
            check: None,
        }
    }

    pub fn with_check(mut self, check: Expr) -> Self {
        self.check = Some(check);
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostEstimate {
    pub operations: u32,
    pub description: String,
}
