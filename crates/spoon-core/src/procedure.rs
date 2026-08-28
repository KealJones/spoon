use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::concept::{ConceptId, Lifecycle};
use crate::contract::Contract;
use crate::episode::EpisodeId;
use crate::evidence::VerifiabilityTier;
use crate::expr::Expr;
use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcedureId(pub Uuid);

impl ProcedureId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProcedureId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ProcedureId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Procedure {
    pub id: ProcedureId,
    pub name: String,
    pub params: Vec<Param>,
    pub body: Expr,
    pub contract: Contract,
    /// Self-growing regression suite: every episode with a verified
    /// answer becomes a permanent test. (section 27)
    pub test_cases: Vec<TestCase>,
    /// The concept this procedure implements (e.g., DOUBLE -> MULTIPLY(x, 2))
    pub concept: Option<ConceptId>,
    pub version: u32,
    pub lifecycle: Lifecycle,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A durable reference to one imported capability procedure used by a
/// procedure body. The reference is data only; the runtime re-resolves and
/// re-authorizes it at execution time.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDependency {
    pub content_id: String,
    pub procedure_id: String,
}

impl Procedure {
    pub fn new(name: impl Into<String>, params: Vec<Param>, body: Expr) -> Self {
        let now = now_unix();
        Self {
            id: ProcedureId::new(),
            name: name.into(),
            params,
            body,
            contract: Contract::default(),
            test_cases: Vec::new(),
            concept: None,
            version: 1,
            lifecycle: Lifecycle::Active,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_contract(mut self, contract: Contract) -> Self {
        self.contract = contract;
        self
    }

    pub fn with_concept(mut self, concept: ConceptId) -> Self {
        self.concept = Some(concept);
        self
    }

    /// Return the capability procedures referenced by this body. Pure
    /// procedures have an empty list; effectful procedures are still the same
    /// durable Procedure type, but expose their runtime dependencies.
    pub fn capability_dependencies(&self) -> Vec<CapabilityDependency> {
        let mut dependencies = Vec::new();
        self.body.collect_capability_dependencies(&mut dependencies);
        dependencies.sort_by(|left, right| {
            left.content_id
                .cmp(&right.content_id)
                .then(left.procedure_id.cmp(&right.procedure_id))
        });
        dependencies.dedup();
        dependencies
    }

    pub fn is_effectful(&self) -> bool {
        !self.capability_dependencies().is_empty()
    }
}

impl Expr {
    fn collect_capability_dependencies(&self, output: &mut Vec<CapabilityDependency>) {
        match self {
            Self::CapabilityCall {
                content_id,
                procedure_id,
                input,
            } => {
                output.push(CapabilityDependency {
                    content_id: content_id.clone(),
                    procedure_id: procedure_id.clone(),
                });
                input.collect_capability_dependencies(output);
            }
            Self::BinOp { left, right, .. } => {
                left.collect_capability_dependencies(output);
                right.collect_capability_dependencies(output);
            }
            Self::UnOp { operand, .. } => operand.collect_capability_dependencies(output),
            Self::Call { args, .. }
            | Self::CallExact { args, .. }
            | Self::ListExpr(args)
            | Self::Block(args) => {
                for arg in args {
                    arg.collect_capability_dependencies(output);
                }
            }
            Self::If { cond, then, else_ } => {
                cond.collect_capability_dependencies(output);
                then.collect_capability_dependencies(output);
                else_.collect_capability_dependencies(output);
            }
            Self::Let { value, body, .. } => {
                value.collect_capability_dependencies(output);
                body.collect_capability_dependencies(output);
            }
            Self::Index { collection, index } => {
                collection.collect_capability_dependencies(output);
                index.collect_capability_dependencies(output);
            }
            Self::FieldAccess { object, .. } => object.collect_capability_dependencies(output),
            Self::Map {
                collection, body, ..
            }
            | Self::Filter {
                collection,
                predicate: body,
                ..
            } => {
                collection.collect_capability_dependencies(output);
                body.collect_capability_dependencies(output);
            }
            Self::Reduce {
                collection,
                init,
                body,
                ..
            } => {
                collection.collect_capability_dependencies(output);
                init.collect_capability_dependencies(output);
                body.collect_capability_dependencies(output);
            }
            Self::Intrinsic { args, .. } => {
                for arg in args {
                    arg.collect_capability_dependencies(output);
                }
            }
            Self::Literal(_) | Self::Var(_) => {}
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub description: Option<String>,
    /// The value category accepted by this parameter. `None` is retained for
    /// legacy procedures authored before parameter types were part of the IR.
    #[serde(default)]
    pub value_type: Option<ParamType>,
}

/// Stable input categories used at the procedure boundary. These are
/// deliberately coarser than the evaluator's internal expression types: the
/// interpreter needs to know whether a slot is numeric, textual, structured,
/// etc., without pretending to encode a full static type system in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    Any,
    Null,
    Bool,
    Number,
    Text,
    List,
    Map,
}

impl ParamType {
    pub fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Any => true,
            Self::Null => matches!(value, Value::Null),
            Self::Bool => matches!(value, Value::Bool(_)),
            Self::Number => value.is_numeric(),
            Self::Text => matches!(value, Value::Text(_)),
            Self::List => matches!(value, Value::List(_)),
            Self::Map => matches!(value, Value::Map(_)),
        }
    }
}

impl Param {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            // Programmatic callers are explicitly opting into a slot, even
            // when they do not know a narrower category yet. Persisted JSON
            // from before this field existed still deserializes as `None` and
            // remains ineligible for language interpretation.
            value_type: Some(ParamType::Any),
        }
    }

    pub fn typed(name: impl Into<String>, value_type: ParamType) -> Self {
        Self {
            name: name.into(),
            description: None,
            value_type: Some(value_type),
        }
    }
}

/// A test case, potentially from a verified episode.
/// Tier 1 verified -> hard regression test.
/// Tier 2 verified -> consistency test.
/// Tier 3 only -> not a test, too noisy to gate on. (section 27)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestCase {
    pub inputs: Vec<(String, Value)>,
    pub expected_output: Value,
    pub from_episode: Option<EpisodeId>,
    pub tier: VerifiabilityTier,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
