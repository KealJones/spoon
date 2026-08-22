use ekg_core::{ConceptId, EpisodeId, ProcedureId, Value};
use ekg_episode::EpisodeStore;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::error::{AdaptError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContradictionId(pub i64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Implication {
    pub predicate: String,
    pub value: Value,
}

impl Implication {
    pub fn new(predicate: impl Into<String>, value: Value) -> Self {
        Self {
            predicate: predicate.into(),
            value,
        }
    }

    pub fn for_concept(concept: ConceptId, value: Value) -> Self {
        Self::new(format!("concept:{concept}"), value)
    }

    pub fn for_procedure(procedure: ProcedureId, value: Value) -> Self {
        Self::new(format!("procedure:{procedure}:result"), value)
    }

    fn conflicts_with(&self, other: &Self) -> bool {
        self.predicate == other.predicate && self.value != other.value
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeAssignment {
    pub feature: String,
    pub value: Value,
    pub learned_from: EpisodeId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub statement: String,
    pub implication: Implication,
    pub supporting_episodes: Vec<EpisodeId>,
    pub scope: Vec<ScopeAssignment>,
}

impl Claim {
    pub fn new(
        id: impl Into<String>,
        statement: impl Into<String>,
        implication: Implication,
        supporting_episodes: Vec<EpisodeId>,
    ) -> Self {
        Self {
            id: id.into(),
            statement: statement.into(),
            implication,
            supporting_episodes,
            scope: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DemonstratedFeature {
    pub feature: String,
    pub left_value: Value,
    pub left_episode: EpisodeId,
    pub right_value: Value,
    pub right_episode: EpisodeId,
}

impl DemonstratedFeature {
    pub fn new(
        feature: impl Into<String>,
        left_value: Value,
        left_episode: EpisodeId,
        right_value: Value,
        right_episode: EpisodeId,
    ) -> Result<Self> {
        let feature = feature.into();
        if feature.trim().is_empty() {
            return Err(AdaptError::Invalid(
                "discriminating feature name must be non-empty".into(),
            ));
        }
        if left_value == right_value {
            return Err(AdaptError::Invalid(
                "discriminating feature must differ across supporting cases".into(),
            ));
        }
        if left_episode == right_episode {
            return Err(AdaptError::Invalid(
                "discriminating feature needs distinct supporting episodes".into(),
            ));
        }
        Ok(Self {
            feature,
            left_value,
            left_episode,
            right_value,
            right_episode,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Refinement {
    pub left: Claim,
    pub right: Claim,
    pub discriminator: DemonstratedFeature,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedPredicateRefinement {
    pub contradiction_id: ContradictionId,
    pub claim: Claim,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PredicateRefinementContext {
    pub applied: Vec<AppliedPredicateRefinement>,
    pub unresolved: Vec<ContradictionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContradictionStatus {
    Held,
    Refined,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contradiction {
    pub id: ContradictionId,
    pub left: Claim,
    pub right: Claim,
    pub status: ContradictionStatus,
    pub refinement: Option<Refinement>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Uncertainty {
    Certain,
    HeldContradictions(Vec<ContradictionId>),
}

pub struct ContradictionStore {
    conn: Connection,
}

impl ContradictionStore {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Self { conn })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self { conn })
    }

    fn init(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS contradictions (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                canonical_key   TEXT NOT NULL UNIQUE,
                left_json       TEXT NOT NULL,
                right_json      TEXT NOT NULL,
                status_json     TEXT NOT NULL,
                refinement_json TEXT,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_contradictions_status
                ON contradictions(status_json);
             CREATE TABLE IF NOT EXISTS claim_dependencies (
                dependent_claim_id TEXT NOT NULL,
                support_claim_id   TEXT NOT NULL,
                PRIMARY KEY (dependent_claim_id, support_claim_id)
             );
             CREATE INDEX IF NOT EXISTS idx_claim_dependencies_dependent
                ON claim_dependencies(dependent_claim_id);",
        )?;
        Ok(())
    }

    pub fn record(
        &self,
        left: Claim,
        right: Claim,
        episodes: &EpisodeStore,
        created_at: i64,
    ) -> Result<Contradiction> {
        if left.id.trim().is_empty() || right.id.trim().is_empty() {
            return Err(AdaptError::Invalid("claim ids must be non-empty".into()));
        }
        if left.id == right.id {
            return Err(AdaptError::Invalid(
                "a contradiction requires two distinct claims".into(),
            ));
        }
        if left.supporting_episodes.is_empty() || right.supporting_episodes.is_empty() {
            return Err(AdaptError::Invalid(
                "both contradictory claims need supporting episodes".into(),
            ));
        }
        validate_claim_evidence(&left, episodes)?;
        validate_claim_evidence(&right, episodes)?;
        if !left.implication.conflicts_with(&right.implication)
            || !scopes_can_overlap(&left, &right)
        {
            return Err(AdaptError::Invalid(
                "claims do not have conflicting implications in overlapping scopes".into(),
            ));
        }
        let canonical_key = canonical_pair_key(&left, &right);
        let transaction = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if let Some(existing_id) = transaction
            .query_row(
                "SELECT id FROM contradictions WHERE canonical_key = ?1",
                params![canonical_key],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            let existing = Self::get_in(&transaction, ContradictionId(existing_id))?
                .ok_or_else(|| AdaptError::NotFound(format!("contradiction {existing_id}")))?;
            if !same_claim_pair(&existing, &left, &right) {
                return Err(AdaptError::Invalid(
                    "canonical claim ids already refer to different content".into(),
                ));
            }
            transaction.commit()?;
            return Ok(existing);
        }
        transaction.execute(
            "INSERT INTO contradictions
                (canonical_key, left_json, right_json, status_json, refinement_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5)",
            params![
                canonical_key,
                serde_json::to_string(&left)?,
                serde_json::to_string(&right)?,
                serde_json::to_string(&ContradictionStatus::Held)?,
                created_at,
            ],
        )?;
        let id = ContradictionId(transaction.last_insert_rowid());
        transaction.commit()?;
        Ok(Contradiction {
            id,
            left,
            right,
            status: ContradictionStatus::Held,
            refinement: None,
            created_at,
            updated_at: created_at,
        })
    }

    pub fn get(&self, id: ContradictionId) -> Result<Option<Contradiction>> {
        Self::get_in(&self.conn, id)
    }

    fn get_in(conn: &Connection, id: ContradictionId) -> Result<Option<Contradiction>> {
        let row = conn
            .query_row(
                "SELECT left_json, right_json, status_json, refinement_json,
                        created_at, updated_at
                 FROM contradictions WHERE id = ?1",
                params![id.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()?;
        row.map(|row| Self::decode(id, row)).transpose()
    }

    pub fn list_held(&self) -> Result<Vec<Contradiction>> {
        Ok(self
            .list_all()?
            .into_iter()
            .filter(|contradiction| contradiction.status == ContradictionStatus::Held)
            .collect())
    }

    fn list_all(&self) -> Result<Vec<Contradiction>> {
        let mut statement = self.conn.prepare(
            "SELECT id, left_json, right_json, status_json, refinement_json,
                    created_at, updated_at
             FROM contradictions ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                ContradictionId(row.get::<_, i64>(0)?),
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        let mut contradictions = Vec::new();
        for row in rows {
            let (id, left, right, status, refinement, created_at, updated_at) = row?;
            let contradiction = Self::decode(
                id,
                (left, right, status, refinement, created_at, updated_at),
            )?;
            contradictions.push(contradiction);
        }
        Ok(contradictions)
    }

    pub fn uncertainty_for_claim(&self, claim_id: &str) -> Result<Uncertainty> {
        if claim_id.trim().is_empty() {
            return Err(AdaptError::Invalid("claim id must be non-empty".into()));
        }
        let reachable = self.reachable_support_claims(claim_id)?;
        let ids = self
            .list_held()?
            .into_iter()
            .filter(|conflict| {
                reachable.contains(&conflict.left.id) || reachable.contains(&conflict.right.id)
            })
            .map(|conflict| conflict.id)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            Ok(Uncertainty::Certain)
        } else {
            Ok(Uncertainty::HeldContradictions(ids))
        }
    }

    pub fn held_for_predicate(&self, predicate: &str) -> Result<Vec<ContradictionId>> {
        if predicate.trim().is_empty() {
            return Err(AdaptError::Invalid("predicate must be non-empty".into()));
        }
        Ok(self
            .list_held()?
            .into_iter()
            .filter(|conflict| {
                conflict.left.implication.predicate == predicate
                    && conflict.right.implication.predicate == predicate
            })
            .map(|conflict| conflict.id)
            .collect())
    }

    pub fn refinement_context_for_predicate(
        &self,
        predicate: &str,
        environment: &std::collections::BTreeMap<String, Value>,
    ) -> Result<PredicateRefinementContext> {
        if predicate.trim().is_empty() {
            return Err(AdaptError::Invalid("predicate must be non-empty".into()));
        }
        let mut context = PredicateRefinementContext::default();
        for contradiction in self.list_all()?.into_iter().filter(|contradiction| {
            contradiction.status == ContradictionStatus::Refined
                && contradiction.left.implication.predicate == predicate
                && contradiction.right.implication.predicate == predicate
        }) {
            let refinement = contradiction.refinement.as_ref().ok_or_else(|| {
                AdaptError::Invalid(format!(
                    "refined contradiction {} has no refinement payload",
                    contradiction.id.0
                ))
            })?;
            let matches = [&refinement.left, &refinement.right]
                .into_iter()
                .filter(|claim| {
                    !claim.scope.is_empty()
                        && claim.scope.iter().all(|assignment| {
                            environment.get(&assignment.feature) == Some(&assignment.value)
                        })
                })
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                context.applied.push(AppliedPredicateRefinement {
                    contradiction_id: contradiction.id,
                    claim: matches[0].clone(),
                });
            } else {
                context.unresolved.push(contradiction.id);
            }
        }
        Ok(context)
    }

    pub fn refinements_for_claim(&self, claim_id: &str) -> Result<Vec<Refinement>> {
        if claim_id.trim().is_empty() {
            return Err(AdaptError::Invalid("claim id must be non-empty".into()));
        }
        let reachable = self.reachable_support_claims(claim_id)?;
        let mut statement = self.conn.prepare(
            "SELECT refinement_json
             FROM contradictions
             WHERE status_json = ?1 AND refinement_json IS NOT NULL
             ORDER BY id",
        )?;
        let rows = statement.query_map(
            params![serde_json::to_string(&ContradictionStatus::Refined)?],
            |row| row.get::<_, String>(0),
        )?;
        let mut refinements = Vec::new();
        for row in rows {
            let refinement: Refinement = serde_json::from_str(&row?)?;
            if reachable.contains(&refinement.left.id) || reachable.contains(&refinement.right.id) {
                refinements.push(refinement);
            }
        }
        Ok(refinements)
    }

    pub fn add_claim_dependency(
        &self,
        dependent_claim_id: &str,
        support_claim_id: &str,
    ) -> Result<()> {
        if dependent_claim_id.trim().is_empty() || support_claim_id.trim().is_empty() {
            return Err(AdaptError::Invalid(
                "claim dependency ids must be non-empty".into(),
            ));
        }
        if dependent_claim_id == support_claim_id {
            return Err(AdaptError::Invalid(
                "a claim cannot directly depend on itself".into(),
            ));
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO claim_dependencies
                (dependent_claim_id, support_claim_id) VALUES (?1, ?2)",
            params![dependent_claim_id, support_claim_id],
        )?;
        Ok(())
    }

    /// Whether an identifier is backed by a recorded claim or by the
    /// canonical predicate of one. Predicate identifiers let a reasoning
    /// concept depend on a more specific observed claim without inventing an
    /// unaudited placeholder claim.
    pub fn contains_claim_identifier(&self, claim_id: &str) -> Result<bool> {
        if claim_id.trim().is_empty() {
            return Ok(false);
        }
        Ok(self.list_all()?.into_iter().any(|contradiction| {
            [&contradiction.left, &contradiction.right]
                .into_iter()
                .any(|claim| claim.id == claim_id || claim.implication.predicate == claim_id)
        }))
    }

    fn reachable_support_claims(
        &self,
        claim_id: &str,
    ) -> Result<std::collections::HashSet<String>> {
        let mut statement = self.conn.prepare(
            "WITH RECURSIVE reachable(claim_id) AS (
                SELECT ?1
                UNION
                SELECT dependencies.support_claim_id
                FROM claim_dependencies AS dependencies
                JOIN reachable
                  ON dependencies.dependent_claim_id = reachable.claim_id
             )
             SELECT claim_id FROM reachable",
        )?;
        let rows = statement.query_map(params![claim_id], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?)
    }

    pub fn refine(
        &self,
        id: ContradictionId,
        discriminator: DemonstratedFeature,
        episodes: &EpisodeStore,
        updated_at: i64,
    ) -> Result<Refinement> {
        let transaction = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let contradiction = Self::get_in(&transaction, id)?
            .ok_or_else(|| AdaptError::NotFound(format!("contradiction {}", id.0)))?;
        validate_claim_evidence(&contradiction.left, episodes)?;
        validate_claim_evidence(&contradiction.right, episodes)?;
        validate_discriminator_episodes(&discriminator, episodes)?;
        if contradiction.status != ContradictionStatus::Held {
            return Err(AdaptError::Invalid(
                "only a held contradiction can be refined".into(),
            ));
        }
        if !contradiction
            .left
            .supporting_episodes
            .contains(&discriminator.left_episode)
            || !contradiction
                .right
                .supporting_episodes
                .contains(&discriminator.right_episode)
        {
            return Err(AdaptError::Invalid(
                "discriminating feature must be demonstrated by each claim's evidence".into(),
            ));
        }
        let mut left = contradiction.left.clone();
        left.scope.push(ScopeAssignment {
            feature: discriminator.feature.clone(),
            value: discriminator.left_value.clone(),
            learned_from: discriminator.left_episode,
        });
        let mut right = contradiction.right.clone();
        right.scope.push(ScopeAssignment {
            feature: discriminator.feature.clone(),
            value: discriminator.right_value.clone(),
            learned_from: discriminator.right_episode,
        });
        let refinement = Refinement {
            left,
            right,
            discriminator,
        };
        let changed = transaction.execute(
            "UPDATE contradictions
             SET status_json = ?2, refinement_json = ?3, updated_at = ?4
             WHERE id = ?1 AND status_json = ?5",
            params![
                id.0,
                serde_json::to_string(&ContradictionStatus::Refined)?,
                serde_json::to_string(&refinement)?,
                updated_at,
                serde_json::to_string(&ContradictionStatus::Held)?,
            ],
        )?;
        if changed != 1 {
            return Err(AdaptError::Unauthorized(
                "held contradiction changed before refinement commit".into(),
            ));
        }
        transaction.commit()?;
        Ok(refinement)
    }

    fn decode(
        id: ContradictionId,
        row: (String, String, String, Option<String>, i64, i64),
    ) -> Result<Contradiction> {
        let (left, right, status, refinement, created_at, updated_at) = row;
        Ok(Contradiction {
            id,
            left: serde_json::from_str(&left)?,
            right: serde_json::from_str(&right)?,
            status: serde_json::from_str(&status)?,
            refinement: refinement
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            created_at,
            updated_at,
        })
    }
}

fn validate_claim_evidence(claim: &Claim, episodes: &EpisodeStore) -> Result<()> {
    if claim.implication.predicate.trim().is_empty() {
        return Err(AdaptError::Invalid(
            "claim implication predicate must be non-empty".into(),
        ));
    }
    for episode_id in &claim.supporting_episodes {
        let episode = episodes.get(*episode_id)?;
        let evaluation = episode.evaluation.as_ref().ok_or_else(|| {
            AdaptError::Unauthorized(
                "claim evidence must be a successful Hard or Consensus episode".into(),
            )
        })?;
        if !evaluation.success
            || !matches!(
                evaluation.tier,
                ekg_core::VerifiabilityTier::Hard | ekg_core::VerifiabilityTier::Consensus
            )
        {
            return Err(AdaptError::Unauthorized(
                "claim evidence must be a successful Hard or Consensus episode".into(),
            ));
        }
        let demonstrates_exact_fact = episode.observed_facts.iter().any(|fact| {
            fact.predicate == claim.implication.predicate && fact.value == claim.implication.value
        });
        if !demonstrates_exact_fact {
            return Err(AdaptError::Unauthorized(format!(
                "claim evidence does not contain the exact observed predicate {:?} and value",
                claim.implication.predicate
            )));
        }
    }
    Ok(())
}

fn canonical_pair_key(left: &Claim, right: &Claim) -> String {
    if left.id <= right.id {
        format!("{}\u{0}{}", left.id, right.id)
    } else {
        format!("{}\u{0}{}", right.id, left.id)
    }
}

fn same_claim_pair(existing: &Contradiction, left: &Claim, right: &Claim) -> bool {
    (existing.left == *left && existing.right == *right)
        || (existing.left == *right && existing.right == *left)
}

fn validate_discriminator_episodes(
    discriminator: &DemonstratedFeature,
    episodes: &EpisodeStore,
) -> Result<()> {
    for (episode_id, expected_value) in [
        (discriminator.left_episode, &discriminator.left_value),
        (discriminator.right_episode, &discriminator.right_value),
    ] {
        let episode = episodes.get(episode_id)?;
        let evaluation = episode.evaluation.as_ref().ok_or_else(|| {
            AdaptError::Unauthorized("discriminator episode has no evaluation".into())
        })?;
        if !evaluation.success
            || !matches!(
                evaluation.tier,
                ekg_core::VerifiabilityTier::Hard | ekg_core::VerifiabilityTier::Consensus
            )
            || episode.context.environment.get(&discriminator.feature) != Some(expected_value)
        {
            return Err(AdaptError::Unauthorized(
                "discriminator is not demonstrated by stored verified episode context".into(),
            ));
        }
    }
    Ok(())
}

fn scopes_can_overlap(left: &Claim, right: &Claim) -> bool {
    !left.scope.iter().any(|left_scope| {
        right.scope.iter().any(|right_scope| {
            left_scope.feature == right_scope.feature && left_scope.value != right_scope.value
        })
    })
}
