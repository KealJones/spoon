use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};

use rusqlite::params;
use serde::{Deserialize, Serialize};
use spoon_core::{ConceptId, Lifecycle, RelationshipId};
use uuid::Uuid;

use crate::{GraphError, KnowledgeStore, Result};

pub const MAX_ACTIVATION_SEEDS: usize = 64;
pub const MAX_ACTIVATION_TRAVERSALS: usize = 32;
pub const MAX_ACTIVATION_HOPS: u32 = 16;
pub const MAX_ACTIVATION_CANDIDATES: usize = 1_024;
pub const MAX_ACTIVATION_EXPANSIONS: usize = 8_192;
const MAX_RELATIONSHIP_KIND_CHARS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ActivationSeed {
    pub concept: ConceptId,
    pub activation: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraversalDirection {
    Outgoing,
    Incoming,
    Both,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedRelationshipTraversal {
    pub kind: String,
    pub direction: TraversalDirection,
    pub decay: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivationSpreadQuery {
    pub seeds: Vec<ActivationSeed>,
    pub traversals: Vec<TypedRelationshipTraversal>,
    pub max_hops: u32,
    pub max_candidates: usize,
    pub max_expansions: usize,
    pub min_activation: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipDirection {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivationHop {
    pub relationship: RelationshipId,
    pub kind: String,
    pub direction: RelationshipDirection,
    pub from: ConceptId,
    pub to: ConceptId,
    pub strength: f64,
    pub contribution: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivatedConcept {
    pub concept: ConceptId,
    pub activation: f64,
    pub min_hops: u32,
    pub strongest_path: Vec<ActivationHop>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivationSpreadResult {
    pub candidates: Vec<ActivatedConcept>,
    pub expansions: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
struct FrontierItem {
    concept: ConceptId,
    activation: f64,
    hops: u32,
    path: Vec<ActivationHop>,
    path_concepts: HashSet<ConceptId>,
}

#[derive(Debug, Clone)]
struct CandidateState {
    activation: f64,
    min_hops: u32,
    strongest_contribution: f64,
    strongest_path: Vec<ActivationHop>,
}

#[derive(Debug, Clone)]
struct Edge {
    relationship: RelationshipId,
    from: ConceptId,
    to: ConceptId,
    strength: f64,
    direction: RelationshipDirection,
}

impl KnowledgeStore {
    /// Spreads independent relevance activation over explicitly allowed typed
    /// relationships. Every store read and returned collection is hard bounded.
    pub fn activation_spread(
        &self,
        query: &ActivationSpreadQuery,
    ) -> Result<ActivationSpreadResult> {
        validate_query(query)?;
        if query.max_hops == 0
            || query.max_expansions == 0
            || query.seeds.is_empty()
            || query.traversals.is_empty()
        {
            return Ok(ActivationSpreadResult {
                candidates: Vec::new(),
                expansions: 0,
                truncated: query.max_expansions == 0
                    && !query.seeds.is_empty()
                    && !query.traversals.is_empty()
                    && query.max_hops > 0,
            });
        }

        let traversals = canonical_traversals(&query.traversals);
        let seed_ids = query
            .seeds
            .iter()
            .map(|seed| seed.concept)
            .collect::<HashSet<_>>();
        let mut frontier = VecDeque::new();
        for seed in canonical_seeds(&query.seeds) {
            let concept = self
                .get_concept(seed.concept)?
                .ok_or_else(|| GraphError::NotFound(format!("concept {}", seed.concept)))?;
            if !activation_lifecycle(concept.lifecycle) {
                continue;
            }
            frontier.push_back(FrontierItem {
                concept: seed.concept,
                activation: seed.activation,
                hops: 0,
                path: Vec::new(),
                path_concepts: HashSet::from([seed.concept]),
            });
        }

        let mut candidates = HashMap::<ConceptId, CandidateState>::new();
        let mut expansions = 0_usize;
        while let Some(item) = frontier.pop_front() {
            if item.hops >= query.max_hops || expansions >= query.max_expansions {
                continue;
            }
            for traversal in &traversals {
                if expansions >= query.max_expansions {
                    break;
                }
                let remaining = query.max_expansions - expansions;
                let edges = self.activation_edges(item.concept, traversal, remaining)?;
                for edge in edges {
                    expansions += 1;
                    if item.path_concepts.contains(&edge.to) {
                        continue;
                    }
                    let contribution =
                        item.activation * traversal.decay * edge.strength.clamp(0.0, 1.0);
                    if !contribution.is_finite()
                        || contribution <= 0.0
                        || contribution < query.min_activation
                    {
                        continue;
                    }
                    let next_hops = item.hops + 1;
                    let mut path = item.path.clone();
                    path.push(ActivationHop {
                        relationship: edge.relationship,
                        kind: traversal.kind.clone(),
                        direction: edge.direction,
                        from: edge.from,
                        to: edge.to,
                        strength: edge.strength,
                        contribution,
                    });
                    if !seed_ids.contains(&edge.to) {
                        candidates
                            .entry(edge.to)
                            .and_modify(|state| {
                                state.activation =
                                    combine_activation(state.activation, contribution);
                                state.min_hops = state.min_hops.min(next_hops);
                                if stronger_path(
                                    contribution,
                                    &path,
                                    state.strongest_contribution,
                                    &state.strongest_path,
                                ) {
                                    state.strongest_contribution = contribution;
                                    state.strongest_path = path.clone();
                                }
                            })
                            .or_insert(CandidateState {
                                activation: contribution,
                                min_hops: next_hops,
                                strongest_contribution: contribution,
                                strongest_path: path.clone(),
                            });
                    }
                    if next_hops < query.max_hops {
                        let mut path_concepts = item.path_concepts.clone();
                        path_concepts.insert(edge.to);
                        frontier.push_back(FrontierItem {
                            concept: edge.to,
                            activation: contribution,
                            hops: next_hops,
                            path,
                            path_concepts,
                        });
                    }
                    if expansions >= query.max_expansions {
                        break;
                    }
                }
            }
        }

        let candidate_count = candidates.len();
        let mut candidates = candidates
            .into_iter()
            .map(|(concept, state)| ActivatedConcept {
                concept,
                activation: state.activation,
                min_hops: state.min_hops,
                strongest_path: state.strongest_path,
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .activation
                .partial_cmp(&left.activation)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.min_hops.cmp(&right.min_hops))
                .then_with(|| left.concept.0.cmp(&right.concept.0))
        });
        candidates.truncate(query.max_candidates);
        // Reaching the read budget is conservatively reported as truncation;
        // the bounded query deliberately does not issue another read to prove
        // whether an additional edge exists.
        let expansion_truncated = expansions >= query.max_expansions;
        Ok(ActivationSpreadResult {
            candidates,
            expansions,
            truncated: expansion_truncated || candidate_count > query.max_candidates,
        })
    }

    fn activation_edges(
        &self,
        current: ConceptId,
        traversal: &TypedRelationshipTraversal,
        limit: usize,
    ) -> Result<Vec<Edge>> {
        let active = serde_json::to_string(&Lifecycle::Active)?;
        let validated = serde_json::to_string(&Lifecycle::Validated)?;
        let provisional = serde_json::to_string(&Lifecycle::Provisional)?;
        let under_review = serde_json::to_string(&Lifecycle::UnderReview)?;
        let include_outgoing = i64::from(matches!(
            traversal.direction,
            TraversalDirection::Outgoing | TraversalDirection::Both
        ));
        let include_incoming = i64::from(matches!(
            traversal.direction,
            TraversalDirection::Incoming | TraversalDirection::Both
        ));
        let mut statement = self.conn.prepare(
            "SELECT r.id, r.source, r.target, r.strength, \
                    CASE WHEN ?3 = 1 AND r.source = ?1 THEN 0 ELSE 1 END AS direction \
             FROM relationships r \
             JOIN concepts source_concept ON source_concept.id = r.source \
             JOIN concepts target_concept ON target_concept.id = r.target \
             WHERE r.kind = ?2 \
               AND ((?3 = 1 AND r.source = ?1) OR (?4 = 1 AND r.target = ?1)) \
               AND r.lifecycle IN (?5, ?6, ?7, ?8) \
               AND source_concept.lifecycle IN (?5, ?6, ?7, ?8) \
               AND target_concept.lifecycle IN (?5, ?6, ?7, ?8) \
             ORDER BY r.strength DESC, r.id ASC LIMIT ?9",
        )?;
        let rows = statement.query_map(
            params![
                current.0.to_string(),
                traversal.kind,
                include_outgoing,
                include_incoming,
                active,
                validated,
                provisional,
                under_review,
                i64::try_from(limit).unwrap_or(i64::MAX),
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        let mut edges = Vec::new();
        for row in rows {
            let (relationship, source, target, strength, direction) = row?;
            if !strength.is_finite() || strength <= 0.0 {
                continue;
            }
            let source = ConceptId(Uuid::parse_str(&source)?);
            let target = ConceptId(Uuid::parse_str(&target)?);
            let (from, to, direction) = if direction == 0 {
                (source, target, RelationshipDirection::Outgoing)
            } else {
                (target, source, RelationshipDirection::Incoming)
            };
            edges.push(Edge {
                relationship: RelationshipId(Uuid::parse_str(&relationship)?),
                from,
                to,
                strength,
                direction,
            });
        }
        Ok(edges)
    }
}

fn validate_query(query: &ActivationSpreadQuery) -> Result<()> {
    if query.seeds.len() > MAX_ACTIVATION_SEEDS {
        return invalid("too many activation seeds");
    }
    if query.traversals.len() > MAX_ACTIVATION_TRAVERSALS {
        return invalid("too many typed relationship traversals");
    }
    if query.max_hops > MAX_ACTIVATION_HOPS {
        return invalid("max_hops exceeds the hard activation bound");
    }
    if query.max_candidates > MAX_ACTIVATION_CANDIDATES {
        return invalid("max_candidates exceeds the hard activation bound");
    }
    if query.max_expansions > MAX_ACTIVATION_EXPANSIONS {
        return invalid("max_expansions exceeds the hard activation bound");
    }
    if !query.min_activation.is_finite() || !(0.0..=1.0).contains(&query.min_activation) {
        return invalid("min_activation must be finite and between zero and one");
    }
    for seed in &query.seeds {
        if !seed.activation.is_finite() || !(0.0..=1.0).contains(&seed.activation) {
            return invalid("seed activation must be finite and between zero and one");
        }
    }
    for traversal in &query.traversals {
        if traversal.kind.trim().is_empty()
            || traversal.kind.chars().count() > MAX_RELATIONSHIP_KIND_CHARS
        {
            return invalid("relationship kind must be nonempty and bounded");
        }
        if !traversal.decay.is_finite() || !(0.0..=1.0).contains(&traversal.decay) {
            return invalid("relationship decay must be finite and between zero and one");
        }
    }
    Ok(())
}

fn canonical_seeds(seeds: &[ActivationSeed]) -> Vec<ActivationSeed> {
    let mut combined = HashMap::<ConceptId, f64>::new();
    for seed in seeds {
        combined
            .entry(seed.concept)
            .and_modify(|activation| *activation = combine_activation(*activation, seed.activation))
            .or_insert(seed.activation);
    }
    let mut seeds = combined
        .into_iter()
        .map(|(concept, activation)| ActivationSeed {
            concept,
            activation,
        })
        .collect::<Vec<_>>();
    seeds.sort_by_key(|seed| seed.concept.0);
    seeds
}

fn canonical_traversals(
    traversals: &[TypedRelationshipTraversal],
) -> Vec<TypedRelationshipTraversal> {
    let mut combined = HashMap::<(String, TraversalDirection), f64>::new();
    for traversal in traversals {
        combined
            .entry((traversal.kind.clone(), traversal.direction))
            .and_modify(|decay| *decay = decay.max(traversal.decay))
            .or_insert(traversal.decay);
    }
    let mut traversals = combined
        .into_iter()
        .map(|((kind, direction), decay)| TypedRelationshipTraversal {
            kind,
            direction,
            decay,
        })
        .collect::<Vec<_>>();
    traversals.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.direction.cmp(&right.direction))
    });
    traversals
}

fn combine_activation(current: f64, contribution: f64) -> f64 {
    1.0 - (1.0 - current) * (1.0 - contribution)
}

fn stronger_path(
    contribution: f64,
    path: &[ActivationHop],
    current_contribution: f64,
    current_path: &[ActivationHop],
) -> bool {
    contribution > current_contribution
        || (contribution == current_contribution
            && path
                .iter()
                .map(|hop| hop.relationship.0)
                .cmp(current_path.iter().map(|hop| hop.relationship.0))
                == Ordering::Less)
}

fn activation_lifecycle(lifecycle: Lifecycle) -> bool {
    matches!(
        lifecycle,
        Lifecycle::Active | Lifecycle::Validated | Lifecycle::Provisional | Lifecycle::UnderReview
    )
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(GraphError::InvalidActivationQuery(message.into()))
}
