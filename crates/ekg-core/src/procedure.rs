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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub description: Option<String>,
}

impl Param {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
        }
    }
}

/// A test case, potentially from a verified episode.
/// Tier 1 verified -> hard regression test.
/// Tier 2 verified -> consistency test.
/// Tier 3 only -> not a test, too noisy to gate on. (section 27)
#[derive(Debug, Clone, Serialize, Deserialize)]
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
