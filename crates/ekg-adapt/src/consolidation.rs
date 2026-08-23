use ekg_core::{Episode, EpisodeId, Expr, ProcedureId, UnOp, VerifiabilityTier};
use ekg_exec::{ConditionCheck, ConditionCheckStatus, ExecStep, ExecStepStatus, ExecTrace};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A conservative skill candidate extracted from immutable episode evidence.
/// Candidates are reports only; promotion remains the separate replay gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillCandidate {
    pub name: String,
    pub source_episode_ids: Vec<EpisodeId>,
    pub support_count: u32,
    pub rationale: String,
    pub failure_critic: bool,
}

/// The evidence shape behind a discovered skill. These artifacts remain
/// separate from managed-skill storage so a later admission layer can inspect
/// exactly what was generalized instead of trusting an action label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryKind {
    RepeatedVerifiedExecution,
    SingleVerifiedExecution,
    FailureCritic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutableStep {
    pub position: u32,
    pub procedure_id: String,
    pub procedure_version: u32,
    pub expression: String,
}

/// The ordered, version-pinned procedure calls that were actually observed.
/// Inputs and results are not generalized: they remain in the source episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutableStructure {
    pub action: String,
    pub steps: Vec<ExecutableStep>,
}

/// Only conditions that execution recorded as passed are present here. Empty
/// lists mean that no executable conditions were recorded, not that a contract
/// was silently assumed to hold.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractEvidence {
    pub verified_requires: Vec<String>,
    pub verified_promises: Vec<String>,
    pub verified_non_failure_conditions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryEvidence {
    pub evaluation_tiers: Vec<VerifiabilityTier>,
    pub evaluation_details: Vec<String>,
    pub contract: ContractEvidence,
    pub trace_step_count: u32,
}

/// Which contract section produced a guardable failure. Promise failures are
/// deliberately excluded: they are not preventive preconditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardContractSection {
    Requires,
    FailsWhen,
}

/// A neutral, compilable pre-dispatch guard. The runtime must bind
/// `runtime_binding` to the freshly checked condition before evaluating
/// `guard_expr`; a description by itself is never treated as executable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreventiveGuardSpecification {
    pub procedure_id: String,
    pub procedure_version: u32,
    pub contract_section: GuardContractSection,
    pub condition_description: String,
    pub runtime_binding: String,
    pub guard_expr: Expr,
    pub evaluation_detail: String,
}

impl PreventiveGuardSpecification {
    pub fn compile(&self) -> Expr {
        self.guard_expr.clone()
    }
}

/// Full discoverable artifact. `candidate` preserves the current admission
/// boundary while the other fields retain inspectable generalized evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryArtifact {
    pub kind: DiscoveryKind,
    pub candidate: SkillCandidate,
    pub executable: ExecutableStructure,
    pub evidence: DiscoveryEvidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preventive_guard: Option<PreventiveGuardSpecification>,
}

pub fn discover_skills(episodes: &[Episode]) -> Vec<SkillCandidate> {
    discover_skill_artifacts(episodes)
        .into_iter()
        .map(|artifact| artifact.candidate)
        .collect()
}

/// Finds only repeated executions whose action, ordered procedure/version
/// path, expression descriptions, and executable contract checks agree.
/// Matching action labels alone are deliberately not discovery evidence.
pub fn discover_skill_artifacts(episodes: &[Episode]) -> Vec<DiscoveryArtifact> {
    let mut groups: BTreeMap<ExecutionFingerprint, Vec<SuccessfulExecution>> = BTreeMap::new();
    for episode in episodes {
        let Some(execution) = successful_execution(episode) else {
            continue;
        };
        groups
            .entry(execution.fingerprint())
            .or_default()
            .push(execution);
    }
    groups
        .into_values()
        .filter(|executions| executions.len() >= 2)
        .map(repeated_artifact)
        .collect()
}

pub fn discover_single_success(episode: &Episode) -> Option<SkillCandidate> {
    discover_single_success_artifact(episode).map(|artifact| artifact.candidate)
}

/// Extracts the exact execution/contract evidence from one strong success. It
/// refuses action-only episodes and incomplete traces rather than inventing a
/// generic explanation for them.
pub fn discover_single_success_artifact(episode: &Episode) -> Option<DiscoveryArtifact> {
    let execution = successful_execution(episode)?;
    let evidence = DiscoveryEvidence {
        evaluation_tiers: vec![execution.tier],
        evaluation_details: vec![execution.evaluation_detail.clone()],
        contract: execution.contract.clone(),
        trace_step_count: execution.structure.steps.len() as u32,
    };
    let rationale = format!(
        "one {:?}-verified execution: {} trace step(s) through {}; {} executable contract check(s) passed; verifier: {}",
        execution.tier,
        evidence.trace_step_count,
        executable_summary(&execution.structure),
        contract_check_count(&execution.contract),
        execution.evaluation_detail,
    );
    Some(DiscoveryArtifact {
        kind: DiscoveryKind::SingleVerifiedExecution,
        candidate: SkillCandidate {
            name: format!("explanation:{}", episode.id),
            source_episode_ids: vec![episode.id],
            support_count: 1,
            rationale,
            failure_critic: false,
        },
        executable: execution.structure,
        evidence,
        preventive_guard: None,
    })
}

pub fn discover_failure_critic(episode: &Episode) -> Option<SkillCandidate> {
    discover_failure_critic_artifact(episode).map(|artifact| artifact.candidate)
}

/// Produces a preventive guard only for a strong failed execution that records
/// a violated `requires` or `fails_when` check. A failed promise, missing
/// trace, deferred evaluation, or non-executable check is not enough evidence
/// to safely claim a pre-dispatch remedy.
pub fn discover_failure_critic_artifact(episode: &Episode) -> Option<DiscoveryArtifact> {
    let evaluation = episode.evaluation.as_ref()?;
    if evaluation.success || !strong_tier(evaluation.tier) || evaluation.details.trim().is_empty() {
        return None;
    }
    let action = episode.action.as_deref()?;
    let trace = decode_trace(episode)?;
    let (position, step, section, condition) = failed_precondition(&trace)?;
    let procedure = step.procedure_called?;
    let version = step.procedure_version?;
    let structure = structure_from_trace(action, &trace)?;
    let runtime_binding = contract_binding(procedure, version, section, &condition.description);
    let guard_expr = match section {
        GuardContractSection::Requires => Expr::Var(runtime_binding.clone()),
        GuardContractSection::FailsWhen => Expr::UnOp {
            op: UnOp::Not,
            operand: Box::new(Expr::Var(runtime_binding.clone())),
        },
    };
    let guard = PreventiveGuardSpecification {
        procedure_id: procedure.to_string(),
        procedure_version: version,
        contract_section: section,
        condition_description: condition.description.clone(),
        runtime_binding,
        guard_expr,
        evaluation_detail: evaluation.details.clone(),
    };
    let contract = failed_contract_evidence(section, &condition.description);
    let rationale = format!(
        "{:?}-verified failure at trace step {position}: {:?} condition {:?} was violated; preventive guard requires binding `{}`; verifier: {}",
        evaluation.tier, section, condition.description, guard.runtime_binding, evaluation.details,
    );
    Some(DiscoveryArtifact {
        kind: DiscoveryKind::FailureCritic,
        candidate: SkillCandidate {
            name: format!("critic:{}", episode.id),
            source_episode_ids: vec![episode.id],
            support_count: 1,
            rationale,
            failure_critic: true,
        },
        executable: structure,
        evidence: DiscoveryEvidence {
            evaluation_tiers: vec![evaluation.tier],
            evaluation_details: vec![evaluation.details.clone()],
            contract,
            trace_step_count: trace.steps.len() as u32,
        },
        preventive_guard: Some(guard),
    })
}

#[derive(Debug, Clone)]
struct SuccessfulExecution {
    episode_id: EpisodeId,
    tier: VerifiabilityTier,
    evaluation_detail: String,
    structure: ExecutableStructure,
    contract: ContractEvidence,
}

impl SuccessfulExecution {
    fn fingerprint(&self) -> ExecutionFingerprint {
        ExecutionFingerprint {
            action: self.structure.action.clone(),
            steps: self
                .structure
                .steps
                .iter()
                .map(|step| {
                    format!(
                        "{}:{}@{}:{}",
                        step.position, step.procedure_id, step.procedure_version, step.expression
                    )
                })
                .collect(),
            requires: self.contract.verified_requires.clone(),
            promises: self.contract.verified_promises.clone(),
            fails_when: self.contract.verified_non_failure_conditions.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ExecutionFingerprint {
    action: String,
    steps: Vec<String>,
    requires: Vec<String>,
    promises: Vec<String>,
    fails_when: Vec<String>,
}

fn repeated_artifact(executions: Vec<SuccessfulExecution>) -> DiscoveryArtifact {
    let first = &executions[0];
    let source_episode_ids = executions
        .iter()
        .map(|execution| execution.episode_id)
        .collect::<Vec<_>>();
    let evaluation_tiers = executions.iter().map(|execution| execution.tier).collect();
    let evaluation_details = executions
        .iter()
        .map(|execution| execution.evaluation_detail.clone())
        .collect();
    let rationale = format!(
        "{} independent verified executions share {} trace step(s) through {}; {} executable contract check(s) passed on every cited episode",
        source_episode_ids.len(),
        first.structure.steps.len(),
        executable_summary(&first.structure),
        contract_check_count(&first.contract),
    );
    DiscoveryArtifact {
        kind: DiscoveryKind::RepeatedVerifiedExecution,
        candidate: SkillCandidate {
            name: format!("repeated:{}", first.structure.action),
            source_episode_ids,
            support_count: executions.len() as u32,
            rationale,
            failure_critic: false,
        },
        executable: first.structure.clone(),
        evidence: DiscoveryEvidence {
            evaluation_tiers,
            evaluation_details,
            contract: first.contract.clone(),
            trace_step_count: first.structure.steps.len() as u32,
        },
        preventive_guard: None,
    }
}

fn successful_execution(episode: &Episode) -> Option<SuccessfulExecution> {
    let evaluation = episode.evaluation.as_ref()?;
    if !evaluation.success || !strong_tier(evaluation.tier) || evaluation.details.trim().is_empty()
    {
        return None;
    }
    let action = episode.action.as_deref()?;
    let trace = decode_trace(episode)?;
    let structure = structure_from_trace(action, &trace)?;
    let contract = successful_contract_evidence(&trace)?;
    Some(SuccessfulExecution {
        episode_id: episode.id,
        tier: evaluation.tier,
        evaluation_detail: evaluation.details.clone(),
        structure,
        contract,
    })
}

fn strong_tier(tier: VerifiabilityTier) -> bool {
    matches!(tier, VerifiabilityTier::Hard | VerifiabilityTier::Consensus)
}

fn decode_trace(episode: &Episode) -> Option<ExecTrace> {
    let trace = serde_json::from_value::<ExecTrace>(episode.execution_trace.clone()?).ok()?;
    (!trace.steps.is_empty()).then_some(trace)
}

fn structure_from_trace(action: &str, trace: &ExecTrace) -> Option<ExecutableStructure> {
    let steps = trace
        .steps
        .iter()
        .enumerate()
        .map(|(position, step)| {
            Some(ExecutableStep {
                position: position as u32,
                procedure_id: step.procedure_called?.to_string(),
                procedure_version: step.procedure_version?,
                expression: step.expr_description.clone(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(ExecutableStructure {
        action: action.to_owned(),
        steps,
    })
}

fn successful_contract_evidence(trace: &ExecTrace) -> Option<ContractEvidence> {
    let mut evidence = ContractEvidence::default();
    for step in &trace.steps {
        if !matches!(step.status, ExecStepStatus::Succeeded) {
            return None;
        }
        append_passed_checks(
            &mut evidence.verified_requires,
            &step.contract_checks.requires,
        )?;
        append_passed_checks(
            &mut evidence.verified_promises,
            &step.contract_checks.promises,
        )?;
        append_passed_checks(
            &mut evidence.verified_non_failure_conditions,
            &step.contract_checks.fails_when,
        )?;
    }
    Some(evidence)
}

fn append_passed_checks(destination: &mut Vec<String>, checks: &[ConditionCheck]) -> Option<()> {
    for check in checks {
        if check.description.trim().is_empty() || check.status != ConditionCheckStatus::Passed {
            return None;
        }
        destination.push(check.description.clone());
    }
    Some(())
}

fn failed_precondition(
    trace: &ExecTrace,
) -> Option<(usize, &ExecStep, GuardContractSection, &ConditionCheck)> {
    for (position, step) in trace.steps.iter().enumerate() {
        if !matches!(step.status, ExecStepStatus::Failed { .. })
            || step.procedure_called.is_none()
            || step.procedure_version.is_none()
        {
            continue;
        }
        if let Some(check) = step.contract_checks.requires.iter().find(|check| {
            check.status == ConditionCheckStatus::Violated && !check.description.trim().is_empty()
        }) {
            return Some((position, step, GuardContractSection::Requires, check));
        }
        if let Some(check) = step.contract_checks.fails_when.iter().find(|check| {
            check.status == ConditionCheckStatus::Violated && !check.description.trim().is_empty()
        }) {
            return Some((position, step, GuardContractSection::FailsWhen, check));
        }
    }
    None
}

fn failed_contract_evidence(section: GuardContractSection, condition: &str) -> ContractEvidence {
    match section {
        GuardContractSection::Requires => ContractEvidence {
            verified_requires: vec![format!("violated: {condition}")],
            ..ContractEvidence::default()
        },
        GuardContractSection::FailsWhen => ContractEvidence {
            verified_non_failure_conditions: vec![format!("violated: {condition}")],
            ..ContractEvidence::default()
        },
    }
}

fn contract_binding(
    procedure: ProcedureId,
    version: u32,
    section: GuardContractSection,
    description: &str,
) -> String {
    let section = match section {
        GuardContractSection::Requires => "requires",
        GuardContractSection::FailsWhen => "fails_when",
    };
    let description_hex = description
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("contract::{procedure}@{version}::{section}::{description_hex}")
}

fn executable_summary(structure: &ExecutableStructure) -> String {
    structure
        .steps
        .iter()
        .map(|step| format!("{}@{}", step.procedure_id, step.procedure_version))
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn contract_check_count(contract: &ContractEvidence) -> usize {
    contract.verified_requires.len()
        + contract.verified_promises.len()
        + contract.verified_non_failure_conditions.len()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeCompressionPlan {
    pub retain_full: Vec<EpisodeId>,
    pub summarize: Vec<EpisodeId>,
    pub forgotten_as_known_gap: Vec<EpisodeId>,
}

/// Plans bounded compression while preserving every failure and the boundary
/// examples of a repeated family. It never deletes or mutates episode rows.
pub fn plan_episode_compression(episodes: &[Episode]) -> EpisodeCompressionPlan {
    let mut groups: BTreeMap<String, Vec<&Episode>> = BTreeMap::new();
    for episode in episodes {
        groups
            .entry(
                episode
                    .action
                    .clone()
                    .unwrap_or_else(|| format!("situation:{}", episode.situation)),
            )
            .or_default()
            .push(episode);
    }
    let mut retain_full = Vec::new();
    let mut summarize = Vec::new();
    for group in groups.values() {
        for (index, episode) in group.iter().enumerate() {
            let keep =
                group.len() <= 2 || index == 0 || index + 1 == group.len() || episode.failed();
            if keep {
                retain_full.push(episode.id);
            } else {
                summarize.push(episode.id);
            }
        }
    }
    retain_full.sort_unstable_by_key(|id| id.to_string());
    summarize.sort_unstable_by_key(|id| id.to_string());
    EpisodeCompressionPlan {
        retain_full,
        summarize,
        forgotten_as_known_gap: Vec::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetirementRecord {
    pub retired_skill: String,
    pub successor_skill: String,
    pub reason: String,
    pub reconstructible: bool,
}

pub fn retire_skill(
    retired_skill: impl Into<String>,
    successor_skill: impl Into<String>,
    reason: impl Into<String>,
) -> RetirementRecord {
    RetirementRecord {
        retired_skill: retired_skill.into(),
        successor_skill: successor_skill.into(),
        reason: reason.into(),
        reconstructible: true,
    }
}

#[cfg(test)]
mod tests {
    use ekg_core::{Episode, EscalationRung, Evaluation, ProcedureId, Value, VerifiabilityTier};
    use ekg_exec::{
        ConditionCheck, ConditionCheckStatus, ContractChecks, Env, Evaluator, ExecStep,
        ExecStepStatus, ExecTrace,
    };

    use super::{
        GuardContractSection, discover_failure_critic, discover_failure_critic_artifact,
        discover_single_success, discover_single_success_artifact, discover_skill_artifacts,
        discover_skills, plan_episode_compression, retire_skill,
    };

    fn episode(action: &str, success: bool, created_at: i64) -> Episode {
        let mut value = Episode::new("task");
        value.action = Some(action.into());
        value.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success,
            details: "checked".into(),
            surprise: None,
        });
        value.cost.rung_reached = EscalationRung::Run;
        value.created_at = created_at;
        value
    }

    fn trace(
        procedure: ProcedureId,
        version: u32,
        status: ExecStepStatus,
        requires: Vec<ConditionCheck>,
        promises: Vec<ConditionCheck>,
        fails_when: Vec<ConditionCheck>,
    ) -> ExecTrace {
        ExecTrace {
            steps: vec![ExecStep {
                expr_description: "call tested procedure".into(),
                input: Some(Value::List(vec![Value::Int(2)])),
                output: Value::Int(4),
                procedure_called: Some(procedure),
                procedure_version: Some(version),
                contract_checks: ContractChecks {
                    requires,
                    promises,
                    fails_when,
                },
                status,
            }],
        }
    }

    fn passed(description: &str) -> ConditionCheck {
        ConditionCheck {
            description: description.into(),
            status: ConditionCheckStatus::Passed,
        }
    }

    fn violated(description: &str) -> ConditionCheck {
        ConditionCheck {
            description: description.into(),
            status: ConditionCheckStatus::Violated,
        }
    }

    #[test]
    fn repeated_successes_discover_and_compression_preserves_boundaries_and_failures() {
        let procedure = ProcedureId::new();
        let mut one = episode("procedure:p@1", true, 1);
        one.execution_trace = Some(
            serde_json::to_value(trace(
                procedure,
                1,
                ExecStepStatus::Succeeded,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ))
            .unwrap(),
        );
        let mut two = one.clone();
        two.id = ekg_core::EpisodeId::new();
        two.created_at = 2;
        let mut failure = episode("procedure:p@1", false, 3);
        failure.execution_trace = Some(
            serde_json::to_value(trace(
                procedure,
                1,
                ExecStepStatus::Failed {
                    error: "contract violation".into(),
                },
                vec![violated("safe input")],
                Vec::new(),
                Vec::new(),
            ))
            .unwrap(),
        );
        let mut four = one.clone();
        four.id = ekg_core::EpisodeId::new();
        four.created_at = 4;
        let episodes = vec![one, two, failure, four];
        assert_eq!(discover_skills(&episodes)[0].support_count, 3);
        let plan = plan_episode_compression(&episodes);
        assert_eq!(plan.summarize.len(), 1);
        assert!(plan.retain_full.contains(&episodes[2].id));
        assert!(
            discover_failure_critic(&episodes[2])
                .unwrap()
                .failure_critic
        );
        assert!(retire_skill("old", "new", "replaced").reconstructible);
    }

    #[test]
    fn repeated_discovery_extracts_common_execution_and_contract_evidence() {
        let procedure = ProcedureId::new();
        let mut first = episode("procedure:common@3", true, 1);
        first.execution_trace = Some(
            serde_json::to_value(trace(
                procedure,
                3,
                ExecStepStatus::Succeeded,
                vec![passed("x is positive")],
                vec![passed("output is doubled")],
                vec![passed("input is not forbidden")],
            ))
            .unwrap(),
        );
        let mut second = first.clone();
        second.id = ekg_core::EpisodeId::new();
        second.evaluation.as_mut().unwrap().details = "independent deterministic check".into();
        let artifacts = discover_skill_artifacts(&[first, second]);
        assert_eq!(artifacts.len(), 1);
        let artifact = &artifacts[0];
        assert_eq!(artifact.candidate.support_count, 2);
        assert_eq!(
            artifact.executable.steps[0].procedure_id,
            procedure.to_string()
        );
        assert_eq!(artifact.executable.steps[0].procedure_version, 3);
        assert_eq!(
            artifact.evidence.contract.verified_requires,
            ["x is positive"]
        );
        assert_eq!(
            artifact.evidence.contract.verified_promises,
            ["output is doubled"]
        );
        assert_eq!(artifact.evidence.evaluation_details.len(), 2);
        assert!(
            artifact
                .candidate
                .rationale
                .contains("independent verified executions")
        );
        assert_eq!(artifact.executable.action, "procedure:common@3");
    }

    #[test]
    fn discovery_refuses_action_only_and_non_homogeneous_evidence() {
        let raw = episode("procedure:common@3", true, 1);
        assert!(discover_skill_artifacts(&[raw.clone(), raw.clone()]).is_empty());
        assert!(discover_single_success(&raw).is_none());

        let procedure = ProcedureId::new();
        let mut first = episode("procedure:common@3", true, 1);
        first.execution_trace = Some(
            serde_json::to_value(trace(
                procedure,
                3,
                ExecStepStatus::Succeeded,
                vec![passed("x is positive")],
                Vec::new(),
                Vec::new(),
            ))
            .unwrap(),
        );
        let mut different_version = first.clone();
        different_version.id = ekg_core::EpisodeId::new();
        different_version.execution_trace = Some(
            serde_json::to_value(trace(
                procedure,
                4,
                ExecStepStatus::Succeeded,
                vec![passed("x is positive")],
                Vec::new(),
                Vec::new(),
            ))
            .unwrap(),
        );
        assert!(discover_skill_artifacts(&[first.clone(), different_version]).is_empty());

        let mut non_executable = first.clone();
        non_executable.id = ekg_core::EpisodeId::new();
        non_executable.execution_trace = Some(
            serde_json::to_value(trace(
                procedure,
                3,
                ExecStepStatus::Succeeded,
                vec![ConditionCheck {
                    description: "x is positive".into(),
                    status: ConditionCheckStatus::NotExecutable,
                }],
                Vec::new(),
                Vec::new(),
            ))
            .unwrap(),
        );
        assert!(discover_skill_artifacts(&[first, non_executable]).is_empty());
    }

    #[test]
    fn single_success_cites_evaluation_contract_and_trace_evidence() {
        let procedure = ProcedureId::new();
        let mut success = episode("procedure:single@1", true, 1);
        success.execution_trace = Some(
            serde_json::to_value(trace(
                procedure,
                1,
                ExecStepStatus::Succeeded,
                vec![passed("input is finite")],
                vec![passed("result is finite")],
                Vec::new(),
            ))
            .unwrap(),
        );
        let artifact = discover_single_success_artifact(&success).unwrap();
        assert_eq!(artifact.evidence.trace_step_count, 1);
        assert_eq!(
            artifact.evidence.contract.verified_requires,
            ["input is finite"]
        );
        assert!(artifact.candidate.rationale.contains("checked"));
        assert!(
            artifact
                .candidate
                .rationale
                .contains(&procedure.to_string())
        );
    }

    #[test]
    fn failure_critic_yields_a_compilable_guard_from_contract_evidence() {
        let procedure = ProcedureId::new();
        let mut failed = episode("procedure:guarded@2", false, 1);
        failed.execution_trace = Some(
            serde_json::to_value(trace(
                procedure,
                2,
                ExecStepStatus::Failed {
                    error: "contract violation".into(),
                },
                vec![violated("caller supplied a positive divisor")],
                Vec::new(),
                Vec::new(),
            ))
            .unwrap(),
        );
        let artifact = discover_failure_critic_artifact(&failed).unwrap();
        let guard = artifact.preventive_guard.unwrap();
        assert_eq!(guard.contract_section, GuardContractSection::Requires);
        let mut env = Env::new();
        env.set(guard.runtime_binding.clone(), Value::Bool(true));
        assert_eq!(
            Evaluator::new().eval(&guard.compile(), &mut env).unwrap(),
            Value::Bool(true)
        );
        env.set(guard.runtime_binding.clone(), Value::Bool(false));
        assert_eq!(
            Evaluator::new().eval(&guard.compile(), &mut env).unwrap(),
            Value::Bool(false)
        );
        assert!(
            discover_failure_critic(&failed)
                .unwrap()
                .rationale
                .contains("violated")
        );
    }

    #[test]
    fn failure_critic_refuses_unpreventable_or_unverified_failures() {
        let procedure = ProcedureId::new();
        let mut promise_failure = episode("procedure:guarded@2", false, 1);
        promise_failure.execution_trace = Some(
            serde_json::to_value(trace(
                procedure,
                2,
                ExecStepStatus::Failed {
                    error: "postcondition violation".into(),
                },
                Vec::new(),
                vec![violated("result is sorted")],
                Vec::new(),
            ))
            .unwrap(),
        );
        assert!(discover_failure_critic_artifact(&promise_failure).is_none());
        let mut deferred = episode("procedure:guarded@2", false, 1);
        deferred.evaluation.as_mut().unwrap().tier = VerifiabilityTier::Deferred;
        assert!(discover_failure_critic_artifact(&deferred).is_none());
    }
}
