use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::concept::{ConceptId, Lifecycle};
use crate::episode::EpisodeId;
use crate::evidence::ScopeCondition;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelationshipId(pub Uuid);

impl RelationshipId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RelationshipId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RelationshipId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub id: RelationshipId,
    pub source: ConceptId,
    pub target: ConceptId,
    /// The kind of relationship: "is-a", "has", "implemented-by",
    /// "tested-by", "inverse-of", "special-case-of", etc.
    pub kind: String,
    pub strength: f64,
    pub scope: Vec<ScopeCondition>,
    pub evidence: Vec<EpisodeId>,
    pub lifecycle: Lifecycle,
    pub created_at: i64,
}

impl Relationship {
    pub fn new(source: ConceptId, target: ConceptId, kind: impl Into<String>) -> Self {
        Self {
            id: RelationshipId::new(),
            source,
            target,
            kind: kind.into(),
            strength: 1.0,
            scope: Vec::new(),
            evidence: Vec::new(),
            lifecycle: Lifecycle::Active,
            created_at: now_unix(),
        }
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
