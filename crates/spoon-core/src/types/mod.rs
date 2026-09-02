//! Shared vocabulary for the whole system. Depends on nothing else in Spoon.

pub mod can;
pub mod clause;
pub mod episode;
pub mod intent;
pub mod ir;
pub mod response;
pub mod value;

pub use can::{
    Action, ActionId, Cardinality, Concept, ConceptId, ConceptKind, Effect, Fact, Impl, Input,
    Property, Provenance, Role, Stats, Tier,
};
pub use clause::{
    Act, ArithExpr, ArithOp, Clause, EarsPath, EarsResult, Modal, Pred, Quant, QuestionKind,
    Referent, Term,
};
pub use episode::{Episode, Pair, TurnMetrics};
pub use intent::{Goal, Intent, Plan, PlanNode, PlanOutcome, Signal};
pub use ir::{Expr, Lambda, Program};
pub use response::{AdviceOption, Move, ResponsePlan, Tone};
pub use value::{Type, Value};

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
