use serde::{Deserialize, Serialize};

use crate::procedure::ProcedureId;
use crate::value::Value;

/// The neutral, decomposable procedure representation.
///
/// Not "TypeScript" or "Python" - an internal representation
/// reducible to its parts. A learned skill survives a change
/// of runtime because the skill is knowledge and the runtime
/// is an environment detail. (section 6)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Literal(Value),
    Var(String),
    BinOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    UnOp {
        op: UnOp,
        operand: Box<Expr>,
    },
    Call {
        procedure: ProcedureId,
        args: Vec<Expr>,
    },
    /// Invoke one immutable revision of an already known pure procedure.
    ///
    /// Unlike `Call`, this carries its required revision in the durable
    /// expression so a learned composition cannot silently change behavior
    /// when the dependency receives a later revision.
    CallExact {
        procedure: ProcedureId,
        version: u32,
        args: Vec<Expr>,
    },
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    Let {
        name: String,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    Block(Vec<Expr>),
    ListExpr(Vec<Expr>),
    Index {
        collection: Box<Expr>,
        index: Box<Expr>,
    },
    FieldAccess {
        object: Box<Expr>,
        field: String,
    },
    /// Iterate over a collection, applying a transform to each element.
    Map {
        collection: Box<Expr>,
        var: String,
        body: Box<Expr>,
    },
    /// Filter a collection by a predicate.
    Filter {
        collection: Box<Expr>,
        var: String,
        predicate: Box<Expr>,
    },
    /// Fold a collection into a single value.
    Reduce {
        collection: Box<Expr>,
        init: Box<Expr>,
        acc: String,
        var: String,
        body: Box<Expr>,
    },
    /// Invoke an authority-free operation from a versioned intrinsic
    /// vocabulary. Intrinsics are deterministic runtime machinery; learned
    /// procedures compose them but cannot redefine their semantics.
    Intrinsic {
        version: u16,
        op: IntrinsicOp,
        args: Vec<Expr>,
    },
}

/// Version 1 of Spoon's pure, portable standard-library operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntrinsicOp {
    Length,
    TextByteLength,
    TextScalarLength,
    TextGraphemeLength,
    TextSplit,
    TextJoin,
    TextTrim,
    TextLowercase,
    TextUppercase,
    TextContains,
    TextStartsWith,
    TextEndsWith,
    TextReplace,
    CollectionContains,
    CountEqual,
    MapKeys,
    MapValues,
    JsonParse,
    JsonStringify,
    PathGet,
    PathGetOptional,
    TextNormalizeNfc,
    TextNormalizeNfd,
    TextNormalizeNfkc,
    TextNormalizeNfkd,
    TextTrimStart,
    TextTrimEnd,
    TextGraphemeSubstring,
    TextIndexOf,
    TextCount,
    TextRepeat,
    TextConcatMany,
    MapEntries,
    MapSet,
    MapDelete,
    MapMerge,
    CollectionSlice,
    CollectionReverse,
    CollectionSort,
    CollectionUnique,
    CollectionFlatten,
    CollectionZip,
    Range,
    TypeName,
    ParseInt,
    ParseFloat,
    ParseBool,
    ToText,
    NumericAbs,
    NumericSign,
    NumericMin,
    NumericMax,
    NumericClamp,
    NumericFloor,
    NumericCeil,
    NumericRound,
    NumericTruncate,
    NumericPowInt,
    NumericPowFloat,
    IntegerQuotient,
    IntegerRemainder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Not,
}

impl std::fmt::Display for BinOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BinOp::Add => write!(f, "+"),
            BinOp::Sub => write!(f, "-"),
            BinOp::Mul => write!(f, "*"),
            BinOp::Div => write!(f, "/"),
            BinOp::Mod => write!(f, "%"),
            BinOp::Eq => write!(f, "=="),
            BinOp::Ne => write!(f, "!="),
            BinOp::Lt => write!(f, "<"),
            BinOp::Le => write!(f, "<="),
            BinOp::Gt => write!(f, ">"),
            BinOp::Ge => write!(f, ">="),
            BinOp::And => write!(f, "&&"),
            BinOp::Or => write!(f, "||"),
        }
    }
}

impl std::fmt::Display for UnOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnOp::Neg => write!(f, "-"),
            UnOp::Not => write!(f, "!"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intrinsic_expression_survives_json_round_trip() {
        let expression = Expr::Intrinsic {
            version: 1,
            op: IntrinsicOp::PathGet,
            args: vec![
                Expr::Intrinsic {
                    version: 1,
                    op: IntrinsicOp::JsonParse,
                    args: vec![Expr::Literal(Value::Text(
                        r#"{"items":[{"id":7}]}"#.to_string(),
                    ))],
                },
                Expr::Literal(Value::Text("items[0].id".to_string())),
            ],
        };

        let encoded = serde_json::to_string(&expression).unwrap();
        let decoded: Expr = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, expression);
    }
}
