use serde::{Deserialize, Serialize};

use crate::episode::EpisodeId;

/// How strongly a result can be verified. Determines learning rate.
/// A Tier 1 failure may justify immediately rejecting a procedure.
/// A Tier 3 complaint should not, because the complaint may be wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifiabilityTier {
    /// Deterministic check. Tests, arithmetic, types, invariants.
    Hard,
    /// Independent methods agree, inverse recovers input, cross-check.
    Consensus,
    /// Human judgment, deferred outcome, weak signal.
    Deferred,
}

/// Belief carried as several separate things, not a single number.
/// A scalar cannot separate "barely examined" from "extensively tested"
/// or "works everywhere" from "works in 80% of contexts." (section 9)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Confidence {
    pub support_count: u32,
    pub contradiction_count: u32,
    pub scope: Vec<ScopeCondition>,
    pub sources: Vec<Source>,
    pub last_tested: Option<i64>,
}

impl Default for Confidence {
    fn default() -> Self {
        Self {
            support_count: 0,
            contradiction_count: 0,
            scope: Vec::new(),
            sources: Vec::new(),
            last_tested: None,
        }
    }
}

impl Confidence {
    /// Derived scalar summary. Useful for ranking and display, but
    /// should never be the only thing kept. (section 9)
    pub fn scalar(&self) -> f64 {
        let total = self.support_count + self.contradiction_count;
        if total == 0 {
            return 0.5;
        }
        self.support_count as f64 / total as f64
    }
}

/// A condition under which a claim holds. Refined by contradiction
/// and by failure. This is where defeasibility lives. (section 7)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeCondition {
    pub description: String,
    pub learned_from: Option<EpisodeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    pub id: String,
    pub reliability: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    Taught,
    SelfVerified,
    Inferred,
    Observed,
}
