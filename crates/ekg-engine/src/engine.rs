use std::collections::{BTreeMap, HashSet};

use ekg_core::{
    ContractCheckResult, EkgError, Episode, EpisodeCost, EpisodeId, EscalationRung, Evaluation,
    Procedure, ProcedureId, ReasoningTrace, TraceStep, TraceStepStatus, Value, VerifiabilityTier,
};
use ekg_episode::EpisodeStore;
use ekg_exec::{ConditionCheckStatus, Evaluator, ExecStepStatus, ExecTrace};
use ekg_graph::{GraphError, KnowledgeStore};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::evaluate_deterministic;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error(transparent)]
    Core(#[from] EkgError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error("{0}")]
    InvalidInput(String),
    #[error("episode {0} has no replayable execution trace")]
    MissingTrace(EpisodeId),
    #[error("trace does not identify a top-level procedure")]
    MissingTopLevelProcedure,
    #[error("execution failed in episode {episode_id}: {source}")]
    ExecutionFailed {
        episode_id: EpisodeId,
        #[source]
        source: EkgError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutcome {
    pub value: Value,
    pub trace: ExecTrace,
    pub episode: Episode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayOutcome {
    pub value: Value,
    pub trace: ExecTrace,
    pub source_episode: EpisodeId,
}

/// Phase 0 orchestration boundary. It owns the graph and episode stores and
/// creates a fresh bounded evaluator for each run so execution state cannot
/// leak across episodes.
pub struct Engine {
    graph: KnowledgeStore,
    episodes: EpisodeStore,
    max_steps: u32,
}

impl Engine {
    pub fn open(path: &str) -> Result<Self, EngineError> {
        Ok(Self {
            graph: KnowledgeStore::new(path)?,
            episodes: EpisodeStore::new(path)?,
            max_steps: 1_000_000,
        })
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        Ok(Self {
            graph: KnowledgeStore::in_memory()?,
            episodes: EpisodeStore::in_memory()?,
            max_steps: 1_000_000,
        })
    }

    pub fn with_max_steps(mut self, max_steps: u32) -> Self {
        self.max_steps = max_steps;
        self
    }

    pub fn graph(&self) -> &KnowledgeStore {
        &self.graph
    }

    pub fn episodes(&self) -> &EpisodeStore {
        &self.episodes
    }

    pub fn execute_procedure(
        &self,
        procedure_id: ProcedureId,
        inputs: BTreeMap<String, Value>,
        prediction: Option<Value>,
    ) -> Result<ExecutionOutcome, EngineError> {
        let procedure = self
            .graph
            .get_procedure(procedure_id)?
            .ok_or_else(|| EkgError::NotFound(format!("procedure {procedure_id}")))?;
        let args = bind_inputs(&procedure, &inputs, None)?;
        let mut evaluator = self.current_evaluator()?;
        let attempt = evaluator.exec_procedure_captured(&procedure_id, args);
        let steps_used = evaluator.budget().steps_used;
        match attempt.result {
            Ok(value) => {
                let episode = self.record_execution(
                    &procedure,
                    prediction,
                    Some(value.clone()),
                    &attempt.trace,
                    None,
                    steps_used,
                )?;
                Ok(ExecutionOutcome {
                    value,
                    trace: attempt.trace,
                    episode,
                })
            }
            Err(source) => {
                let episode = self.record_execution(
                    &procedure,
                    prediction,
                    None,
                    &attempt.trace,
                    Some(&source),
                    steps_used,
                )?;
                Err(EngineError::ExecutionFailed {
                    episode_id: episode.id,
                    source,
                })
            }
        }
    }

    pub fn replay_episode(
        &self,
        episode_id: EpisodeId,
        substitutions: BTreeMap<String, Value>,
    ) -> Result<ReplayOutcome, EngineError> {
        let episode = self.episodes.get(episode_id)?;
        let trace_json = episode
            .execution_trace
            .ok_or(EngineError::MissingTrace(episode_id))?;
        let trace: ExecTrace = serde_json::from_value(trace_json)?;
        let top = trace
            .steps
            .last()
            .ok_or(EngineError::MissingTopLevelProcedure)?;
        let procedure_id = top
            .procedure_called
            .ok_or(EngineError::MissingTopLevelProcedure)?;
        let version = top
            .procedure_version
            .ok_or(EngineError::MissingTopLevelProcedure)?;
        let procedure = self
            .graph
            .get_procedure_version(procedure_id, version)?
            .ok_or_else(|| EkgError::NotFound(format!("procedure {procedure_id} v{version}")))?;
        let original = top.input.as_ref().and_then(Value::as_list);
        let args = bind_inputs(&procedure, &substitutions, original)?;

        let mut evaluator = Evaluator::new().with_budget(self.max_steps);
        let mut registered = HashSet::new();
        for step in &trace.steps {
            let (Some(id), Some(version)) = (step.procedure_called, step.procedure_version) else {
                continue;
            };
            if registered.insert(id) {
                let exact = self
                    .graph
                    .get_procedure_version(id, version)?
                    .ok_or_else(|| EkgError::NotFound(format!("procedure {id} v{version}")))?;
                evaluator.register_procedure(exact);
            }
        }

        let replayed = evaluator.replay(&trace, args)?;
        Ok(ReplayOutcome {
            value: replayed.value,
            trace: replayed.trace,
            source_episode: episode_id,
        })
    }

    fn current_evaluator(&self) -> Result<Evaluator, EngineError> {
        let mut evaluator = Evaluator::new().with_budget(self.max_steps);
        for procedure in self.graph.list_procedures()? {
            evaluator.register_procedure(procedure);
        }
        Ok(evaluator)
    }

    fn record_execution(
        &self,
        procedure: &Procedure,
        prediction: Option<Value>,
        observed: Option<Value>,
        trace: &ExecTrace,
        failure: Option<&EkgError>,
        steps_used: u32,
    ) -> Result<Episode, EngineError> {
        let mut episode = Episode::new(format!("execute {}", procedure.name));
        if let Some(concept) = procedure.concept {
            episode.context.entities.push(concept);
        }
        episode.prediction = prediction.clone();
        episode.action = Some(format!("procedure:{}@{}", procedure.id, procedure.version));
        episode.observed_result = observed.clone();
        episode.evaluation = match (failure, prediction.as_ref(), observed.as_ref()) {
            (Some(error), _, _) => Some(Evaluation {
                tier: VerifiabilityTier::Hard,
                success: false,
                details: error.to_string(),
                surprise: prediction.as_ref().map(|_| 1.0),
            }),
            (None, Some(expected), Some(actual)) => Some(evaluate_deterministic(expected, actual)),
            _ => None,
        };
        episode.reasoning_trace = reasoning_trace(trace);
        episode.execution_trace = Some(serde_json::to_value(trace)?);
        episode.cost = EpisodeCost {
            rung_reached: EscalationRung::Run,
            steps_taken: trace.len() as u32,
            budget_spent: f64::from(steps_used),
        };
        self.episodes.insert(&episode)?;
        Ok(episode)
    }
}

fn bind_inputs(
    procedure: &Procedure,
    supplied: &BTreeMap<String, Value>,
    defaults: Option<&[Value]>,
) -> Result<Vec<Value>, EngineError> {
    let known: HashSet<&str> = procedure
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    if let Some(extra) = supplied.keys().find(|name| !known.contains(name.as_str())) {
        return Err(EngineError::InvalidInput(format!(
            "unexpected input '{extra}'"
        )));
    }

    procedure
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            supplied
                .get(&param.name)
                .cloned()
                .or_else(|| defaults.and_then(|values| values.get(index)).cloned())
                .ok_or_else(|| EngineError::InvalidInput(format!("missing input '{}'", param.name)))
        })
        .collect()
}

fn reasoning_trace(trace: &ExecTrace) -> ReasoningTrace {
    ReasoningTrace {
        steps: trace
            .steps
            .iter()
            .map(|step| {
                let requires_violations: Vec<String> = step
                    .contract_checks
                    .requires
                    .iter()
                    .filter(|check| check.status == ConditionCheckStatus::Violated)
                    .map(|check| check.description.clone())
                    .collect();
                let promise_violations: Vec<String> = step
                    .contract_checks
                    .promises
                    .iter()
                    .filter(|check| check.status == ConditionCheckStatus::Violated)
                    .map(|check| check.description.clone())
                    .collect();
                let failure_conditions: Vec<String> = step
                    .contract_checks
                    .fails_when
                    .iter()
                    .filter(|check| check.status == ConditionCheckStatus::Violated)
                    .map(|check| check.description.clone())
                    .collect();
                let violations = requires_violations
                    .iter()
                    .chain(promise_violations.iter())
                    .chain(failure_conditions.iter())
                    .cloned()
                    .collect();
                let (status, output) = match &step.status {
                    ExecStepStatus::Succeeded => {
                        (TraceStepStatus::Succeeded, Some(step.output.clone()))
                    }
                    ExecStepStatus::Failed { error } => (
                        TraceStepStatus::Failed {
                            error: error.clone(),
                        },
                        None,
                    ),
                };
                TraceStep {
                    description: step.expr_description.clone(),
                    procedure_used: step.procedure_called,
                    contract_check: Some(ContractCheckResult {
                        all_requires_met: requires_violations.is_empty(),
                        all_promises_met: promise_violations.is_empty(),
                        no_failure_conditions_met: failure_conditions.is_empty(),
                        violations,
                    }),
                    input: step.input.clone(),
                    output,
                    rung: EscalationRung::Run,
                    status,
                }
            })
            .collect(),
    }
}
