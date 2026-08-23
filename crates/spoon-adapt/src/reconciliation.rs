use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use spoon_core::{
    ConceptId, Lifecycle, ProcedureId, Relationship, RelationshipId, VerifiabilityTier,
};
use spoon_episode::EpisodeStore;
use spoon_graph::{
    DependencyTarget, Dependent, KnowledgeStore, LifecycleChange, LifecycleChangeReceipt,
    LifecycleChangeSet, RelationshipDependency,
};

use crate::error::{AdaptError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KnowledgeRef {
    Concept(ConceptId),
    Procedure(ProcedureId),
    Relationship(RelationshipId),
}

impl KnowledgeRef {
    fn dependency_target(self) -> Option<DependencyTarget> {
        match self {
            Self::Concept(id) => Some(DependencyTarget::Concept(id)),
            Self::Procedure(id) => Some(DependencyTarget::Procedure(id)),
            Self::Relationship(_) => None,
        }
    }
}

impl From<Dependent> for KnowledgeRef {
    fn from(value: Dependent) -> Self {
        match value {
            Dependent::Concept(id) => Self::Concept(id),
            Dependent::Procedure { id, .. } => Self::Procedure(id),
        }
    }
}

impl From<RelationshipDependency> for KnowledgeRef {
    fn from(value: RelationshipDependency) -> Self {
        Self::Relationship(value.relationship_id)
    }
}

pub trait AlternativeSupport {
    fn has_alternative_support(
        &self,
        graph: &KnowledgeStore,
        changed: KnowledgeRef,
        dependent: KnowledgeRef,
    ) -> Result<AlternativeSupportVerdict>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlternativeSupportVerdict {
    Proven,
    Absent,
    Unknown,
}

/// Conservative graph support check backed by canonical episode evidence.
///
/// A validated `alternative-support:<changed-id>` edge proves support only
/// when every linked episode is finalized, claim-specific, and consistently
/// supports it with Hard or Consensus evidence. Conflicting or weaker late
/// feedback revokes proof. Arbitrary UUIDs, co-mentions, and cherry-picked
/// successes are never proof.
pub struct GraphAlternativeSupport<'a> {
    episodes: &'a EpisodeStore,
    trusted_episode_ids: Option<&'a HashSet<spoon_core::EpisodeId>>,
}

impl<'a> GraphAlternativeSupport<'a> {
    pub fn new(episodes: &'a EpisodeStore) -> Self {
        Self {
            episodes,
            trusted_episode_ids: None,
        }
    }

    pub fn new_trusted(
        episodes: &'a EpisodeStore,
        trusted_episode_ids: &'a HashSet<spoon_core::EpisodeId>,
    ) -> Self {
        Self {
            episodes,
            trusted_episode_ids: Some(trusted_episode_ids),
        }
    }

    fn relationship_is_verified(&self, relationship: &Relationship) -> Result<bool> {
        if relationship.evidence.is_empty() {
            return Ok(false);
        }

        for episode_id in &relationship.evidence {
            if self
                .trusted_episode_ids
                .is_some_and(|trusted| !trusted.contains(episode_id))
            {
                return Ok(false);
            }
            let episode = match self.episodes.get(*episode_id) {
                Ok(episode) => episode,
                Err(spoon_core::SpoonError::NotFound(_)) => return Ok(false),
                Err(error) => return Err(error.into()),
            };
            let Some(evaluation) = episode.evaluation.as_ref() else {
                return Ok(false);
            };
            if !strong_success(evaluation.tier, evaluation.success)
                || !episode_supports_relationship(&episode, relationship)
            {
                return Ok(false);
            }

            for feedback in self.episodes.list_feedback(*episode_id)? {
                if !strong_success(feedback.evaluation.tier, feedback.evaluation.success)
                    || episode
                        .observed_result
                        .as_ref()
                        .is_some_and(|observed| observed != &feedback.observed_result)
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn relationship_can_prove(
        &self,
        graph: &KnowledgeStore,
        relationship: &Relationship,
    ) -> Result<bool> {
        Ok(relationship.lifecycle == Lifecycle::Validated
            && relationship.strength > 0.0
            && graph
                .get_concept(relationship.target)?
                .is_some_and(|concept| lifecycle_is_usable(concept.lifecycle))
            && self.relationship_is_verified(relationship)?)
    }
}

fn strong_success(tier: VerifiabilityTier, success: bool) -> bool {
    success && matches!(tier, VerifiabilityTier::Hard | VerifiabilityTier::Consensus)
}

fn episode_supports_relationship(episode: &spoon_core::Episode, expected: &Relationship) -> bool {
    episode.context.relevant_knowledge.iter().any(|context| {
        let actual = &context.relationship;
        actual.id == expected.id
            && actual.source == expected.source
            && actual.target == expected.target
            && actual.kind == expected.kind
    })
}

impl AlternativeSupport for GraphAlternativeSupport<'_> {
    fn has_alternative_support(
        &self,
        graph: &KnowledgeStore,
        changed: KnowledgeRef,
        dependent: KnowledgeRef,
    ) -> Result<AlternativeSupportVerdict> {
        let (proof_kind, dependent_concept, excluded_concept, fallback_for) =
            match (changed, dependent) {
                (KnowledgeRef::Concept(changed_id), KnowledgeRef::Concept(dependent_id)) => (
                    format!("alternative-support:{changed_id}"),
                    dependent_id,
                    Some(changed_id),
                    None,
                ),
                (KnowledgeRef::Procedure(changed_id), KnowledgeRef::Procedure(dependent_id)) => {
                    let Some(dependent) = graph.get_procedure(dependent_id)? else {
                        return Ok(AlternativeSupportVerdict::Unknown);
                    };
                    let Some(dependent_concept) = dependent.concept else {
                        return Ok(AlternativeSupportVerdict::Unknown);
                    };
                    (
                        format!("alternative-support:{changed_id}"),
                        dependent_concept,
                        None,
                        Some((changed_id, dependent_id)),
                    )
                }
                _ => return Ok(AlternativeSupportVerdict::Unknown),
            };
        let relationships = graph.get_relationships_from(dependent_concept)?;
        for relationship in relationships
            .iter()
            .filter(|relationship| relationship.kind == proof_kind)
        {
            if excluded_concept == Some(relationship.target)
                || relationship.target == dependent_concept
            {
                continue;
            }
            if !self.relationship_can_prove(graph, relationship)? {
                continue;
            }
            if let Some((changed_id, dependent_id)) = fallback_for
                && !graph.list_procedures()?.into_iter().any(|procedure| {
                    procedure.concept == Some(relationship.target)
                        && lifecycle_is_usable(procedure.lifecycle)
                        && procedure.id != changed_id
                        && procedure.id != dependent_id
                })
            {
                continue;
            }
            return Ok(AlternativeSupportVerdict::Proven);
        }
        Ok(AlternativeSupportVerdict::Unknown)
    }
}

fn lifecycle_is_usable(lifecycle: Lifecycle) -> bool {
    !matches!(
        lifecycle,
        Lifecycle::Stale | Lifecycle::Superseded | Lifecycle::Retired | Lifecycle::Invalid
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReconciliationOutcome {
    PreservedByAlternativeSupport,
    MarkStale,
    MarkUnderReview,
}

impl ReconciliationOutcome {
    fn lifecycle(self, previous: Lifecycle) -> Lifecycle {
        match self {
            Self::PreservedByAlternativeSupport => previous,
            Self::MarkStale => Lifecycle::Stale,
            Self::MarkUnderReview => Lifecycle::UnderReview,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationEntry {
    pub knowledge: KnowledgeRef,
    pub depth: usize,
    pub expected_version: u32,
    pub previous_lifecycle: Lifecycle,
    pub next_lifecycle: Lifecycle,
    pub outcome: ReconciliationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationPlan {
    pub changed: KnowledgeRef,
    pub entries: Vec<ReconciliationEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedReconciliation {
    idempotency_key: String,
    plan: ReconciliationPlan,
    updated_at: i64,
}

impl StagedReconciliation {
    pub fn new(
        idempotency_key: impl Into<String>,
        plan: ReconciliationPlan,
        updated_at: i64,
    ) -> Result<Self> {
        let idempotency_key = idempotency_key.into();
        if idempotency_key.trim().is_empty() {
            return Err(AdaptError::Invalid(
                "reconciliation idempotency key must be non-empty".into(),
            ));
        }
        Ok(Self {
            idempotency_key,
            plan,
            updated_at,
        })
    }

    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    pub fn plan(&self) -> &ReconciliationPlan {
        &self.plan
    }

    pub fn updated_at(&self) -> i64 {
        self.updated_at
    }

    pub fn remaining<'a>(&'a self, graph: &KnowledgeStore) -> Result<Vec<&'a ReconciliationEntry>> {
        if self.receipt(graph)?.is_some() {
            Ok(Vec::new())
        } else {
            Ok(self
                .plan
                .entries
                .iter()
                .filter(|entry| {
                    entry.outcome != ReconciliationOutcome::PreservedByAlternativeSupport
                        && entry.previous_lifecycle != entry.next_lifecycle
                })
                .collect())
        }
    }

    /// Materializes the exact graph transaction request represented by this
    /// durable stage. Recovery can serialize the stage, reconstruct this
    /// request, and validate any existing receipt against the whole plan.
    pub fn change_set(&self) -> LifecycleChangeSet {
        LifecycleChangeSet {
            idempotency_key: self.idempotency_key.clone(),
            updated_at: self.updated_at,
            changes: self
                .plan
                .entries
                .iter()
                .filter(|entry| {
                    entry.outcome != ReconciliationOutcome::PreservedByAlternativeSupport
                        && entry.previous_lifecycle != entry.next_lifecycle
                })
                .map(|entry| match entry.knowledge {
                    KnowledgeRef::Concept(id) => LifecycleChange::Concept {
                        id,
                        expected_version: entry.expected_version,
                        lifecycle: entry.next_lifecycle,
                    },
                    KnowledgeRef::Procedure(id) => LifecycleChange::Procedure {
                        id,
                        expected_version: entry.expected_version,
                        lifecycle: entry.next_lifecycle,
                    },
                    KnowledgeRef::Relationship(id) => LifecycleChange::Relationship {
                        id,
                        expected_version: entry.expected_version,
                        lifecycle: entry.next_lifecycle,
                    },
                })
                .collect(),
        }
    }

    pub fn receipt(&self, graph: &KnowledgeStore) -> Result<Option<LifecycleChangeReceipt>> {
        Ok(graph.get_change_set_receipt(&self.change_set())?)
    }

    pub fn is_applied(&self, graph: &KnowledgeStore) -> Result<bool> {
        Ok(self.receipt(graph)?.is_some())
    }
}

pub struct ReconciliationPlanner;

impl ReconciliationPlanner {
    pub fn plan(
        graph: &KnowledgeStore,
        changed: KnowledgeRef,
        support: &impl AlternativeSupport,
    ) -> Result<ReconciliationPlan> {
        let mut entries = Vec::new();
        let mut visited = HashSet::from([changed]);
        let mut pending = VecDeque::from([(changed, 0_usize)]);

        while let Some((target, target_depth)) = pending.pop_front() {
            let Some(dependency_target) = target.dependency_target() else {
                continue;
            };
            let report = graph.get_dependency_report(dependency_target)?;
            for dependent in report.dependents {
                let dependent = KnowledgeRef::from(dependent);
                if !visited.insert(dependent) {
                    continue;
                }
                let depth = target_depth.saturating_add(1);
                let (previous_lifecycle, expected_version) = current_state(graph, dependent)?;
                let support_verdict = support.has_alternative_support(graph, target, dependent)?;
                let outcome = match support_verdict {
                    AlternativeSupportVerdict::Proven => {
                        ReconciliationOutcome::PreservedByAlternativeSupport
                    }
                    AlternativeSupportVerdict::Absent if depth == 1 => {
                        ReconciliationOutcome::MarkStale
                    }
                    AlternativeSupportVerdict::Absent | AlternativeSupportVerdict::Unknown => {
                        ReconciliationOutcome::MarkUnderReview
                    }
                };
                entries.push(ReconciliationEntry {
                    knowledge: dependent,
                    depth,
                    expected_version,
                    previous_lifecycle,
                    next_lifecycle: outcome.lifecycle(previous_lifecycle),
                    outcome,
                });
                if support_verdict != AlternativeSupportVerdict::Proven {
                    pending.push_back((dependent, depth));
                }
            }
            for relationship in report.relationships {
                let dependent = KnowledgeRef::from(relationship);
                if !visited.insert(dependent) {
                    continue;
                }
                let depth = target_depth.saturating_add(1);
                let (previous_lifecycle, expected_version) = current_state(graph, dependent)?;
                let support_verdict = support.has_alternative_support(graph, target, dependent)?;
                let outcome = match support_verdict {
                    AlternativeSupportVerdict::Proven => {
                        ReconciliationOutcome::PreservedByAlternativeSupport
                    }
                    AlternativeSupportVerdict::Absent if depth == 1 => {
                        ReconciliationOutcome::MarkStale
                    }
                    AlternativeSupportVerdict::Absent | AlternativeSupportVerdict::Unknown => {
                        ReconciliationOutcome::MarkUnderReview
                    }
                };
                entries.push(ReconciliationEntry {
                    knowledge: dependent,
                    depth,
                    expected_version,
                    previous_lifecycle,
                    next_lifecycle: outcome.lifecycle(previous_lifecycle),
                    outcome,
                });
            }
        }

        Ok(ReconciliationPlan { changed, entries })
    }
}

fn current_state(graph: &KnowledgeStore, knowledge: KnowledgeRef) -> Result<(Lifecycle, u32)> {
    match knowledge {
        KnowledgeRef::Concept(id) => {
            let version = graph.current_concept_version(id)?;
            graph
                .get_concept(id)?
                .map(|concept| (concept.lifecycle, version))
                .ok_or_else(|| AdaptError::NotFound(format!("concept {id}")))
        }
        KnowledgeRef::Procedure(id) => graph
            .get_procedure(id)?
            .map(|procedure| (procedure.lifecycle, procedure.version))
            .ok_or_else(|| AdaptError::NotFound(format!("procedure {id}"))),
        KnowledgeRef::Relationship(id) => {
            let version = graph.current_relationship_version(id)?;
            graph
                .get_relationship(id)?
                .map(|relationship| (relationship.lifecycle, version))
                .ok_or_else(|| AdaptError::NotFound(format!("relationship {id}")))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationApplyResult {
    pub updated: Vec<KnowledgeRef>,
    pub preserved: Vec<KnowledgeRef>,
    pub receipt: Option<LifecycleChangeReceipt>,
}

pub struct ReconciliationApplier;

impl ReconciliationApplier {
    pub fn apply(
        graph: &KnowledgeStore,
        staged: &StagedReconciliation,
    ) -> Result<ReconciliationApplyResult> {
        let mut preserved = Vec::new();
        for entry in &staged.plan.entries {
            if entry.outcome == ReconciliationOutcome::PreservedByAlternativeSupport {
                preserved.push(entry.knowledge);
                continue;
            }
            if entry.previous_lifecycle == entry.next_lifecycle {
                preserved.push(entry.knowledge);
                continue;
            }
        }

        let receipt = graph.apply_lifecycle_change_set(&staged.change_set())?;
        let updated = receipt
            .changes
            .iter()
            .map(|change| match change.target {
                spoon_graph::LifecycleTarget::Concept { id } => KnowledgeRef::Concept(id),
                spoon_graph::LifecycleTarget::Procedure { id } => KnowledgeRef::Procedure(id),
                spoon_graph::LifecycleTarget::Relationship { id } => KnowledgeRef::Relationship(id),
            })
            .collect();
        Ok(ReconciliationApplyResult {
            updated,
            preserved,
            receipt: Some(receipt),
        })
    }
}
