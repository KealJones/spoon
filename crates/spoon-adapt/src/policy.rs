use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use spoon_core::{
    ConceptId, Condition, Episode, EpisodeId, Param, Procedure, ProcedureId, ScopeCondition, Value,
    VerifiabilityTier,
};
use spoon_credit::{
    Attribution, AttributionConfidence, AttributionEvidence, AttributionMechanism, ContractSection,
    CounterfactualMode,
};
use spoon_episode::EpisodeStore;
use spoon_exec::{
    ConditionCheck, ConditionCheckStatus, Env, Evaluator, ExecStep, ExecStepStatus, ExecTrace,
};
use spoon_graph::KnowledgeStore;

use crate::error::{AdaptError, Result};

const PROCEDURE_REPLACEMENT_EPISODES: u32 = 3;
const CONCEPT_REVISION_EPISODES: u32 = 5;
const CONCEPT_REVISION_SOURCES: u32 = 2;

/// Lossless local boundary for the strength classes produced by `spoon-credit`.
/// Statistical suspicion is deliberately non-actionable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttributionStrength {
    StatisticalSuspicion,
    InsufficientEvidence,
    SimulatedEvidence,
    ContractViolation,
    ReplayConfirmed,
}

impl From<&Attribution> for AttributionStrength {
    fn from(attribution: &Attribution) -> Self {
        match attribution.mechanism {
            AttributionMechanism::StatisticalSuspicion => Self::StatisticalSuspicion,
            AttributionMechanism::ContractViolation
                if matches!(
                    attribution.confidence,
                    AttributionConfidence::High | AttributionConfidence::Certain
                ) =>
            {
                Self::ContractViolation
            }
            AttributionMechanism::CounterfactualReplay
                if matches!(
                    attribution.confidence,
                    AttributionConfidence::High | AttributionConfidence::Certain
                ) =>
            {
                match attribution
                    .evidence
                    .iter()
                    .find_map(|evidence| match evidence {
                        AttributionEvidence::Replay {
                            mode,
                            counterfactual_succeeded: Some(true),
                            ..
                        } => Some(*mode),
                        _ => None,
                    }) {
                    Some(CounterfactualMode::Deterministic) => Self::ReplayConfirmed,
                    Some(CounterfactualMode::Simulated) => Self::SimulatedEvidence,
                    None => Self::InsufficientEvidence,
                }
            }
            _ => Self::InsufficientEvidence,
        }
    }
}

impl AttributionStrength {
    fn actionable(self) -> bool {
        matches!(self, Self::ContractViolation | Self::ReplayConfirmed)
    }

    fn broadly_actionable(self) -> bool {
        matches!(self, Self::ContractViolation | Self::ReplayConfirmed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceGate {
    pub verified_episodes: u32,
    pub distinct_sources: u32,
    pub strongest_tier: Option<VerifiabilityTier>,
    pub challenger_beats_incumbent: bool,
    pub corroborated: bool,
    pub offline: bool,
}

impl EvidenceGate {
    fn has_strong_tier(&self) -> bool {
        matches!(
            self.strongest_tier,
            Some(VerifiabilityTier::Hard | VerifiabilityTier::Consensus)
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CorrectionTarget {
    UnusualInput {
        reason: String,
    },
    Assumption {
        key: String,
        replacement: Value,
    },
    ProcedureScope {
        procedure_id: ProcedureId,
        expected_version: u32,
        condition: Condition,
        learned_from: EpisodeId,
    },
    ProcedureReplacement {
        incumbent_id: ProcedureId,
        incumbent_version: u32,
        challenger: Box<Procedure>,
    },
    ConceptRevision {
        concept_id: ConceptId,
        expected_version: u32,
        revised_description: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionRequest {
    pub attribution: AttributionStrength,
    pub target: CorrectionTarget,
    pub evidence: EvidenceGate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CorrectionAction {
    RecordOnly {
        reason: String,
    },
    FixAssumption {
        key: String,
        replacement: Value,
    },
    NarrowScope {
        procedure_id: ProcedureId,
        expected_version: u32,
        condition: Condition,
        learned_from: EpisodeId,
    },
    ReplaceProcedure {
        incumbent_id: ProcedureId,
        incumbent_version: u32,
        challenger: Box<Procedure>,
    },
    ReviseConceptOffline {
        concept_id: ConceptId,
        expected_version: u32,
        revised_description: String,
        supporting_episodes: u32,
    },
    ScheduleTest {
        reason: String,
    },
}

impl CorrectionAction {
    pub fn is_schedule_test(&self) -> bool {
        matches!(self, Self::ScheduleTest { .. })
    }

    pub fn is_narrow_scope(&self) -> bool {
        matches!(self, Self::NarrowScope { .. })
    }

    pub fn is_replace_procedure(&self) -> bool {
        matches!(self, Self::ReplaceProcedure { .. })
    }

    pub fn is_revise_concept(&self) -> bool {
        matches!(self, Self::ReviseConceptOffline { .. })
    }

    pub fn modifies_procedure(&self) -> bool {
        matches!(
            self,
            Self::NarrowScope { .. } | Self::ReplaceProcedure { .. }
        )
    }

    pub fn assumption_fix(&self) -> Option<(&str, &Value)> {
        match self {
            Self::FixAssumption { key, replacement } => Some((key, replacement)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionDecision {
    pub action: CorrectionAction,
    pub rationale: String,
}

pub struct AdaptationPolicy;

impl AdaptationPolicy {
    pub fn decide(request: CorrectionRequest) -> CorrectionDecision {
        if request.attribution == AttributionStrength::StatisticalSuspicion {
            return schedule("statistical suspicion ranks a candidate but cannot justify mutation");
        }

        match request.target {
            CorrectionTarget::UnusualInput { reason } => {
                if request.evidence.verified_episodes >= 1 {
                    CorrectionDecision {
                        action: CorrectionAction::RecordOnly {
                            reason: reason.clone(),
                        },
                        rationale: format!("record only: {reason}"),
                    }
                } else {
                    schedule("record-only classification still needs an observed episode")
                }
            }
            CorrectionTarget::Assumption { key, replacement } => {
                if request.attribution.actionable() && request.evidence.verified_episodes >= 1 {
                    CorrectionDecision {
                        action: CorrectionAction::FixAssumption { key, replacement },
                        rationale:
                            "failure was attributed to context, so procedure knowledge is unchanged"
                                .into(),
                    }
                } else {
                    schedule("assumption correction needs strong attribution and one episode")
                }
            }
            CorrectionTarget::ProcedureScope {
                procedure_id,
                expected_version,
                condition,
                learned_from,
            } => {
                if request.attribution.actionable()
                    && request.evidence.verified_episodes >= 1
                    && request.evidence.has_strong_tier()
                    && condition.check.is_some()
                    && !condition.description.trim().is_empty()
                {
                    CorrectionDecision {
                        action: CorrectionAction::NarrowScope {
                            procedure_id,
                            expected_version,
                            condition,
                            learned_from,
                        },
                        rationale: "a strong scoped counterexample justifies the narrow additive correction"
                            .into(),
                    }
                } else {
                    schedule(
                        "scope narrowing needs an executable condition, strong attribution, and Hard or Consensus evidence",
                    )
                }
            }
            CorrectionTarget::ProcedureReplacement {
                incumbent_id,
                incumbent_version,
                challenger,
            } => {
                if request.attribution.broadly_actionable()
                    && request.evidence.verified_episodes >= PROCEDURE_REPLACEMENT_EPISODES
                    && request.evidence.has_strong_tier()
                    && request.evidence.challenger_beats_incumbent
                    && challenger.id == incumbent_id
                {
                    CorrectionDecision {
                        action: CorrectionAction::ReplaceProcedure {
                            incumbent_id,
                            incumbent_version,
                            challenger,
                        },
                        rationale:
                            "the challenger beat the incumbent after several verified failures"
                                .into(),
                    }
                } else {
                    schedule(
                        "replacement needs three verified failures and a challenger that beats the incumbent",
                    )
                }
            }
            CorrectionTarget::ConceptRevision {
                concept_id,
                expected_version,
                revised_description,
            } => {
                if request.attribution.broadly_actionable()
                    && request.evidence.verified_episodes >= CONCEPT_REVISION_EPISODES
                    && request.evidence.distinct_sources >= CONCEPT_REVISION_SOURCES
                    && request.evidence.corroborated
                    && request.evidence.offline
                    && request.evidence.has_strong_tier()
                {
                    CorrectionDecision {
                        action: CorrectionAction::ReviseConceptOffline {
                            concept_id,
                            expected_version,
                            revised_description,
                            supporting_episodes: request.evidence.verified_episodes,
                        },
                        rationale: "highly corroborated offline evidence permits concept revision"
                            .into(),
                    }
                } else {
                    schedule(
                        "concept revision needs five verified episodes, two sources, corroboration, and offline review",
                    )
                }
            }
        }
    }
}

fn schedule(reason: impl Into<String>) -> CorrectionDecision {
    let reason = reason.into();
    CorrectionDecision {
        action: CorrectionAction::ScheduleTest {
            reason: reason.clone(),
        },
        rationale: reason,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApplyOutcome {
    NoGraphChange,
    ProcedureUpdated {
        procedure_id: ProcedureId,
        previous_version: u32,
        current_version: u32,
    },
    ConceptUpdated {
        concept_id: ConceptId,
    },
}

pub struct CorrectionApplier;

/// Opaque proof that a concrete persisted failure authorized one exact local
/// graph mutation. Its fields are private so callers cannot mint authority by
/// constructing planning values.
#[derive(Debug)]
pub struct AuthorizedCorrection {
    action: AuthorizedAction,
}

#[derive(Debug)]
enum AuthorizedAction {
    NarrowScope {
        procedure_id: ProcedureId,
        expected_version: u32,
        condition: Condition,
        learned_from: EpisodeId,
    },
}

pub struct MutationAuthorizer;

impl MutationAuthorizer {
    pub fn authorize(
        episodes: &EpisodeStore,
        decision: &CorrectionDecision,
        attribution: &Attribution,
    ) -> Result<AuthorizedCorrection> {
        Self::authorize_inner(episodes, None, decision, attribution, None)
    }

    /// Authorize against the exact procedure revision named by the plan.
    ///
    /// Execution traces intentionally store canonical positional arguments.
    /// Supplying the procedure revision lets the trust boundary reconstruct
    /// the same named environment used by the evaluator without trusting
    /// caller-provided episode context.
    pub fn authorize_for_procedure(
        episodes: &EpisodeStore,
        procedure: &Procedure,
        decision: &CorrectionDecision,
        attribution: &Attribution,
    ) -> Result<AuthorizedCorrection> {
        Self::authorize_inner(episodes, Some(procedure), decision, attribution, None)
    }

    /// Engine boundary variant. Strong regression rows are usable only when
    /// the Engine has independently authenticated their exact immutable bytes.
    pub fn authorize_for_procedure_with_trusted_regressions(
        episodes: &EpisodeStore,
        procedure: &Procedure,
        decision: &CorrectionDecision,
        attribution: &Attribution,
        trusted_regression_episodes: &HashSet<EpisodeId>,
    ) -> Result<AuthorizedCorrection> {
        Self::authorize_inner(
            episodes,
            Some(procedure),
            decision,
            attribution,
            Some(trusted_regression_episodes),
        )
    }

    fn authorize_inner(
        episodes: &EpisodeStore,
        procedure: Option<&Procedure>,
        decision: &CorrectionDecision,
        attribution: &Attribution,
        trusted_regression_episodes: Option<&HashSet<EpisodeId>>,
    ) -> Result<AuthorizedCorrection> {
        match &decision.action {
            CorrectionAction::NarrowScope {
                procedure_id,
                expected_version,
                condition,
                learned_from,
            } => {
                validate_attribution(attribution, *procedure_id, *expected_version, *learned_from)?;
                let episode = episodes.get(*learned_from)?;
                let evaluation = episode.evaluation.as_ref().ok_or_else(|| {
                    AdaptError::Unauthorized("evidence episode has no evaluation".into())
                })?;
                if evaluation.success
                    || !matches!(
                        evaluation.tier,
                        VerifiabilityTier::Hard | VerifiabilityTier::Consensus
                    )
                {
                    return Err(AdaptError::Unauthorized(
                        "scope correction requires a failed Hard or Consensus episode".into(),
                    ));
                }
                if !trace_supports_contract_attribution(
                    episode.execution_trace.as_ref(),
                    attribution,
                ) {
                    return Err(AdaptError::Unauthorized(
                        "stored episode trace does not demonstrate the attributed contract violation"
                            .into(),
                    ));
                }
                let failed_trace = decode_trace(&episode).ok_or_else(|| {
                    AdaptError::Unauthorized(
                        "stored episode trace cannot be evaluated for scope narrowing".into(),
                    )
                })?;
                let failed_step = failed_trace
                    .steps
                    .get(attribution.suspect.trace_step)
                    .ok_or_else(|| {
                        AdaptError::Unauthorized(
                            "attributed trace step is unavailable for scope narrowing".into(),
                        )
                    })?;
                if let Some(procedure) = procedure
                    && (procedure.id != *procedure_id || procedure.version != *expected_version)
                {
                    return Err(AdaptError::Unauthorized(
                        "procedure revision does not match the correction target".into(),
                    ));
                }
                let params = procedure.map(|procedure| procedure.params.as_slice());
                match evaluate_scope_condition(condition, &episode, failed_step, params) {
                    Ok(false) => {}
                    Ok(true) => {
                        return Err(AdaptError::Unauthorized(
                            "scope condition does not exclude the failed trace input".into(),
                        ));
                    }
                    Err(reason) => {
                        return Err(AdaptError::Unauthorized(format!(
                            "scope condition cannot be evaluated against the failed trace input: {reason}"
                        )));
                    }
                }
                if !has_admitted_canonical_regression(
                    episodes,
                    *procedure_id,
                    *expected_version,
                    condition,
                    params,
                    trusted_regression_episodes,
                )? {
                    return Err(AdaptError::Unauthorized(
                        "scope correction requires a successful Hard or Consensus regression episode for the same procedure version that the condition admits"
                            .into(),
                    ));
                }
                Ok(AuthorizedCorrection {
                    action: AuthorizedAction::NarrowScope {
                        procedure_id: *procedure_id,
                        expected_version: *expected_version,
                        condition: condition.clone(),
                        learned_from: *learned_from,
                    },
                })
            }
            CorrectionAction::ReplaceProcedure { .. } => {
                Err(AdaptError::OfflineCapabilityRequired(
                    "procedure replacement needs a trusted regression-suite capability".into(),
                ))
            }
            CorrectionAction::ReviseConceptOffline { .. } => {
                Err(AdaptError::OfflineCapabilityRequired(
                    "concept revision needs an engine-issued quiescence capability".into(),
                ))
            }
            CorrectionAction::RecordOnly { .. }
            | CorrectionAction::FixAssumption { .. }
            | CorrectionAction::ScheduleTest { .. } => Err(AdaptError::Unauthorized(
                "the decision does not contain a graph mutation".into(),
            )),
        }
    }
}

fn validate_attribution(
    attribution: &Attribution,
    procedure_id: ProcedureId,
    expected_version: u32,
    learned_from: EpisodeId,
) -> Result<()> {
    if attribution.suspect.procedure != procedure_id
        || attribution.suspect.version != expected_version
        || !attribution.provenance.episode_ids.contains(&learned_from)
    {
        return Err(AdaptError::Unauthorized(
            "attribution provenance does not match the correction target".into(),
        ));
    }
    match AttributionStrength::from(attribution) {
        AttributionStrength::ContractViolation => {
            if !attribution.evidence.iter().any(|evidence| {
                matches!(
                    evidence,
                    AttributionEvidence::Contract {
                        status: ConditionCheckStatus::Violated,
                        ..
                    }
                )
            }) {
                return Err(AdaptError::Unauthorized(
                    "contract attribution lacks a concrete violated check".into(),
                ));
            }
        }
        AttributionStrength::ReplayConfirmed | AttributionStrength::SimulatedEvidence => {
            return Err(AdaptError::Unauthorized(
                "replay mutation needs a trusted replay receipt capability".into(),
            ));
        }
        AttributionStrength::StatisticalSuspicion | AttributionStrength::InsufficientEvidence => {
            return Err(AdaptError::Unauthorized(
                "attribution strength cannot authorize mutation".into(),
            ));
        }
    }
    Ok(())
}

fn trace_supports_contract_attribution(
    trace: Option<&serde_json::Value>,
    attribution: &Attribution,
) -> bool {
    let Some(trace) =
        trace.and_then(|value| serde_json::from_value::<ExecTrace>(value.clone()).ok())
    else {
        return false;
    };
    let Some(step) = trace.steps.get(attribution.suspect.trace_step) else {
        return false;
    };
    if step.procedure_called != Some(attribution.suspect.procedure)
        || step.procedure_version != Some(attribution.suspect.version)
    {
        return false;
    }
    attribution.evidence.iter().any(|evidence| {
        let AttributionEvidence::Contract {
            section,
            description,
            status: ConditionCheckStatus::Violated,
        } = evidence
        else {
            return false;
        };
        let checks = match section {
            ContractSection::Requires => &step.contract_checks.requires,
            ContractSection::Promises => &step.contract_checks.promises,
            ContractSection::FailsWhen => &step.contract_checks.fails_when,
        };
        checks.iter().any(|check| {
            check
                == &ConditionCheck {
                    description: description.clone(),
                    status: ConditionCheckStatus::Violated,
                }
        })
    })
}

fn decode_trace(episode: &Episode) -> Option<ExecTrace> {
    episode
        .execution_trace
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

fn evaluate_scope_condition(
    condition: &Condition,
    episode: &Episode,
    step: &ExecStep,
    params: Option<&[Param]>,
) -> std::result::Result<bool, String> {
    let check = condition
        .check
        .as_ref()
        .ok_or_else(|| "condition has no executable check".to_string())?;
    let args = match step.input.as_ref() {
        None => Vec::new(),
        Some(Value::List(values)) => values.clone(),
        Some(_) => return Err("trace input is not a canonical argument list".into()),
    };

    let mut env = Env::new();
    let canonical_input = Value::List(args.clone());
    env.set("input", canonical_input.clone());
    env.set("args", canonical_input);
    for (index, value) in args.iter().enumerate() {
        env.set(format!("arg{index}"), value.clone());
    }
    if let Some(params) = params {
        if params.len() != args.len() {
            return Err(format!(
                "trace has {} arguments but procedure revision declares {}",
                args.len(),
                params.len()
            ));
        }
        for (param, value) in params.iter().zip(&args) {
            env.set(param.name.clone(), value.clone());
        }
    }
    let mut unmatched_args = args.clone();
    for (name, value) in &episode.context.environment {
        if let Some(index) = unmatched_args.iter().position(|argument| argument == value) {
            env.set(name.clone(), value.clone());
            unmatched_args.remove(index);
        }
    }

    let value = Evaluator::new()
        .with_budget(10_000)
        .eval(check, &mut env)
        .map_err(|error| error.to_string())?;
    value
        .as_bool()
        .ok_or_else(|| format!("condition returned {}, not bool", value.type_name()))
}

fn has_admitted_canonical_regression(
    episodes: &EpisodeStore,
    procedure_id: ProcedureId,
    expected_version: u32,
    condition: &Condition,
    params: Option<&[Param]>,
    trusted_regression_episodes: Option<&HashSet<EpisodeId>>,
) -> Result<bool> {
    for episode in episodes.list_recent(u32::MAX)? {
        if trusted_regression_episodes.is_some_and(|trusted| !trusted.contains(&episode.id)) {
            continue;
        }
        let Some(evaluation) = episode.evaluation.as_ref() else {
            continue;
        };
        if !evaluation.success
            || !matches!(
                evaluation.tier,
                VerifiabilityTier::Hard | VerifiabilityTier::Consensus
            )
        {
            continue;
        }
        let Some(trace) = decode_trace(&episode) else {
            continue;
        };
        for step in &trace.steps {
            if step.procedure_called != Some(procedure_id)
                || step.procedure_version != Some(expected_version)
                || !matches!(step.status, ExecStepStatus::Succeeded)
            {
                continue;
            }
            if matches!(
                evaluate_scope_condition(condition, &episode, step, params),
                Ok(true)
            ) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

impl CorrectionApplier {
    pub fn apply(
        graph: &KnowledgeStore,
        authorization: &AuthorizedCorrection,
        updated_at: i64,
    ) -> Result<ApplyOutcome> {
        match &authorization.action {
            AuthorizedAction::NarrowScope {
                procedure_id,
                expected_version,
                condition,
                learned_from,
            } => {
                let mut procedure = graph
                    .get_procedure(*procedure_id)?
                    .ok_or_else(|| AdaptError::NotFound(format!("procedure {procedure_id}")))?;
                if procedure.version != *expected_version {
                    return Err(spoon_graph::GraphError::RevisionConflict {
                        entity: format!("procedure {procedure_id}"),
                        expected: *expected_version,
                        actual: procedure.version,
                    }
                    .into());
                }
                if let Some(concept_id) = procedure.concept {
                    let concept = graph
                        .get_concept(concept_id)?
                        .ok_or_else(|| AdaptError::NotFound(format!("concept {concept_id}")))?;
                    if matches!(
                        concept.mutability,
                        spoon_core::MutabilityClass::Definitional
                            | spoon_core::MutabilityClass::Normative
                            | spoon_core::MutabilityClass::CoreMachinery
                    ) {
                        return Err(AdaptError::Unauthorized(format!(
                            "learning cannot revise {:?} knowledge",
                            concept.mutability
                        )));
                    }
                }
                if let Some(existing) = procedure
                    .contract
                    .requires
                    .iter()
                    .find(|existing| existing.description == condition.description)
                {
                    if existing.check == condition.check {
                        return Ok(ApplyOutcome::NoGraphChange);
                    }
                    return Err(AdaptError::Invalid(format!(
                        "condition description conflict: {:?} already names a different executable condition",
                        condition.description
                    )));
                }
                let previous_version = *expected_version;
                procedure.contract.requires.push(condition.clone());
                procedure.contract.confidence.scope.push(ScopeCondition {
                    description: condition.description.clone(),
                    learned_from: Some(*learned_from),
                });
                procedure.version = expected_version.saturating_add(1);
                procedure.updated_at = updated_at;
                graph.revise_procedure(&procedure, previous_version)?;
                Ok(ApplyOutcome::ProcedureUpdated {
                    procedure_id: *procedure_id,
                    previous_version,
                    current_version: procedure.version,
                })
            }
        }
    }
}
