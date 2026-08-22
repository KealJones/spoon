use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use ekg_core::{Episode, EpisodeId, ProcedureId};
use ekg_episode::CreditAggregateSnapshot;
use ekg_exec::ExecTrace;
use serde::{Deserialize, Serialize};

use crate::types::{cost_ratio, original_execution_cost, total_with_attribution};
use crate::{
    Attribution, AttributionConfidence, AttributionEvidence, AttributionLimitation,
    AttributionMechanism, AttributionProvenance, CreditError, Suspect,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Element {
    procedure: ProcedureId,
    version: u32,
}

#[derive(Default)]
struct ElementStats {
    exposures: u32,
    failures: u32,
    weighted_exposure: f64,
    weighted_failures: f64,
    episodes: Vec<EpisodeId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatisticalCost {
    pub failed_trace_steps_scanned: u64,
    pub history_episodes_considered: u64,
    pub history_episodes_used: u64,
    pub history_trace_steps_scanned: u64,
    pub element_exposures_counted: u64,
    pub cooccurrence_pairs_counted: u64,
    /// Materialized per-element rows read by indexed attribution.
    #[serde(default)]
    pub aggregate_rows_read: u64,
    /// Materialized pair rows read by indexed attribution.
    #[serde(default)]
    pub pair_aggregate_rows_read: u64,
    pub work_units: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatisticalRankingReport {
    pub attributions: Vec<Attribution>,
    pub cost: StatisticalCost,
}

pub fn rank_statistical_suspects(
    failed_episode: &Episode,
    history: &[Episode],
) -> Result<Vec<Attribution>, CreditError> {
    Ok(rank_statistical_suspects_with_cost(failed_episode, history)?.attributions)
}

pub fn rank_statistical_suspects_with_cost(
    failed_episode: &Episode,
    history: &[Episode],
) -> Result<StatisticalRankingReport, CreditError> {
    let failed_trace = parse_trace(failed_episode)?;
    let mut cost = StatisticalCost {
        failed_trace_steps_scanned: failed_trace.steps.len() as u64,
        history_episodes_considered: history.len() as u64,
        ..StatisticalCost::default()
    };
    let mut current = Vec::new();
    let mut seen = HashSet::new();
    for (trace_step, step) in failed_trace.steps.iter().enumerate() {
        let procedure = step
            .procedure_called
            .ok_or(CreditError::MissingProcedure { step: trace_step })?;
        let version = step
            .procedure_version
            .ok_or(CreditError::MissingProcedureVersion { step: trace_step })?;
        let element = Element { procedure, version };
        if seen.insert(element) {
            current.push((element, trace_step));
        }
    }

    let current_elements = current
        .iter()
        .map(|(element, _)| *element)
        .collect::<HashSet<_>>();
    let mut stats = HashMap::<Element, ElementStats>::new();
    let mut cooccurrences = HashMap::<(Element, Element), u32>::new();
    let mut seen_episodes = HashSet::new();
    for episode in history {
        let Some(evaluation) = episode.evaluation.as_ref() else {
            continue;
        };
        if !seen_episodes.insert(episode.id) {
            continue;
        }
        let Some(trace_json) = episode.execution_trace.clone() else {
            continue;
        };
        let trace: ExecTrace =
            serde_json::from_value(trace_json).map_err(|source| CreditError::InvalidTrace {
                episode: episode.id,
                source,
            })?;
        cost.history_episodes_used = cost.history_episodes_used.saturating_add(1);
        cost.history_trace_steps_scanned = cost
            .history_trace_steps_scanned
            .saturating_add(trace.steps.len() as u64);
        let evidence_weight = tier_weight(evaluation.tier);
        let exposed = trace
            .steps
            .iter()
            .filter_map(|step| {
                Some(Element {
                    procedure: step.procedure_called?,
                    version: step.procedure_version?,
                })
            })
            .filter(|element| current_elements.contains(element))
            .collect::<HashSet<_>>();
        for element in &exposed {
            cost.element_exposures_counted = cost.element_exposures_counted.saturating_add(1);
            let entry = stats.entry(*element).or_default();
            entry.exposures = entry.exposures.saturating_add(1);
            entry.failures = entry.failures.saturating_add(u32::from(episode.failed()));
            entry.weighted_exposure += evidence_weight;
            if episode.failed() {
                entry.weighted_failures += evidence_weight;
            }
            entry.episodes.push(episode.id);
        }
        let mut exposed = exposed.into_iter().collect::<Vec<_>>();
        exposed.sort_by_key(|element| (element.procedure.0, element.version));
        for left in 0..exposed.len() {
            for right in (left + 1)..exposed.len() {
                cost.cooccurrence_pairs_counted = cost.cooccurrence_pairs_counted.saturating_add(1);
                *cooccurrences
                    .entry((exposed[left], exposed[right]))
                    .or_default() += 1;
            }
        }
    }

    let original_cost = original_execution_cost(
        failed_episode.cost.steps_taken,
        failed_episode.cost.budget_spent,
    );
    cost.work_units = cost
        .failed_trace_steps_scanned
        .saturating_add(cost.history_episodes_considered)
        .saturating_add(cost.history_trace_steps_scanned)
        .saturating_add(cost.element_exposures_counted)
        .saturating_add(cost.cooccurrence_pairs_counted);
    let attribution_cost = cost.work_units as f64;
    let total_cost = total_with_attribution(original_cost, attribution_cost);
    let ratio = cost_ratio(attribution_cost, total_cost);
    let mut ranked = current
        .into_iter()
        .map(|(element, trace_step)| {
            let element_stats = stats.get(&element);
            let exposures = element_stats.map_or(0, |value| value.exposures);
            let failures = element_stats.map_or(0, |value| value.failures);
            let cooccurrence =
                maximum_cooccurrence(element, &current_elements, &stats, &cooccurrences);
            let uncertainty = if exposures == 0 {
                1.0
            } else {
                (1.0 / f64::from(exposures).sqrt() + 0.5 * cooccurrence).min(1.0)
            };
            let weighted_exposure = element_stats.map_or(0.0, |value| value.weighted_exposure);
            let weighted_failures = element_stats.map_or(0.0, |value| value.weighted_failures);
            let failure_rate = if weighted_exposure <= 0.0 {
                0.0
            } else {
                weighted_failures / weighted_exposure
            };
            let support_factor = f64::from(exposures) / f64::from(exposures.saturating_add(2));
            let score = failure_rate * support_factor * (1.0 - 0.5 * cooccurrence);
            Attribution {
                suspect: Suspect {
                    procedure: element.procedure,
                    version: element.version,
                    trace_step,
                },
                mechanism: AttributionMechanism::StatisticalSuspicion,
                confidence: AttributionConfidence::Low,
                score,
                decisive: false,
                evidence: vec![AttributionEvidence::Statistics {
                    exposures,
                    failures,
                    support: exposures,
                    cooccurrence,
                    uncertainty,
                    weighted_exposure,
                    weighted_failures,
                }],
                limitations: if cooccurrence > 0.0 {
                    vec![
                        AttributionLimitation::CorrelationNotCausation,
                        AttributionLimitation::CorrelatedCandidates { cooccurrence },
                    ]
                } else {
                    vec![AttributionLimitation::CorrelationNotCausation]
                },
                provenance: AttributionProvenance {
                    episode_ids: element_stats
                        .map_or_else(Vec::new, |value| value.episodes.clone()),
                    details: vec![
                        "cross-episode association only; replay required before action".into(),
                    ],
                },
                attribution_cost,
                total_cost,
                attribution_cost_ratio: ratio,
            }
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.suspect.trace_step.cmp(&right.suspect.trace_step))
    });
    Ok(StatisticalRankingReport {
        attributions: ranked,
        cost,
    })
}

/// Ranks suspects from transactionally maintained sufficient statistics.
/// This is semantically equivalent to scanning the represented episodes but
/// its work is bounded by unique elements in the failed trace and their pairs.
pub fn rank_statistical_suspects_from_aggregates(
    failed_episode: &Episode,
    snapshot: &CreditAggregateSnapshot,
) -> Result<StatisticalRankingReport, CreditError> {
    let failed_trace = parse_trace(failed_episode)?;
    let mut cost = StatisticalCost {
        failed_trace_steps_scanned: failed_trace.steps.len() as u64,
        aggregate_rows_read: snapshot.elements.len() as u64,
        pair_aggregate_rows_read: snapshot.pairs.len() as u64,
        ..StatisticalCost::default()
    };
    let mut current = Vec::new();
    let mut seen = HashSet::new();
    for (trace_step, step) in failed_trace.steps.iter().enumerate() {
        let procedure = step
            .procedure_called
            .ok_or(CreditError::MissingProcedure { step: trace_step })?;
        let version = step
            .procedure_version
            .ok_or(CreditError::MissingProcedureVersion { step: trace_step })?;
        let element = Element { procedure, version };
        if seen.insert(element) {
            current.push((element, trace_step));
        }
    }
    let current_elements = current
        .iter()
        .map(|(element, _)| *element)
        .collect::<HashSet<_>>();
    let stats = snapshot
        .elements
        .iter()
        .map(|aggregate| {
            let element = Element {
                procedure: aggregate.element.procedure,
                version: aggregate.element.version,
            };
            (
                element,
                ElementStats {
                    exposures: aggregate.exposures,
                    failures: aggregate.failures,
                    weighted_exposure: aggregate.weighted_exposure,
                    weighted_failures: aggregate.weighted_failures,
                    episodes: aggregate.episode_ids.clone(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let cooccurrences = snapshot
        .pairs
        .iter()
        .map(|aggregate| {
            (
                ordered_pair(
                    Element {
                        procedure: aggregate.left.procedure,
                        version: aggregate.left.version,
                    },
                    Element {
                        procedure: aggregate.right.procedure,
                        version: aggregate.right.version,
                    },
                ),
                aggregate.together,
            )
        })
        .collect::<HashMap<_, _>>();
    cost.history_episodes_used = snapshot
        .elements
        .iter()
        .map(|aggregate| u64::from(aggregate.provenance_count))
        .sum();
    cost.work_units = cost
        .failed_trace_steps_scanned
        .saturating_add(cost.aggregate_rows_read)
        .saturating_add(cost.pair_aggregate_rows_read);
    let original_cost = original_execution_cost(
        failed_episode.cost.steps_taken,
        failed_episode.cost.budget_spent,
    );
    let attribution_cost = cost.work_units as f64;
    let total_cost = total_with_attribution(original_cost, attribution_cost);
    let ratio = cost_ratio(attribution_cost, total_cost);
    let mut ranked = current
        .into_iter()
        .map(|(element, trace_step)| {
            let element_stats = stats.get(&element);
            let exposures = element_stats.map_or(0, |value| value.exposures);
            let failures = element_stats.map_or(0, |value| value.failures);
            let cooccurrence =
                maximum_cooccurrence(element, &current_elements, &stats, &cooccurrences);
            let uncertainty = if exposures == 0 {
                1.0
            } else {
                (1.0 / f64::from(exposures).sqrt() + 0.5 * cooccurrence).min(1.0)
            };
            let weighted_exposure = element_stats.map_or(0.0, |value| value.weighted_exposure);
            let weighted_failures = element_stats.map_or(0.0, |value| value.weighted_failures);
            let failure_rate = if weighted_exposure <= 0.0 {
                0.0
            } else {
                weighted_failures / weighted_exposure
            };
            let support_factor = f64::from(exposures) / f64::from(exposures.saturating_add(2));
            let score = failure_rate * support_factor * (1.0 - 0.5 * cooccurrence);
            Attribution {
                suspect: Suspect {
                    procedure: element.procedure,
                    version: element.version,
                    trace_step,
                },
                mechanism: AttributionMechanism::StatisticalSuspicion,
                confidence: AttributionConfidence::Low,
                score,
                decisive: false,
                evidence: vec![AttributionEvidence::Statistics {
                    exposures,
                    failures,
                    support: exposures,
                    cooccurrence,
                    uncertainty,
                    weighted_exposure,
                    weighted_failures,
                }],
                limitations: if cooccurrence > 0.0 {
                    vec![
                        AttributionLimitation::CorrelationNotCausation,
                        AttributionLimitation::CorrelatedCandidates { cooccurrence },
                    ]
                } else {
                    vec![AttributionLimitation::CorrelationNotCausation]
                },
                provenance: AttributionProvenance {
                    episode_ids: element_stats
                        .map_or_else(Vec::new, |value| value.episodes.clone()),
                    details: vec![
                        "cross-episode association only; replay required before action".into(),
                    ],
                },
                attribution_cost,
                total_cost,
                attribution_cost_ratio: ratio,
            }
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.suspect.trace_step.cmp(&right.suspect.trace_step))
    });
    Ok(StatisticalRankingReport {
        attributions: ranked,
        cost,
    })
}

fn tier_weight(tier: ekg_core::VerifiabilityTier) -> f64 {
    match tier {
        ekg_core::VerifiabilityTier::Hard => 1.0,
        ekg_core::VerifiabilityTier::Consensus => 0.6,
        ekg_core::VerifiabilityTier::Deferred => 0.2,
    }
}

fn parse_trace(episode: &Episode) -> Result<ExecTrace, CreditError> {
    let json = episode
        .execution_trace
        .clone()
        .ok_or(CreditError::MissingTrace(episode.id))?;
    serde_json::from_value(json).map_err(|source| CreditError::InvalidTrace {
        episode: episode.id,
        source,
    })
}

fn maximum_cooccurrence(
    element: Element,
    candidates: &HashSet<Element>,
    stats: &HashMap<Element, ElementStats>,
    cooccurrences: &HashMap<(Element, Element), u32>,
) -> f64 {
    candidates
        .iter()
        .copied()
        .filter(|other| *other != element)
        .filter_map(|other| {
            let pair = ordered_pair(element, other);
            let together = f64::from(*cooccurrences.get(&pair).unwrap_or(&0));
            let denominator = stats
                .get(&element)
                .map_or(0, |value| value.exposures)
                .min(stats.get(&other).map_or(0, |value| value.exposures));
            (denominator > 0).then(|| together / f64::from(denominator))
        })
        .fold(0.0, f64::max)
}

fn ordered_pair(left: Element, right: Element) -> (Element, Element) {
    if (left.procedure.0, left.version) <= (right.procedure.0, right.version) {
        (left, right)
    } else {
        (right, left)
    }
}
