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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Confidence {
    pub support_count: u32,
    pub contradiction_count: u32,
    pub scope: Vec<ScopeCondition>,
    pub sources: Vec<Source>,
    pub last_tested: Option<i64>,
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

/// A provenance-bearing observation used to support or contradict knowledge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub tier: VerifiabilityTier,
    pub source: Source,
    pub timestamp: i64,
    pub linked_episode: Option<EpisodeId>,
}

impl Evidence {
    pub fn new(tier: VerifiabilityTier, source: Source, timestamp: i64) -> Self {
        Self {
            tier,
            source,
            timestamp,
            linked_episode: None,
        }
    }

    pub fn linked_to(mut self, episode: EpisodeId) -> Self {
        self.linked_episode = Some(episode);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_roundtrip_preserves_provenance_and_episode_link() {
        let episode = EpisodeId::new();
        let evidence = Evidence::new(
            VerifiabilityTier::Hard,
            Source {
                kind: SourceKind::SelfVerified,
                id: "arithmetic-check".into(),
                reliability: 1.0,
            },
            123,
        )
        .linked_to(episode);

        let json = serde_json::to_string(&evidence).unwrap();
        let restored: Evidence = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.tier, VerifiabilityTier::Hard);
        assert_eq!(restored.source.id, "arithmetic-check");
        assert_eq!(restored.timestamp, 123);
        assert_eq!(restored.linked_episode, Some(episode));
    }
}
