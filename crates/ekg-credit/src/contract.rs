use ekg_core::Episode;
use ekg_exec::{ConditionCheck, ConditionCheckStatus, ExecTrace};
use serde::{Deserialize, Serialize};

use crate::types::{cost_ratio, original_execution_cost, total_with_attribution};
use crate::{
    Attribution, AttributionConfidence, AttributionEvidence, AttributionLimitation,
    AttributionMechanism, AttributionProvenance, ContractSection, CreditError, Suspect,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractAttributionReport {
    pub attributions: Vec<Attribution>,
    pub steps_inspected: usize,
    pub attribution_cost: f64,
    pub total_cost: f64,
    pub attribution_cost_ratio: f64,
}

pub fn attribute_contract_violations(
    episode: &Episode,
) -> Result<ContractAttributionReport, CreditError> {
    let trace_json = episode
        .execution_trace
        .clone()
        .ok_or(CreditError::MissingTrace(episode.id))?;
    let trace: ExecTrace =
        serde_json::from_value(trace_json).map_err(|source| CreditError::InvalidTrace {
            episode: episode.id,
            source,
        })?;
    let steps_inspected = trace.steps.len();
    let attribution_cost = steps_inspected as f64;
    let original_cost =
        original_execution_cost(episode.cost.steps_taken, episode.cost.budget_spent);
    let total_cost = total_with_attribution(original_cost, attribution_cost);
    let ratio = cost_ratio(attribution_cost, total_cost);
    let mut attributions = Vec::new();

    for (trace_step, step) in trace.steps.iter().enumerate() {
        let procedure = step
            .procedure_called
            .ok_or(CreditError::MissingProcedure { step: trace_step })?;
        let version = step
            .procedure_version
            .ok_or(CreditError::MissingProcedureVersion { step: trace_step })?;
        let suspect = Suspect {
            procedure,
            version,
            trace_step,
        };
        collect_violations(
            &mut attributions,
            episode,
            suspect,
            ContractSection::Requires,
            &step.contract_checks.requires,
            attribution_cost,
            total_cost,
            ratio,
        );
        collect_violations(
            &mut attributions,
            episode,
            suspect,
            ContractSection::Promises,
            &step.contract_checks.promises,
            attribution_cost,
            total_cost,
            ratio,
        );
        collect_violations(
            &mut attributions,
            episode,
            suspect,
            ContractSection::FailsWhen,
            &step.contract_checks.fails_when,
            attribution_cost,
            total_cost,
            ratio,
        );
    }

    Ok(ContractAttributionReport {
        attributions,
        steps_inspected,
        attribution_cost,
        total_cost,
        attribution_cost_ratio: ratio,
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_violations(
    output: &mut Vec<Attribution>,
    episode: &Episode,
    suspect: Suspect,
    section: ContractSection,
    checks: &[ConditionCheck],
    attribution_cost: f64,
    total_cost: f64,
    ratio: f64,
) {
    output.extend(
        checks
            .iter()
            .filter(|check| check.status == ConditionCheckStatus::Violated)
            .map(|check| Attribution {
                suspect,
                mechanism: AttributionMechanism::ContractViolation,
                confidence: AttributionConfidence::High,
                score: contract_score(section),
                decisive: false,
                evidence: vec![AttributionEvidence::Contract {
                    section,
                    description: check.description.clone(),
                    status: check.status,
                }],
                limitations: vec![AttributionLimitation::ContractViolationNotSoleCause],
                provenance: AttributionProvenance {
                    episode_ids: vec![episode.id],
                    details: vec![format!(
                        "contract {section:?} violation at trace step {}",
                        suspect.trace_step
                    )],
                },
                attribution_cost,
                total_cost,
                attribution_cost_ratio: ratio,
            }),
    );
}

fn contract_score(section: ContractSection) -> f64 {
    match section {
        ContractSection::Requires => 0.95,
        ContractSection::Promises => 1.0,
        ContractSection::FailsWhen => 0.98,
    }
}
