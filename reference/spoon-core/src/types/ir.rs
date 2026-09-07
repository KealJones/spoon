//! Spoon's typed intermediate representation. Every non-primitive capability
//! is a `Program`. Programs are small, first-order except for lambdas passed
//! to higher-order primitives (map/filter/fold/sort_by).
//!
//! Identity of a program is the blake3 hash of its canonical JSON body, not
//! its name. Names are phrasings that live on the `Action`.

use serde::{Deserialize, Serialize};

use super::can::ActionId;
use super::value::{Type, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "snake_case")]
pub enum Expr {
    Const { value: Value },
    /// Top-level program parameter by index.
    Param { index: usize },
    /// Parameter of the innermost enclosing lambda by index. Lambdas do not
    /// nest (the synthesizer never needs it and it keeps evaluation trivial).
    LambdaParam { index: usize },
    Call { action: ActionId, args: Vec<Expr> },
    If { cond: Box<Expr>, then: Box<Expr>, otherwise: Box<Expr> },
    Lambda { lambda: Box<Lambda> },
    /// Build a struct value of a CAN concept.
    Struct { concept: super::can::ConceptId, fields: Vec<(String, Expr)> },
    /// Read a property from a struct value.
    Field { of: Box<Expr>, name: String },
    ListLit { items: Vec<Expr> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lambda {
    pub params: Vec<Type>,
    pub ret: Type,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub params: Vec<Type>,
    pub ret: Type,
    pub body: Expr,
}

impl Expr {
    pub fn call(action: impl Into<ActionId>, args: Vec<Expr>) -> Expr {
        Expr::Call { action: action.into(), args }
    }
    pub fn constant(value: impl Into<Value>) -> Expr {
        Expr::Const { value: value.into() }
    }
    pub fn param(index: usize) -> Expr {
        Expr::Param { index }
    }

    /// Number of nodes. Used as the cost in synthesis and consolidation.
    pub fn size(&self) -> usize {
        1 + match self {
            Expr::Const { .. } | Expr::Param { .. } | Expr::LambdaParam { .. } => 0,
            Expr::Call { args, .. } => args.iter().map(Expr::size).sum(),
            Expr::If { cond, then, otherwise } => cond.size() + then.size() + otherwise.size(),
            Expr::Lambda { lambda } => lambda.body.size(),
            Expr::Struct { fields, .. } => fields.iter().map(|(_, e)| e.size()).sum(),
            Expr::Field { of, .. } => of.size(),
            Expr::ListLit { items } => items.iter().map(Expr::size).sum(),
        }
    }

    /// Every action referenced anywhere in the expression.
    pub fn actions(&self, out: &mut Vec<ActionId>) {
        match self {
            Expr::Call { action, args } => {
                out.push(action.clone());
                args.iter().for_each(|a| a.actions(out));
            }
            Expr::If { cond, then, otherwise } => {
                cond.actions(out);
                then.actions(out);
                otherwise.actions(out);
            }
            Expr::Lambda { lambda } => lambda.body.actions(out),
            Expr::Struct { fields, .. } => fields.iter().for_each(|(_, e)| e.actions(out)),
            Expr::Field { of, .. } => of.actions(out),
            Expr::ListLit { items } => items.iter().for_each(|e| e.actions(out)),
            _ => {}
        }
    }
}

impl Program {
    pub fn new(params: Vec<Type>, ret: Type, body: Expr) -> Program {
        Program { params, ret, body }
    }
    pub fn size(&self) -> usize {
        self.body.size()
    }
    /// Content hash. Stable across processes.
    pub fn hash(&self) -> String {
        let json = serde_json::to_vec(self).expect("program serializes");
        blake3::hash(&json).to_hex()[..16].to_string()
    }
    pub fn actions(&self) -> Vec<ActionId> {
        let mut out = Vec::new();
        self.body.actions(&mut out);
        out.sort();
        out.dedup();
        out
    }
}
