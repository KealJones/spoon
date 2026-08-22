use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use ekg_core::{
    AssembledContext, Assumption, Concept, ConceptId, EkgError, Episode, EpisodeId, Interpretation,
    Lifecycle, RelationshipId, Source, Value,
};
use ekg_episode::{EpisodeQuery, EpisodeStore};
use ekg_graph::{GraphError, KnowledgeStore};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{InterpretationCandidate, InterpretationSet};

pub use ekg_core::{
    ContextBudget as RemainingBudget, ContextEpisode as RecentEpisode,
    ContextProcedure as RelevantProcedure, ContextRelationship as RelevantRelationship,
};

/// Absolute ceiling for any caller-configurable collection bound.
pub const MAX_CONTEXT_COLLECTION_ITEMS: usize = 1_024;
/// Absolute ceiling for any caller-configurable string bound.
pub const MAX_CONTEXT_TEXT_CHARS: usize = 65_536;
/// Absolute ceiling for graph traversal depth.
pub const MAX_CONTEXT_GRAPH_HOPS: u32 = 16;
/// Absolute ceiling for nested environment/result values.
pub const MAX_CONTEXT_VALUE_DEPTH: usize = 64;

/// Hard caps applied while building active working context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextLimits {
    pub max_goal_chars: usize,
    pub max_entities: usize,
    pub max_relationships: usize,
    pub max_relevant_procedures: usize,
    pub max_recent_episodes: usize,
    pub max_recent_text_chars: usize,
    pub max_assumptions: usize,
    pub max_assumption_chars: usize,
    pub max_environment_entries: usize,
    pub max_environment_key_chars: usize,
    pub max_environment_value_chars: usize,
    pub max_embedded_items: usize,
    pub max_value_depth: usize,
    pub graph_hops: u32,
}

impl Default for ContextLimits {
    fn default() -> Self {
        Self {
            max_goal_chars: 2_048,
            max_entities: 32,
            max_relationships: 128,
            max_relevant_procedures: 32,
            max_recent_episodes: 20,
            max_recent_text_chars: 2_048,
            max_assumptions: 32,
            max_assumption_chars: 1_024,
            max_environment_entries: 64,
            max_environment_key_chars: 128,
            max_environment_value_chars: 2_048,
            max_embedded_items: 64,
            max_value_depth: 16,
            graph_hops: 2,
        }
    }
}

/// Selection policy for context assembly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextConfig {
    pub relationship_kinds: Vec<String>,
    pub limits: ContextLimits,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            relationship_kinds: vec![
                "has".into(),
                "implemented-by".into(),
                "inverse-of".into(),
                "is-a".into(),
                "operates-on".into(),
                "requires".into(),
                "special-case-of".into(),
                "supports".into(),
                "tested-by".into(),
            ],
            limits: ContextLimits::default(),
        }
    }
}

/// Inputs that cannot be recovered from the graph or episode history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRequest {
    pub goal: Option<String>,
    pub goal_reason: Option<String>,
    pub interpretation: InterpretationSet,
    pub entities: Vec<ConceptId>,
    pub assumptions: Vec<Assumption>,
    pub environment: BTreeMap<String, Value>,
    pub budget_remaining: RemainingBudget,
}

/// Rich working context used by reasoning and persisted losslessly in episodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeContext {
    pub goal: Option<String>,
    pub goal_reason: Option<String>,
    pub interpretations: Vec<InterpretationCandidate>,
    pub chosen_interpretation: Option<ConceptId>,
    pub entities: Vec<ConceptId>,
    pub relevant_knowledge: Vec<RelevantRelationship>,
    pub relevant_procedures: Vec<RelevantProcedure>,
    pub recent_episodes: Vec<RecentEpisode>,
    pub assumptions: Vec<Assumption>,
    pub environment: BTreeMap<String, Value>,
    pub budget_remaining: RemainingBudget,
}

impl KnowledgeContext {
    pub fn to_episode_context(&self) -> AssembledContext {
        AssembledContext {
            goal: self.goal.clone(),
            goal_reason: self.goal_reason.clone(),
            interpretations: self
                .interpretations
                .iter()
                .map(|candidate| Interpretation {
                    meaning: candidate.meaning,
                    weight: candidate.weight,
                    chosen: self.chosen_interpretation == Some(candidate.meaning),
                })
                .collect(),
            entities: self.entities.clone(),
            relevant_knowledge: self.relevant_knowledge.clone(),
            relevant_procedures: self.relevant_procedures.clone(),
            recent_episodes: self.recent_episodes.clone(),
            assumptions: self.assumptions.clone(),
            environment: self.environment.clone(),
            budget_remaining: Some(self.budget_remaining),
        }
    }
}

/// Deterministic heuristic context selector backed by the live stores.
pub struct ContextAssembler<'a> {
    graph: &'a KnowledgeStore,
    episodes: &'a EpisodeStore,
    config: ContextConfig,
}

impl<'a> ContextAssembler<'a> {
    pub fn new(
        graph: &'a KnowledgeStore,
        episodes: &'a EpisodeStore,
        mut config: ContextConfig,
    ) -> Result<Self, ContextError> {
        validate_limits(&config.limits)?;
        if config.relationship_kinds.len() > MAX_CONTEXT_COLLECTION_ITEMS {
            return Err(ContextError::InputExceedsHardMaximum {
                name: "relationship_kinds",
                count: config.relationship_kinds.len(),
                maximum: MAX_CONTEXT_COLLECTION_ITEMS,
            });
        }
        if config
            .relationship_kinds
            .iter()
            .any(|kind| kind.chars().count() > MAX_CONTEXT_TEXT_CHARS)
        {
            return Err(ContextError::TextExceedsHardMaximum {
                name: "relationship_kind",
                maximum: MAX_CONTEXT_TEXT_CHARS,
            });
        }
        config
            .relationship_kinds
            .retain(|kind| !kind.trim().is_empty());
        config.relationship_kinds.sort();
        config.relationship_kinds.dedup();
        Ok(Self {
            graph,
            episodes,
            config,
        })
    }

    pub fn config(&self) -> &ContextConfig {
        &self.config
    }

    pub fn assemble(&self, request: &ContextRequest) -> Result<KnowledgeContext, ContextError> {
        validate_request(request)?;
        self.validate_concepts_exist(request)?;

        let entities = self.select_entities(request);
        let relevant_knowledge = self.select_graph_neighborhood(&entities, request)?;
        let relevant_procedures = self.select_relevant_procedures(&entities, request)?;
        let recent_episodes = self.select_recent_episodes(&entities)?;
        let assumptions = request
            .assumptions
            .iter()
            .take(self.config.limits.max_assumptions)
            .map(|assumption| Assumption {
                description: truncate(
                    &assumption.description,
                    self.config.limits.max_assumption_chars,
                ),
                basis: truncate(&assumption.basis, self.config.limits.max_assumption_chars),
                concept: assumption.concept,
            })
            .collect();
        let environment = request
            .environment
            .iter()
            .take(self.config.limits.max_environment_entries)
            .map(|(key, value)| {
                (
                    truncate(key, self.config.limits.max_environment_key_chars),
                    truncate_value(
                        value,
                        self.config.limits.max_environment_value_chars,
                        self.config.limits.max_embedded_items,
                        self.config.limits.max_value_depth,
                    ),
                )
            })
            .collect();

        Ok(KnowledgeContext {
            goal: request
                .goal
                .as_deref()
                .map(|goal| truncate(goal, self.config.limits.max_goal_chars)),
            goal_reason: request
                .goal_reason
                .as_deref()
                .map(|reason| truncate(reason, self.config.limits.max_goal_chars)),
            interpretations: request.interpretation.candidates().to_vec(),
            chosen_interpretation: request.interpretation.chosen(),
            entities,
            relevant_knowledge,
            relevant_procedures,
            recent_episodes,
            assumptions,
            environment,
            budget_remaining: request.budget_remaining,
        })
    }

    fn validate_concepts_exist(&self, request: &ContextRequest) -> Result<(), ContextError> {
        let mut seen = HashSet::new();
        for concept in request
            .interpretation
            .candidates()
            .iter()
            .map(|candidate| candidate.meaning)
            .chain(request.entities.iter().copied())
        {
            if seen.insert(concept) && self.graph.get_concept(concept)?.is_none() {
                return Err(ContextError::MissingConcept(concept));
            }
        }
        Ok(())
    }

    fn select_entities(&self, request: &ContextRequest) -> Vec<ConceptId> {
        let mut candidates = request.interpretation.candidates().to_vec();
        candidates.sort_by(|left, right| {
            let left_chosen = request.interpretation.chosen() == Some(left.meaning);
            let right_chosen = request.interpretation.chosen() == Some(right.meaning);
            right_chosen
                .cmp(&left_chosen)
                .then_with(|| {
                    right
                        .weight
                        .partial_cmp(&left.weight)
                        .unwrap_or(Ordering::Equal)
                })
                .then_with(|| left.meaning.0.cmp(&right.meaning.0))
        });

        let mut explicit = request.entities.clone();
        explicit.sort_by_key(|concept| concept.0);
        let mut seen = HashSet::new();
        candidates
            .into_iter()
            .map(|candidate| candidate.meaning)
            .chain(explicit)
            .filter(|concept| seen.insert(*concept))
            .take(self.config.limits.max_entities)
            .collect()
    }

    fn select_graph_neighborhood(
        &self,
        entities: &[ConceptId],
        request: &ContextRequest,
    ) -> Result<Vec<RelevantRelationship>, ContextError> {
        let limits = &self.config.limits;
        if limits.max_relationships == 0
            || limits.graph_hops == 0
            || self.config.relationship_kinds.is_empty()
        {
            return Ok(Vec::new());
        }

        let allowed = self
            .config
            .relationship_kinds
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut seed_relevance = request
            .interpretation
            .candidates()
            .iter()
            .map(|candidate| (candidate.meaning, candidate.weight))
            .collect::<HashMap<_, _>>();
        for entity in &request.entities {
            seed_relevance
                .entry(*entity)
                .and_modify(|score| *score = score.max(0.5))
                .or_insert(0.5);
        }
        if let Some(chosen) = request.interpretation.chosen() {
            seed_relevance.insert(chosen, 1.0);
        }

        let mut frontier = entities
            .iter()
            .map(|concept| {
                (
                    *concept,
                    0_u32,
                    seed_relevance.get(concept).copied().unwrap_or(0.5),
                )
            })
            .collect::<VecDeque<_>>();
        let mut visited_concepts = entities.iter().copied().collect::<HashSet<_>>();
        let mut visited_relationships = HashSet::<RelationshipId>::new();
        let mut selected = Vec::new();

        while let Some((current, prior_hops, prior_relevance)) = frontier.pop_front() {
            if prior_hops >= limits.graph_hops {
                continue;
            }
            let mut relationships = self.graph.get_relationships_from(current)?;
            relationships.extend(self.graph.get_relationships_to(current)?);
            relationships.retain(|relationship| {
                allowed.contains(relationship.kind.as_str())
                    && is_active_lifecycle(relationship.lifecycle)
            });
            relationships.sort_by(|left, right| {
                right
                    .strength
                    .partial_cmp(&left.strength)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| left.kind.cmp(&right.kind))
                    .then_with(|| left.source.0.cmp(&right.source.0))
                    .then_with(|| left.target.0.cmp(&right.target.0))
                    .then_with(|| left.id.0.cmp(&right.id.0))
            });

            for relationship in relationships {
                if !visited_relationships.insert(relationship.id) {
                    continue;
                }
                let adjacent = if relationship.source == current {
                    relationship.target
                } else {
                    relationship.source
                };
                let Some(adjacent_concept) = self.graph.get_concept(adjacent)? else {
                    continue;
                };
                if !is_active_lifecycle(adjacent_concept.lifecycle) {
                    continue;
                }
                let hops = prior_hops + 1;
                let strength = if relationship.strength.is_finite() {
                    relationship.strength.max(0.0)
                } else {
                    0.0
                };
                let relevance_score = prior_relevance * strength / f64::from(hops);
                selected.push(RelevantRelationship {
                    relationship: bound_relationship(relationship, limits),
                    discovered_from: current,
                    adjacent_concept: bound_concept(adjacent_concept, limits),
                    hops,
                    relevance_score,
                });
                if selected.len() == limits.max_relationships {
                    return Ok(selected);
                }
                if hops < limits.graph_hops && visited_concepts.insert(adjacent) {
                    frontier.push_back((adjacent, hops, relevance_score));
                }
            }
        }

        Ok(selected)
    }

    fn select_relevant_procedures(
        &self,
        entities: &[ConceptId],
        request: &ContextRequest,
    ) -> Result<Vec<RelevantProcedure>, ContextError> {
        let limits = &self.config.limits;
        if limits.max_relevant_procedures == 0 {
            return Ok(Vec::new());
        }

        let entity_relevance = entities
            .iter()
            .enumerate()
            .map(|(index, concept)| {
                let candidate_weight = request
                    .interpretation
                    .candidates()
                    .iter()
                    .find(|candidate| candidate.meaning == *concept)
                    .map_or(0.5, |candidate| candidate.weight);
                let chosen_bonus = if request.interpretation.chosen() == Some(*concept) {
                    1.0
                } else {
                    candidate_weight
                };
                (*concept, chosen_bonus / (index + 1) as f64)
            })
            .collect::<HashMap<_, _>>();
        let normalized_goal = request.goal.as_deref().unwrap_or_default().to_lowercase();

        let mut procedures = self
            .graph
            .list_procedures()?
            .into_iter()
            .filter(|procedure| is_active_lifecycle(procedure.lifecycle))
            .filter_map(|procedure| {
                let linked_score = procedure
                    .concept
                    .and_then(|concept| entity_relevance.get(&concept).copied());
                let mentioned_score = (!procedure.name.is_empty()
                    && normalized_goal.contains(&procedure.name.to_lowercase()))
                .then_some(0.75);
                let relevance_score = linked_score
                    .into_iter()
                    .chain(mentioned_score)
                    .fold(0.0_f64, f64::max);
                (relevance_score > 0.0).then(|| RelevantProcedure {
                    id: procedure.id,
                    name: truncate(&procedure.name, limits.max_recent_text_chars),
                    params: procedure
                        .params
                        .iter()
                        .take(limits.max_embedded_items)
                        .map(|param| truncate(&param.name, limits.max_recent_text_chars))
                        .collect(),
                    concept: procedure.concept,
                    version: procedure.version,
                    lifecycle: procedure.lifecycle,
                    relevance_score,
                })
            })
            .collect::<Vec<_>>();
        procedures.sort_by(|left, right| {
            right
                .relevance_score
                .partial_cmp(&left.relevance_score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.0.cmp(&right.id.0))
        });
        procedures.truncate(limits.max_relevant_procedures);
        Ok(procedures)
    }

    fn select_recent_episodes(
        &self,
        entities: &[ConceptId],
    ) -> Result<Vec<RecentEpisode>, ContextError> {
        let limits = &self.config.limits;
        if limits.max_recent_episodes == 0 {
            return Ok(Vec::new());
        }
        let query_limit = u32::try_from(limits.max_recent_episodes).unwrap_or(u32::MAX);
        let mut ranked = HashMap::<EpisodeId, (usize, Episode)>::new();
        for (entity_rank, concept) in entities.iter().enumerate() {
            let episodes = self
                .episodes
                .query(&EpisodeQuery {
                    concept: Some(*concept),
                    limit: query_limit,
                    ..EpisodeQuery::default()
                })
                .map_err(ContextError::EpisodeStore)?;
            for episode in episodes {
                ranked
                    .entry(episode.id)
                    .and_modify(|(rank, _)| *rank = (*rank).min(entity_rank))
                    .or_insert((entity_rank, episode));
            }
        }
        for episode in self
            .episodes
            .list_recent(query_limit)
            .map_err(ContextError::EpisodeStore)?
        {
            ranked
                .entry(episode.id)
                .or_insert((entities.len(), episode));
        }
        let mut episodes = ranked.into_values().collect::<Vec<_>>();
        episodes.sort_by(|(left_rank, left), (right_rank, right)| {
            left_rank.cmp(right_rank).then_with(|| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| left.id.0.cmp(&right.id.0))
            })
        });

        Ok(episodes
            .into_iter()
            .take(limits.max_recent_episodes)
            .map(|(_, episode)| RecentEpisode {
                episode_id: episode.id,
                situation: truncate(&episode.situation, limits.max_recent_text_chars),
                action: episode
                    .action
                    .as_deref()
                    .map(|action| truncate(action, limits.max_recent_text_chars)),
                observed_result: episode.observed_result.as_ref().map(|value| {
                    truncate_value(
                        value,
                        limits.max_environment_value_chars,
                        limits.max_embedded_items,
                        limits.max_value_depth,
                    )
                }),
                succeeded: episode
                    .evaluation
                    .as_ref()
                    .map(|evaluation| evaluation.success),
                created_at: episode.created_at,
            })
            .collect())
    }
}

fn validate_limits(limits: &ContextLimits) -> Result<(), ContextError> {
    for (name, value) in [
        ("max_goal_chars", limits.max_goal_chars),
        ("max_entities", limits.max_entities),
        ("max_recent_text_chars", limits.max_recent_text_chars),
        ("max_assumption_chars", limits.max_assumption_chars),
        (
            "max_environment_key_chars",
            limits.max_environment_key_chars,
        ),
        (
            "max_environment_value_chars",
            limits.max_environment_value_chars,
        ),
        ("max_embedded_items", limits.max_embedded_items),
        ("max_value_depth", limits.max_value_depth),
    ] {
        if value == 0 {
            return Err(ContextError::InvalidLimit(name));
        }
    }
    for (name, value) in [
        ("max_entities", limits.max_entities),
        ("max_relationships", limits.max_relationships),
        ("max_relevant_procedures", limits.max_relevant_procedures),
        ("max_recent_episodes", limits.max_recent_episodes),
        ("max_assumptions", limits.max_assumptions),
        ("max_environment_entries", limits.max_environment_entries),
        ("max_embedded_items", limits.max_embedded_items),
    ] {
        if value > MAX_CONTEXT_COLLECTION_ITEMS {
            return Err(ContextError::LimitExceedsHardMaximum {
                name,
                value,
                maximum: MAX_CONTEXT_COLLECTION_ITEMS,
            });
        }
    }
    for (name, value) in [
        ("max_goal_chars", limits.max_goal_chars),
        ("max_recent_text_chars", limits.max_recent_text_chars),
        ("max_assumption_chars", limits.max_assumption_chars),
        (
            "max_environment_key_chars",
            limits.max_environment_key_chars,
        ),
        (
            "max_environment_value_chars",
            limits.max_environment_value_chars,
        ),
    ] {
        if value > MAX_CONTEXT_TEXT_CHARS {
            return Err(ContextError::LimitExceedsHardMaximum {
                name,
                value,
                maximum: MAX_CONTEXT_TEXT_CHARS,
            });
        }
    }
    if limits.max_value_depth > MAX_CONTEXT_VALUE_DEPTH {
        return Err(ContextError::LimitExceedsHardMaximum {
            name: "max_value_depth",
            value: limits.max_value_depth,
            maximum: MAX_CONTEXT_VALUE_DEPTH,
        });
    }
    if limits.graph_hops > MAX_CONTEXT_GRAPH_HOPS {
        return Err(ContextError::GraphHopsExceedHardMaximum {
            value: limits.graph_hops,
            maximum: MAX_CONTEXT_GRAPH_HOPS,
        });
    }
    Ok(())
}

fn validate_request(request: &ContextRequest) -> Result<(), ContextError> {
    if !request.budget_remaining.cost.is_finite() || request.budget_remaining.cost < 0.0 {
        return Err(ContextError::InvalidBudgetCost(
            request.budget_remaining.cost,
        ));
    }
    for (name, count) in [
        ("entities", request.entities.len()),
        ("assumptions", request.assumptions.len()),
        ("environment", request.environment.len()),
    ] {
        if count > MAX_CONTEXT_COLLECTION_ITEMS {
            return Err(ContextError::InputExceedsHardMaximum {
                name,
                count,
                maximum: MAX_CONTEXT_COLLECTION_ITEMS,
            });
        }
    }
    for (index, assumption) in request.assumptions.iter().enumerate() {
        if assumption.basis.trim().is_empty() {
            return Err(ContextError::UnmarkedAssumption { index });
        }
    }
    Ok(())
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn truncate_value(value: &Value, max_chars: usize, max_items: usize, max_depth: usize) -> Value {
    match value {
        Value::Text(text) => Value::Text(truncate(text, max_chars)),
        Value::List(_) | Value::Map(_) if max_depth == 0 => match value {
            Value::List(_) => Value::List(Vec::new()),
            Value::Map(_) => Value::Map(BTreeMap::new()),
            _ => unreachable!(),
        },
        Value::List(items) => Value::List(
            items
                .iter()
                .take(max_items)
                .map(|item| truncate_value(item, max_chars, max_items, max_depth - 1))
                .collect(),
        ),
        Value::Map(entries) => Value::Map(
            entries
                .iter()
                .take(max_items)
                .map(|(key, value)| {
                    (
                        truncate(key, max_chars),
                        truncate_value(value, max_chars, max_items, max_depth - 1),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn is_active_lifecycle(lifecycle: Lifecycle) -> bool {
    matches!(
        lifecycle,
        Lifecycle::Active | Lifecycle::Validated | Lifecycle::Provisional | Lifecycle::UnderReview
    )
}

fn bound_relationship(
    mut relationship: ekg_core::Relationship,
    limits: &ContextLimits,
) -> ekg_core::Relationship {
    relationship.kind = truncate(&relationship.kind, limits.max_recent_text_chars);
    relationship.scope = relationship
        .scope
        .into_iter()
        .take(limits.max_embedded_items)
        .map(|mut condition| {
            condition.description = truncate(&condition.description, limits.max_recent_text_chars);
            condition
        })
        .collect();
    relationship.evidence.truncate(limits.max_embedded_items);
    relationship
}

fn bound_concept(mut concept: Concept, limits: &ContextLimits) -> Concept {
    concept.name = truncate(&concept.name, limits.max_recent_text_chars);
    concept.description = concept
        .description
        .as_deref()
        .map(|description| truncate(description, limits.max_recent_text_chars));
    concept.confidence.scope = concept
        .confidence
        .scope
        .into_iter()
        .take(limits.max_embedded_items)
        .map(|mut condition| {
            condition.description = truncate(&condition.description, limits.max_recent_text_chars);
            condition
        })
        .collect();
    concept.confidence.sources = concept
        .confidence
        .sources
        .into_iter()
        .take(limits.max_embedded_items)
        .map(|source| Source {
            kind: source.kind,
            id: truncate(&source.id, limits.max_recent_text_chars),
            reliability: source.reliability,
        })
        .collect();
    concept
}

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("context limit {0} must be greater than zero")]
    InvalidLimit(&'static str),
    #[error("context limit {name}={value} exceeds hard maximum {maximum}")]
    LimitExceedsHardMaximum {
        name: &'static str,
        value: usize,
        maximum: usize,
    },
    #[error("graph_hops={value} exceeds hard maximum {maximum}")]
    GraphHopsExceedHardMaximum { value: u32, maximum: u32 },
    #[error("context input {name} has {count} items, above hard maximum {maximum}")]
    InputExceedsHardMaximum {
        name: &'static str,
        count: usize,
        maximum: usize,
    },
    #[error("context input {name} exceeds hard text maximum {maximum}")]
    TextExceedsHardMaximum { name: &'static str, maximum: usize },
    #[error("assumption at index {index} has no marked basis")]
    UnmarkedAssumption { index: usize },
    #[error("remaining budget cost must be finite and nonnegative, got {0}")]
    InvalidBudgetCost(f64),
    #[error("active concept {0} does not exist in the knowledge graph")]
    MissingConcept(ConceptId),
    #[error("graph lookup failed: {0}")]
    Graph(#[from] GraphError),
    #[error("episode lookup failed: {0}")]
    EpisodeStore(EkgError),
}
