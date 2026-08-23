use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::evidence::Confidence;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConceptId(pub Uuid);

impl ConceptId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ConceptId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ConceptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// How a piece of knowledge may change. Different kinds of knowledge
/// have genuinely different truth conditions. (section 8)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MutabilityClass {
    /// double(x) = x*2. Only by better formalization, never by observation.
    Definitional,
    /// Dogs have four legs. Scope refinement: exceptions narrow applicability.
    DefeasibleGeneral,
    /// This dog has three legs. Append only, timestamped, never overwritten.
    Particular,
    /// How to scale a recipe. Versioned, test-gated, replaceable.
    Procedural,
    /// Goals, priorities. Only by explicit authorization.
    Normative,
    /// The learning rules themselves. Only by deliberate design change.
    CoreMachinery,
}

/// Where a piece of knowledge is in its lifecycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lifecycle {
    #[default]
    Active,
    Validated,
    Provisional,
    Stale,
    UnderReview,
    Superseded,
    Retired,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Concept {
    pub id: ConceptId,
    pub name: String,
    pub description: Option<String>,
    pub mutability: MutabilityClass,
    pub confidence: Confidence,
    pub lifecycle: Lifecycle,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Concept {
    pub fn new(name: impl Into<String>, mutability: MutabilityClass) -> Self {
        let now = now_unix();
        Self {
            id: ConceptId::new(),
            name: name.into(),
            description: None,
            mutability,
            confidence: Confidence::default(),
            lifecycle: Lifecycle::Active,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
