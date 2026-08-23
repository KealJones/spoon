use std::cmp::Ordering;
use std::fmt::Display;

use serde::{Deserialize, Serialize};
use spoon_core::Episode;

use crate::types::{cost_ratio, total_with_attribution};
use crate::{
    Attribution, AttributionConfidence, AttributionEvidence, AttributionLimitation,
    AttributionMechanism, AttributionProvenance, CounterfactualMode, CreditError, Suspect,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CounterfactualChange {
    pub description: String,
    pub replacement: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CounterfactualCandidate {
    pub suspect: Suspect,
    pub prior_score: f64,
    pub change: CounterfactualChange,
    pub mode: CounterfactualMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplayBudget {
    pub top_k: usize,
    pub max_replays: u32,
    pub max_steps: u32,
    pub total_episode_cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplayRequest {
    pub source_episode: spoon_core::EpisodeId,
    pub suspect: Suspect,
    /// A singular field makes the one-change invariant structural.
    pub change: CounterfactualChange,
    pub mode: CounterfactualMode,
    pub step_budget: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayObservation {
    pub outcome: ReplayOutcome,
    pub steps_used: u32,
    pub details: String,
    #[serde(default)]
    pub provenance: crate::ReplayProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReplayOutcome {
    Succeeded,
    Failed,
    NotReplayable { reason: String },
}

pub trait CounterfactualReplayer {
    type Error: Display;

    fn replay(&mut self, request: ReplayRequest) -> Result<ReplayObservation, Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStopReason {
    TopK,
    ReplayLimit,
    StepLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CounterfactualReport {
    pub attributions: Vec<Attribution>,
    pub replays_run: u32,
    pub steps_spent: u32,
    pub stop_reason: Option<BudgetStopReason>,
    pub attribution_cost: f64,
    pub total_cost: f64,
    pub attribution_cost_ratio: f64,
}

pub fn run_counterfactual_replays<R: CounterfactualReplayer>(
    source: &Episode,
    candidates: &[CounterfactualCandidate],
    budget: ReplayBudget,
    replayer: &mut R,
) -> Result<CounterfactualReport, CreditError> {
    if !budget.total_episode_cost.is_finite() || budget.total_episode_cost < 0.0 {
        return Err(CreditError::InvalidTotalCost(budget.total_episode_cost));
    }
    let mut ranked = candidates.iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .prior_score
            .partial_cmp(&left.prior_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.suspect.trace_step.cmp(&right.suspect.trace_step))
            .then_with(|| left.suspect.procedure.0.cmp(&right.suspect.procedure.0))
            .then_with(|| left.suspect.version.cmp(&right.suspect.version))
    });
    let selected_count = budget.top_k.min(ranked.len());
    ranked.truncate(selected_count);

    let mut replays_run = 0_u32;
    let mut steps_spent = 0_u32;
    let mut attributions = Vec::new();
    for candidate in ranked {
        if replays_run >= budget.max_replays || steps_spent >= budget.max_steps {
            break;
        }
        let step_budget = budget.max_steps - steps_spent;
        let observation = replayer
            .replay(ReplayRequest {
                source_episode: source.id,
                suspect: candidate.suspect,
                change: candidate.change.clone(),
                mode: candidate.mode,
                step_budget,
            })
            .map_err(|error| CreditError::Replay(error.to_string()))?;
        if observation.steps_used > step_budget {
            return Err(CreditError::ReplayExceededStepBudget {
                used: observation.steps_used,
                allowed: step_budget,
            });
        }
        replays_run += 1;
        steps_spent += observation.steps_used;
        let provenance_error = validate_replay_provenance(source, candidate.mode, &observation);
        let (confidence, decisive, score) = match (
            &observation.outcome,
            candidate.mode,
            provenance_error.is_none(),
        ) {
            (ReplayOutcome::Succeeded, CounterfactualMode::Deterministic, true) => {
                // The generic credit crate cannot know whether a replayer is
                // an engine-owned trust boundary. The Engine may promote this
                // result after validating its canonical oracle binding.
                (AttributionConfidence::Medium, false, 0.75)
            }
            (ReplayOutcome::Succeeded, CounterfactualMode::Simulated, true) => (
                AttributionConfidence::Inconclusive,
                false,
                candidate.prior_score * 0.25,
            ),
            (ReplayOutcome::Succeeded, _, false)
            | (ReplayOutcome::Failed | ReplayOutcome::NotReplayable { .. }, _, _) => (
                AttributionConfidence::Inconclusive,
                false,
                candidate.prior_score * 0.25,
            ),
        };
        let mut limitations = Vec::new();
        if candidates.len() > 1 {
            limitations.push(
                AttributionLimitation::SingleChangeCannotDetectInteractions {
                    candidate_count: candidates.len(),
                },
            );
        }
        if let ReplayOutcome::NotReplayable { reason } = &observation.outcome {
            limitations.push(AttributionLimitation::NotReplayable {
                reason: reason.clone(),
            });
        }
        if let Some(reason) = provenance_error {
            limitations.push(AttributionLimitation::UnverifiedReplayProvenance { reason });
        }
        let counterfactual_succeeded = match &observation.outcome {
            ReplayOutcome::Succeeded => Some(true),
            ReplayOutcome::Failed => Some(false),
            ReplayOutcome::NotReplayable { .. } => None,
        };
        attributions.push(Attribution {
            suspect: candidate.suspect,
            mechanism: AttributionMechanism::CounterfactualReplay,
            confidence,
            score,
            decisive,
            evidence: vec![AttributionEvidence::Replay {
                mode: candidate.mode,
                change_description: candidate.change.description.clone(),
                counterfactual_succeeded,
                steps_used: observation.steps_used,
                details: observation.details,
                provenance: observation.provenance,
            }],
            limitations,
            provenance: AttributionProvenance {
                episode_ids: vec![source.id],
                details: vec!["exactly one counterfactual change was replayed".into()],
            },
            attribution_cost: 0.0,
            total_cost: budget.total_episode_cost,
            attribution_cost_ratio: 0.0,
        });
    }

    let stop_reason =
        if replays_run >= budget.max_replays && (replays_run as usize) < selected_count {
            Some(BudgetStopReason::ReplayLimit)
        } else if steps_spent >= budget.max_steps && (replays_run as usize) < selected_count {
            Some(BudgetStopReason::StepLimit)
        } else if selected_count < candidates.len() {
            Some(BudgetStopReason::TopK)
        } else {
            None
        };
    let attribution_cost = f64::from(steps_spent);
    let total_cost = total_with_attribution(budget.total_episode_cost, attribution_cost);
    let ratio = cost_ratio(attribution_cost, total_cost);
    for attribution in &mut attributions {
        attribution.attribution_cost = attribution_cost;
        attribution.total_cost = total_cost;
        attribution.attribution_cost_ratio = ratio;
    }
    Ok(CounterfactualReport {
        attributions,
        replays_run,
        steps_spent,
        stop_reason,
        attribution_cost,
        total_cost,
        attribution_cost_ratio: ratio,
    })
}

fn validate_replay_provenance(
    source: &Episode,
    mode: CounterfactualMode,
    observation: &ReplayObservation,
) -> Option<String> {
    if !source.failed() {
        return Some("source episode is not a failed evaluated episode".into());
    }
    let provenance = &observation.provenance;
    if provenance
        .source_trace_hash
        .as_deref()
        .is_none_or(str::is_empty)
        || provenance
            .mutation_hash
            .as_deref()
            .is_none_or(str::is_empty)
    {
        return Some("replay is missing source-trace or mutation identity".into());
    }
    if mode == CounterfactualMode::Simulated {
        return Some("simulated replay has no trusted simulator receipt".into());
    }
    let matches = matches!(
        (&provenance.verification, mode),
        (
            Some(crate::ReplayVerificationProvenance::Deterministic { verifier }),
            CounterfactualMode::Deterministic
        ) if !verifier.trim().is_empty()
    );
    (!matches).then(|| "replay verification provenance does not match its mode".into())
}
