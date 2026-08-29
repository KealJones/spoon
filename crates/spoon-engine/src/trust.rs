use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spoon_core::{Episode, ObservedFact, VerifiabilityTier};
use spoon_episode::EpisodeFeedback;

use crate::EngineError;

/// Durable proof that the Engine, rather than a caller-controlled store write,
/// evaluated this exact immutable evidence item at a strong tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustReceipt {
    pub evidence_kind: TrustEvidenceKind,
    pub evidence_id: String,
    pub evidence_digest: String,
    pub tier: VerifiabilityTier,
    pub issuer: String,
    pub issued_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustEvidenceKind {
    Episode,
    Feedback,
    Fact,
}

pub(crate) struct TrustLedger {
    conn: Connection,
}

impl TrustLedger {
    pub(crate) fn open(path: &str) -> Result<Self, EngineError> {
        let ledger = Self {
            conn: Connection::open(path)?,
        };
        ledger.create_schema()?;
        Ok(ledger)
    }

    pub(crate) fn in_memory() -> Result<Self, EngineError> {
        let ledger = Self {
            conn: Connection::open_in_memory()?,
        };
        ledger.create_schema()?;
        Ok(ledger)
    }

    fn create_schema(&self) -> Result<(), EngineError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS engine_trust_receipts (
                evidence_kind TEXT NOT NULL,
                evidence_id TEXT NOT NULL,
                evidence_digest TEXT NOT NULL,
                tier TEXT NOT NULL,
                issuer TEXT NOT NULL,
                issued_at INTEGER NOT NULL,
                PRIMARY KEY (evidence_kind, evidence_id),
                UNIQUE (evidence_kind, evidence_digest)
             );",
        )?;
        Ok(())
    }

    pub(crate) fn mint_engine_episode(&self, episode: &Episode) -> Result<(), EngineError> {
        let Some(evaluation) = episode.evaluation.as_ref() else {
            return Ok(());
        };
        if !is_strong(evaluation.tier) {
            return Ok(());
        }
        self.insert(TrustReceipt {
            evidence_kind: TrustEvidenceKind::Episode,
            evidence_id: episode.id.to_string(),
            evidence_digest: episode_digest(episode)?,
            tier: evaluation.tier,
            issuer: "engine:deterministic-evaluation".into(),
            issued_at: episode.created_at,
        })
    }

    pub(crate) fn mint_authenticated_episode(
        &self,
        episode: &Episode,
        verifier_identity: &str,
    ) -> Result<(), EngineError> {
        if verifier_identity.trim().is_empty() {
            return Err(EngineError::InvalidInput(
                "authenticated verifier identity must be non-empty".into(),
            ));
        }
        let Some(evaluation) = episode.evaluation.as_ref() else {
            return Err(EngineError::InvalidInput(
                "authenticated observations require an evaluation".into(),
            ));
        };
        if !is_strong(evaluation.tier) {
            return Err(EngineError::InvalidInput(
                "authenticated observations must use Hard or Consensus evidence".into(),
            ));
        }
        self.insert(TrustReceipt {
            evidence_kind: TrustEvidenceKind::Episode,
            evidence_id: episode.id.to_string(),
            evidence_digest: episode_digest(episode)?,
            tier: evaluation.tier,
            issuer: format!("authenticated-verifier:{verifier_identity}"),
            issued_at: episode.created_at,
        })
    }

    pub(crate) fn mint_episode_facts(&self, episode: &Episode) -> Result<(), EngineError> {
        let Some(evaluation) = episode.evaluation.as_ref() else {
            return Ok(());
        };
        if !is_strong(evaluation.tier) {
            return Ok(());
        }
        for fact in &episode.observed_facts {
            if fact.id.trim().is_empty() {
                return Err(EngineError::InvalidInput(
                    "trusted observed facts require a stable fact id".into(),
                ));
            }
            self.insert(TrustReceipt {
                evidence_kind: TrustEvidenceKind::Fact,
                evidence_id: fact.id.clone(),
                evidence_digest: fact_digest(episode, fact)?,
                tier: evaluation.tier,
                issuer: self
                    .verified_engine_episode(episode, evaluation.tier)?
                    .ok_or_else(|| {
                        EngineError::InvalidInput(
                            "trusted fact receipt requires a matching episode receipt".into(),
                        )
                    })?
                    .issuer,
                issued_at: episode.created_at,
            })?;
        }
        Ok(())
    }

    pub(crate) fn mint_authenticated_feedback(
        &self,
        feedback: &EpisodeFeedback,
        verifier_identity: &str,
    ) -> Result<(), EngineError> {
        if verifier_identity.trim().is_empty() {
            return Err(EngineError::InvalidInput(
                "authenticated verifier identity must be non-empty".into(),
            ));
        }
        if !is_strong(feedback.evaluation.tier) {
            return Err(EngineError::InvalidInput(
                "authenticated verifier feedback must use Hard or Consensus evidence".into(),
            ));
        }
        self.insert(TrustReceipt {
            evidence_kind: TrustEvidenceKind::Feedback,
            evidence_id: feedback.id.to_string(),
            evidence_digest: feedback_digest(feedback)?,
            tier: feedback.evaluation.tier,
            issuer: format!("authenticated-verifier:{verifier_identity}"),
            issued_at: feedback.created_at,
        })
    }

    pub(crate) fn verified_engine_episode(
        &self,
        episode: &Episode,
        tier: VerifiabilityTier,
    ) -> Result<Option<TrustReceipt>, EngineError> {
        if !is_strong(tier) {
            return Ok(None);
        }
        self.get_matching(
            TrustEvidenceKind::Episode,
            &episode.id.to_string(),
            &episode_digest(episode)?,
            tier,
        )
    }

    pub(crate) fn verified_feedback(
        &self,
        feedback: &EpisodeFeedback,
        tier: VerifiabilityTier,
    ) -> Result<Option<TrustReceipt>, EngineError> {
        if !is_strong(tier) {
            return Ok(None);
        }
        self.get_matching(
            TrustEvidenceKind::Feedback,
            &feedback.id.to_string(),
            &feedback_digest(feedback)?,
            tier,
        )
    }

    pub(crate) fn receipt_for_episode(
        &self,
        episode: &Episode,
    ) -> Result<Option<TrustReceipt>, EngineError> {
        let tier = episode.evaluation.as_ref().map(|item| item.tier);
        match tier {
            Some(tier) => self.verified_engine_episode(episode, tier),
            None => Ok(None),
        }
    }

    pub(crate) fn verified_fact(
        &self,
        episode: &Episode,
        fact: &ObservedFact,
    ) -> Result<Option<TrustReceipt>, EngineError> {
        let Some(tier) = episode.evaluation.as_ref().map(|item| item.tier) else {
            return Ok(None);
        };
        if !is_strong(tier) || fact.id.trim().is_empty() {
            return Ok(None);
        }
        self.get_matching(
            TrustEvidenceKind::Fact,
            &fact.id,
            &fact_digest(episode, fact)?,
            tier,
        )
    }

    fn insert(&self, receipt: TrustReceipt) -> Result<(), EngineError> {
        let existing = self
            .conn
            .query_row(
                "SELECT evidence_digest, tier, issuer, issued_at
                 FROM engine_trust_receipts
                 WHERE evidence_kind = ?1 AND evidence_id = ?2",
                params![kind_name(receipt.evidence_kind), receipt.evidence_id],
                |row| {
                    Ok(TrustReceipt {
                        evidence_kind: receipt.evidence_kind,
                        evidence_id: receipt.evidence_id.clone(),
                        evidence_digest: row.get(0)?,
                        tier: serde_json::from_str::<VerifiabilityTier>(&row.get::<_, String>(1)?)
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    1,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                        issuer: row.get(2)?,
                        issued_at: row.get(3)?,
                    })
                },
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing == receipt {
                return Ok(());
            }
            return Err(EngineError::InvalidInput(format!(
                "trust receipt conflict for {} evidence {}",
                kind_name(receipt.evidence_kind),
                receipt.evidence_id
            )));
        }
        self.conn.execute(
            "INSERT INTO engine_trust_receipts
                (evidence_kind, evidence_id, evidence_digest, tier, issuer, issued_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                kind_name(receipt.evidence_kind),
                receipt.evidence_id,
                receipt.evidence_digest,
                serde_json::to_string(&receipt.tier)?,
                receipt.issuer,
                receipt.issued_at,
            ],
        )?;
        Ok(())
    }

    fn get_matching(
        &self,
        kind: TrustEvidenceKind,
        id: &str,
        digest: &str,
        tier: VerifiabilityTier,
    ) -> Result<Option<TrustReceipt>, EngineError> {
        let receipt = self
            .conn
            .query_row(
                "SELECT evidence_digest, tier, issuer, issued_at
                 FROM engine_trust_receipts
                 WHERE evidence_kind = ?1 AND evidence_id = ?2",
                params![kind_name(kind), id],
                |row| {
                    let tier_json: String = row.get(1)?;
                    let stored_tier = serde_json::from_str(&tier_json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(TrustReceipt {
                        evidence_kind: kind,
                        evidence_id: id.to_owned(),
                        evidence_digest: row.get(0)?,
                        tier: stored_tier,
                        issuer: row.get(2)?,
                        issued_at: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(receipt.filter(|receipt| receipt.evidence_digest == digest && receipt.tier == tier))
    }
}

fn is_strong(tier: VerifiabilityTier) -> bool {
    matches!(tier, VerifiabilityTier::Hard | VerifiabilityTier::Consensus)
}

fn kind_name(kind: TrustEvidenceKind) -> &'static str {
    match kind {
        TrustEvidenceKind::Episode => "episode",
        TrustEvidenceKind::Feedback => "feedback",
        TrustEvidenceKind::Fact => "fact",
    }
}

fn episode_digest(episode: &Episode) -> Result<String, EngineError> {
    digest(b"spoon:engine-trust:episode:v1\0", episode)
}

fn feedback_digest(feedback: &EpisodeFeedback) -> Result<String, EngineError> {
    digest(b"spoon:engine-trust:feedback:v1\0", feedback)
}

fn fact_digest(episode: &Episode, fact: &ObservedFact) -> Result<String, EngineError> {
    digest(b"spoon:engine-trust:fact:v1\0", &(episode.id, fact))
}

/// Hashes evidence by its parsed value rather than by one particular spelling
/// of that value.
///
/// A receipt is issued against a live struct and later checked against the same
/// evidence loaded back from storage, so the digest only means something if
/// both passes agree. Serializing directly does not give that: JSON floats do
/// not survive a parse and re-serialize intact, and `0.9800000190734863` comes
/// back as `0.9800000190734864`. Every episode carrying an interpreter
/// confidence therefore failed its own receipt, which silently disabled recall.
///
/// Normalizing through one parse settles the value on the spelling storage will
/// hand back, and sorts object keys on the way, so the digest is stable no
/// matter which side computes it.
fn digest(value_domain: &[u8], value: &impl Serialize) -> Result<String, EngineError> {
    let raw = serde_json::to_vec(value)?;
    let normalized: serde_json::Value = serde_json::from_slice(&raw)?;
    let mut hasher = Sha256::new();
    hasher.update(value_domain);
    hasher.update(serde_json::to_vec(&normalized)?);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::digest;

    #[test]
    fn a_digest_survives_the_trip_through_storage() {
        // An interpreter confidence of 0.98 reaches JSON as the f64 widening of
        // an f32, and that exact value is the one serde_json reparses one ulp
        // away. Issuing a receipt against a live value and checking it against
        // the stored copy has to agree, or the receipt attests to nothing.
        let live = serde_json::json!({ "confidence": 0.98f32 as f64 });
        let stored = serde_json::to_vec(&live).unwrap();
        let reloaded: serde_json::Value = serde_json::from_slice(&stored).unwrap();

        assert_ne!(
            stored,
            serde_json::to_vec(&reloaded).unwrap(),
            "the hazard is gone and this test no longer proves anything"
        );
        assert_eq!(
            digest(b"test\0", &live).unwrap(),
            digest(b"test\0", &reloaded).unwrap()
        );
    }
}
