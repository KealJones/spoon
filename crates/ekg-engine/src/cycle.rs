use std::cmp::Ordering;
use std::collections::BTreeMap;

use ekg_core::{
    Assumption, Concept, ConceptId, Episode, EpisodeCost, EscalationRung, Evaluation,
    KnowledgeCandidate, Procedure, ReasoningTrace, TraceStep, TraceStepStatus, Value,
    VerifiabilityTier,
};
use ekg_exec::ExecTrace;
use ekg_reason::{
    ContextAssembler, ContextConfig, ContextRequest, InterpretationCandidate, InterpretationSet,
    RemainingBudget,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::engine::{Engine, EngineError, bind_inputs, reasoning_trace};

const MAX_TEACHER_CONTEXT_ITEMS: usize = 64;
const MAX_TEACHER_TEXT_CHARS: usize = 2_048;
const MAX_TEACHER_VALUE_DEPTH: usize = 8;
const MAX_TEACHER_CONTEXT_NODES: usize = 8_192;
const MAX_TEACHER_CONTEXT_CHARS: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CycleId(pub Uuid);

impl CycleId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl std::fmt::Display for CycleId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleBudget {
    pub max_exec_steps: u32,
    pub max_context_items: usize,
    pub max_teacher_turns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleInput {
    pub situation: String,
    pub environment: BTreeMap<String, Value>,
    pub assumptions: Vec<Assumption>,
    pub budget: CycleBudget,
    pub teacher_allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleDisposition {
    Verified,
    Provisional,
    Abstained,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleOutcome {
    pub cycle_id: CycleId,
    pub disposition: CycleDisposition,
    pub answer: Option<Value>,
    pub episode: Episode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeacherRequestWire {
    pub situation: String,
    pub context: JsonValue,
    pub specific_question: Option<String>,
    pub desired_output: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeacherProposalWire {
    pub content: JsonValue,
    pub source: String,
    pub status: String,
    pub provenance: JsonValue,
    #[serde(default)]
    pub validation: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CycleProgress {
    NeedTeacher {
        cycle_id: CycleId,
        request: TeacherRequestWire,
    },
    Completed(Box<CycleOutcome>),
}

#[derive(Debug, Clone)]
pub(crate) struct PendingCycle {
    input: CycleInput,
    request: TeacherRequestWire,
    initial_interpretations: Vec<ResolvedInterpretation>,
    prior_failure: Option<JsonValue>,
}

#[derive(Debug, Clone)]
struct ResolvedInterpretation {
    concept: Concept,
    weight: f64,
    inputs: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct ProposalContent {
    #[serde(default)]
    interpretations: Vec<ProposalInterpretation>,
    #[serde(default, deserialize_with = "deserialize_optional_procedure")]
    procedure: Option<Procedure>,
    #[serde(default)]
    answer: Option<Value>,
    #[serde(default, rename = "abstainReason")]
    abstain_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProposalInterpretation {
    concept: ProposalConcept,
    weight: f64,
    #[serde(default, deserialize_with = "deserialize_inputs")]
    inputs: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct NamedInput {
    name: String,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProposalConcept {
    Id { id: String },
    Name { name: String },
    Bare(String),
}

impl Engine {
    pub fn begin_cycle(&mut self, input: CycleInput) -> Result<CycleProgress, EngineError> {
        validate_cycle_input(&input)?;
        let cycle_id = CycleId::new();

        if let Some(answer) = self.recall(&input.situation)? {
            return self.complete_simple(
                cycle_id,
                &input,
                CycleDisposition::Verified,
                Some(answer.clone()),
                "recall",
                EscalationRung::Recall,
                Some(Evaluation {
                    tier: VerifiabilityTier::Hard,
                    success: true,
                    details: "recalled an exact previously verified result".into(),
                    surprise: None,
                }),
                None,
                Vec::new(),
            );
        }

        let interpretations = self.local_interpretations(&input.situation)?;
        if let Some(resolved) = uniquely_resolved(&interpretations)
            && let Some(procedure) = self.procedure_for(resolved.concept.id)?
            && let Some(inputs) = complete_inputs(
                &procedure,
                &input.environment,
                &resolved.inputs,
                &extract_literals(&input.situation),
            )
        {
            return self.execute_cycle_procedure(
                cycle_id,
                &input,
                &interpretations,
                &procedure,
                inputs,
                EscalationRung::Run,
                None,
                false,
                None,
            );
        }

        if input.teacher_allowed && input.budget.max_teacher_turns > 0 {
            let request = self.teacher_request(&input, &interpretations)?;
            let mut pending_input = input;
            pending_input.budget.max_teacher_turns =
                pending_input.budget.max_teacher_turns.saturating_sub(1);
            self.pending_cycles.insert(
                cycle_id,
                PendingCycle {
                    input: pending_input,
                    request: request.clone(),
                    initial_interpretations: interpretations,
                    prior_failure: None,
                },
            );
            return Ok(CycleProgress::NeedTeacher { cycle_id, request });
        }

        self.complete_simple(
            cycle_id,
            &input,
            CycleDisposition::Abstained,
            None,
            "abstain",
            EscalationRung::Abstain,
            None,
            None,
            interpretations,
        )
    }

    pub fn resume_cycle(
        &mut self,
        cycle_id: CycleId,
        proposal: TeacherProposalWire,
    ) -> Result<CycleProgress, EngineError> {
        let pending = self.pending_cycles.remove(&cycle_id).ok_or_else(|| {
            EngineError::InvalidInput(format!("cycle {cycle_id} is unknown or already consumed"))
        })?;
        let teacher_json = json!({
            "request": pending.request,
            "proposal": proposal,
            "priorFailure": pending.prior_failure,
        });

        if proposal.status != "unverified"
            || !valid_teacher_provenance(&proposal, &pending.input.situation)
            || proposal.validation.as_ref().is_some_and(|validation| {
                !matches!(
                    validation.get("status").and_then(JsonValue::as_str),
                    Some("verified" | "provisional")
                )
            })
        {
            return self.complete_simple(
                cycle_id,
                &pending.input,
                CycleDisposition::Abstained,
                None,
                "abstain:invalid-teacher-status",
                EscalationRung::Abstain,
                None,
                Some(teacher_json),
                pending.initial_interpretations,
            );
        }

        let content: ProposalContent = match serde_json::from_value(proposal.content.clone()) {
            Ok(content) => content,
            Err(_) => {
                return self.complete_simple(
                    cycle_id,
                    &pending.input,
                    CycleDisposition::Abstained,
                    None,
                    "abstain:invalid-teacher-proposal",
                    EscalationRung::Abstain,
                    None,
                    Some(teacher_json),
                    pending.initial_interpretations,
                );
            }
        };
        let interpretations = match self.resolve_teacher_interpretations(&content.interpretations) {
            Ok(values) => values,
            Err(_) => {
                return self.complete_simple(
                    cycle_id,
                    &pending.input,
                    CycleDisposition::Abstained,
                    None,
                    "abstain:rejected-teacher-proposal",
                    EscalationRung::Abstain,
                    None,
                    Some(teacher_json),
                    pending.initial_interpretations,
                );
            }
        };

        let expected_answer = content.answer.clone();
        if let Some(mut procedure) = content.procedure {
            // A teacher may propose executable knowledge, but successful
            // execution proves only that it runs—not that it means what the
            // teacher claims. Promotion belongs to later evidence gates.
            procedure.lifecycle = ekg_core::Lifecycle::Provisional;
            if procedure.concept.is_none() {
                procedure.concept = uniquely_resolved(&interpretations).map(|item| item.concept.id);
            }
            let teacher_inputs = uniquely_resolved(&interpretations)
                .map(|item| item.inputs.clone())
                .unwrap_or_default();
            if let Some(inputs) = complete_inputs(
                &procedure,
                &pending.input.environment,
                &teacher_inputs,
                &extract_literals(&pending.input.situation),
            ) {
                return self.execute_cycle_procedure(
                    cycle_id,
                    &pending.input,
                    &interpretations,
                    &procedure,
                    inputs,
                    EscalationRung::Ask,
                    Some(teacher_json),
                    true,
                    expected_answer.clone(),
                );
            }
        }

        if let Some(resolved) = uniquely_resolved(&interpretations)
            && let Some(procedure) = self.procedure_for(resolved.concept.id)?
            && let Some(inputs) = complete_inputs(
                &procedure,
                &pending.input.environment,
                &resolved.inputs,
                &extract_literals(&pending.input.situation),
            )
        {
            return self.execute_cycle_procedure(
                cycle_id,
                &pending.input,
                &interpretations,
                &procedure,
                inputs,
                EscalationRung::Ask,
                Some(teacher_json),
                false,
                expected_answer,
            );
        }

        if let Some(answer) = content.answer {
            return self.complete_simple(
                cycle_id,
                &pending.input,
                CycleDisposition::Provisional,
                Some(answer),
                "teacher-answer:provisional",
                EscalationRung::Ask,
                None,
                Some(teacher_json),
                interpretations,
            );
        }

        let action = content
            .abstain_reason
            .map(|reason| format!("abstain:{reason}"))
            .unwrap_or_else(|| "abstain:teacher-could-not-resolve".into());
        self.complete_simple(
            cycle_id,
            &pending.input,
            CycleDisposition::Abstained,
            None,
            &action,
            EscalationRung::Abstain,
            None,
            Some(teacher_json),
            interpretations,
        )
    }

    pub fn abort_cycle(
        &mut self,
        cycle_id: CycleId,
        reason: impl Into<String>,
    ) -> Result<CycleProgress, EngineError> {
        let pending = self.pending_cycles.remove(&cycle_id).ok_or_else(|| {
            EngineError::InvalidInput(format!("cycle {cycle_id} is unknown or already consumed"))
        })?;
        let reason = truncate_text(&reason.into(), MAX_TEACHER_TEXT_CHARS);
        let interaction = json!({
            "request": pending.request,
            "providerError": reason,
            "priorFailure": pending.prior_failure,
        });
        self.complete_simple(
            cycle_id,
            &pending.input,
            CycleDisposition::Abstained,
            None,
            "abstain:teacher-provider-failure",
            EscalationRung::Abstain,
            None,
            Some(interaction),
            pending.initial_interpretations,
        )
    }

    fn recall(&self, situation: &str) -> Result<Option<Value>, EngineError> {
        Ok(self
            .episodes
            .list_recent(u32::MAX)?
            .into_iter()
            .find(|episode| {
                episode.situation == situation
                    && episode.evaluation.as_ref().is_some_and(|evaluation| {
                        evaluation.success
                            && matches!(
                                evaluation.tier,
                                VerifiabilityTier::Hard | VerifiabilityTier::Consensus
                            )
                    })
                    && episode.observed_result.is_some()
            })
            .and_then(|episode| episode.observed_result))
    }

    fn local_interpretations(
        &self,
        situation: &str,
    ) -> Result<Vec<ResolvedInterpretation>, EngineError> {
        let lowered = situation.to_lowercase();
        let mut matches = self
            .graph
            .list_concepts()?
            .into_iter()
            .filter(|concept| {
                usable_lifecycle(concept.lifecycle)
                    && contains_term(&lowered, &concept.name.to_lowercase())
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| left.name.cmp(&right.name));
        let weight = if matches.is_empty() {
            0.0
        } else {
            1.0 / matches.len() as f64
        };
        Ok(matches
            .into_iter()
            .map(|concept| ResolvedInterpretation {
                concept,
                weight,
                inputs: BTreeMap::new(),
            })
            .collect())
    }

    fn resolve_teacher_interpretations(
        &self,
        proposed: &[ProposalInterpretation],
    ) -> Result<Vec<ResolvedInterpretation>, EngineError> {
        if proposed.is_empty() {
            return Ok(Vec::new());
        }
        let mut resolved = Vec::with_capacity(proposed.len());
        for item in proposed {
            let concept = match &item.concept {
                ProposalConcept::Id { id } => {
                    let uuid = Uuid::parse_str(id).map_err(|_| {
                        EngineError::InvalidInput(format!("invalid concept id '{id}'"))
                    })?;
                    self.graph.get_concept(ConceptId(uuid))?
                }
                ProposalConcept::Name { name } | ProposalConcept::Bare(name) => {
                    self.graph.get_concept_by_name(name)?
                }
            }
            .ok_or_else(|| {
                EngineError::InvalidInput("teacher referenced unknown concept".into())
            })?;
            if !usable_lifecycle(concept.lifecycle) {
                return Err(EngineError::InvalidInput(
                    "teacher referenced inactive concept".into(),
                ));
            }
            resolved.push(ResolvedInterpretation {
                concept,
                weight: item.weight,
                inputs: item.inputs.clone(),
            });
        }
        let candidates = resolved
            .iter()
            .map(|item| InterpretationCandidate {
                meaning: item.concept.id,
                weight: item.weight,
            })
            .collect();
        InterpretationSet::try_new(candidates, chosen_concept(&resolved))
            .map_err(|error| EngineError::InvalidInput(error.to_string()))?;
        Ok(resolved)
    }

    fn procedure_for(&self, concept: ConceptId) -> Result<Option<Procedure>, EngineError> {
        Ok(self.graph.list_procedures()?.into_iter().find(|procedure| {
            procedure.concept == Some(concept) && usable_lifecycle(procedure.lifecycle)
        }))
    }

    fn teacher_request(
        &self,
        input: &CycleInput,
        interpretations: &[ResolvedInterpretation],
    ) -> Result<TeacherRequestWire, EngineError> {
        let item_limit = input
            .budget
            .max_context_items
            .min(MAX_TEACHER_CONTEXT_ITEMS);
        let concepts = self
            .graph
            .list_concepts()?
            .into_iter()
            .filter(|concept| usable_lifecycle(concept.lifecycle))
            .take(item_limit)
            .map(|concept| {
                json!({
                    "id": concept.id,
                    "name": truncate_text(&concept.name, MAX_TEACHER_TEXT_CHARS),
                    "description": concept.description.as_deref().map(|description| {
                        truncate_text(description, MAX_TEACHER_TEXT_CHARS)
                    }),
                    "mutability": concept.mutability,
                    "lifecycle": concept.lifecycle,
                })
            })
            .collect::<Vec<_>>();
        let procedures = self
            .graph
            .list_procedures()?
            .into_iter()
            .filter(|procedure| usable_lifecycle(procedure.lifecycle))
            .take(item_limit)
            .map(|procedure| {
                json!({
                    "id": procedure.id,
                    "name": truncate_text(&procedure.name, MAX_TEACHER_TEXT_CHARS),
                    "params": procedure.params.into_iter().take(item_limit).map(|parameter| {
                        json!({
                            "name": truncate_text(&parameter.name, MAX_TEACHER_TEXT_CHARS),
                            "description": parameter.description.as_deref().map(|description| {
                                truncate_text(description, MAX_TEACHER_TEXT_CHARS)
                            }),
                        })
                    }).collect::<Vec<_>>(),
                    "concept": procedure.concept,
                    "version": procedure.version,
                    "lifecycle": procedure.lifecycle,
                })
            })
            .collect::<Vec<_>>();
        let mut environment_nodes = MAX_TEACHER_CONTEXT_NODES;
        let mut environment_chars = MAX_TEACHER_CONTEXT_CHARS;
        let environment = input
            .environment
            .iter()
            .take(item_limit)
            .map(|(key, value)| {
                (
                    truncate_text(key, MAX_TEACHER_TEXT_CHARS),
                    bound_json(
                        serde_json::to_value(value).unwrap_or(JsonValue::Null),
                        0,
                        item_limit,
                        &mut environment_nodes,
                        &mut environment_chars,
                    ),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let assumptions = input
            .assumptions
            .iter()
            .take(item_limit)
            .map(|assumption| {
                json!({
                    "description": truncate_text(&assumption.description, MAX_TEACHER_TEXT_CHARS),
                    "basis": truncate_text(&assumption.basis, MAX_TEACHER_TEXT_CHARS),
                    "concept": assumption.concept,
                })
            })
            .collect::<Vec<_>>();
        let raw_context = json!({
            "candidateInterpretations": interpretations.iter().map(|item| json!({
                "concept": item.concept,
                "weight": item.weight,
            })).collect::<Vec<_>>(),
            "concepts": concepts,
            "procedures": procedures,
            "environment": environment,
            "assumptions": assumptions,
            "budget": input.budget,
        });
        let mut context_nodes = MAX_TEACHER_CONTEXT_NODES;
        let mut context_chars = MAX_TEACHER_CONTEXT_CHARS;
        let context = bound_json(
            raw_context,
            0,
            item_limit,
            &mut context_nodes,
            &mut context_chars,
        );
        Ok(TeacherRequestWire {
            situation: input.situation.clone(),
            context,
            specific_question: Some(
                "Return weighted interpretations and either executable procedure knowledge, an answer, or an explicit abstention."
                    .into(),
            ),
            desired_output: proposal_schema(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_cycle_procedure(
        &mut self,
        cycle_id: CycleId,
        input: &CycleInput,
        interpretations: &[ResolvedInterpretation],
        procedure: &Procedure,
        inputs: BTreeMap<String, Value>,
        rung: EscalationRung,
        teacher_interaction: Option<JsonValue>,
        learn_on_success: bool,
        expected_answer: Option<Value>,
    ) -> Result<CycleProgress, EngineError> {
        let (prior_reasoning, prior_execution_trace, prior_steps_used, prior_trace_len) =
            prior_failure_material(teacher_interaction.as_ref());
        let args = bind_inputs(procedure, &inputs, None)?;
        let mut evaluator = self
            .current_evaluator()?
            .with_budget(input.budget.max_exec_steps.min(self.max_steps));
        evaluator.register_procedure(procedure.clone());
        let attempt = evaluator.exec_procedure_captured(&procedure.id, args);
        let steps_used = evaluator.budget().steps_used;
        let mut episode = self.base_episode(input, interpretations)?;
        episode.action = Some(format!("procedure:{}@{}", procedure.id, procedure.version));
        episode.reasoning_trace = reasoning_trace(&attempt.trace);
        if rung == EscalationRung::Ask {
            for step in &mut episode.reasoning_trace.steps {
                step.rung = EscalationRung::Ask;
            }
        }
        let mut prefix = if prior_reasoning.steps.is_empty() {
            ladder_prefix(rung, rung == EscalationRung::Ask)
        } else {
            let mut steps = prior_reasoning.steps;
            steps.push(simple_step("ask teacher", EscalationRung::Ask));
            steps
        };
        prefix.append(&mut episode.reasoning_trace.steps);
        episode.reasoning_trace.steps = prefix;
        let mut cumulative_trace = prior_execution_trace
            .and_then(|trace| serde_json::from_value::<ExecTrace>(trace).ok())
            .unwrap_or_default();
        cumulative_trace
            .steps
            .extend(attempt.trace.steps.iter().cloned());
        episode.execution_trace = Some(serde_json::to_value(&cumulative_trace)?);
        episode.teacher_interaction = teacher_interaction;
        episode.cost = EpisodeCost {
            rung_reached: rung,
            steps_taken: prior_trace_len.saturating_add(attempt.trace.len() as u32),
            budget_spent: f64::from(prior_steps_used.saturating_add(steps_used))
                + if rung == EscalationRung::Ask {
                    1.0
                } else {
                    0.0
                },
        };
        match attempt.result {
            Ok(value) => {
                if expected_answer
                    .as_ref()
                    .is_some_and(|expected| expected != &value)
                {
                    episode.prediction = expected_answer;
                    episode.observed_result = Some(value);
                    episode.evaluation = Some(Evaluation {
                        tier: VerifiabilityTier::Hard,
                        success: false,
                        details: "teacher procedure contradicts its proposed answer".into(),
                        surprise: Some(1.0),
                    });
                    episode.action = Some("abstain:inconsistent-teacher-procedure".into());
                    episode.cost.rung_reached = EscalationRung::Abstain;
                    episode.reasoning_trace.steps.push(simple_step(
                        "abstain after deterministic proposal contradiction",
                        EscalationRung::Abstain,
                    ));
                    self.episodes.insert(&episode)?;
                    return Ok(CycleProgress::Completed(Box::new(CycleOutcome {
                        cycle_id,
                        disposition: CycleDisposition::Abstained,
                        answer: None,
                        episode,
                    })));
                }
                let semantic_verified = rung == EscalationRung::Run
                    && !learn_on_success
                    && matches!(
                        procedure.lifecycle,
                        ekg_core::Lifecycle::Active | ekg_core::Lifecycle::Validated
                    );
                episode.observed_result = Some(value.clone());
                episode.evaluation = Some(if semantic_verified {
                    Evaluation {
                        tier: VerifiabilityTier::Hard,
                        success: true,
                        details: "deterministic validated procedure execution completed".into(),
                        surprise: None,
                    }
                } else {
                    episode.prediction = Some(value.clone());
                    Evaluation {
                        tier: VerifiabilityTier::Deferred,
                        success: true,
                        details: "procedure executes, but its semantic fit remains provisional"
                            .into(),
                        surprise: None,
                    }
                });
                self.episodes.insert(&episode)?;
                if learn_on_success && let Err(error) = self.graph.insert_procedure(procedure) {
                    episode.action = Some("abstain:teacher-procedure-integration-failed".into());
                    episode.evaluation = Some(Evaluation {
                        tier: VerifiabilityTier::Hard,
                        success: false,
                        details: error.to_string(),
                        surprise: None,
                    });
                    episode.cost.rung_reached = EscalationRung::Abstain;
                    episode.reasoning_trace.steps.push(simple_step(
                        "abstain after procedure integration failure",
                        EscalationRung::Abstain,
                    ));
                    self.episodes.update(&episode)?;
                    return Ok(CycleProgress::Completed(Box::new(CycleOutcome {
                        cycle_id,
                        disposition: CycleDisposition::Abstained,
                        answer: None,
                        episode,
                    })));
                }
                Ok(CycleProgress::Completed(Box::new(CycleOutcome {
                    cycle_id,
                    disposition: if semantic_verified {
                        CycleDisposition::Verified
                    } else {
                        CycleDisposition::Provisional
                    },
                    answer: Some(value),
                    episode,
                })))
            }
            Err(error) => {
                if rung == EscalationRung::Run
                    && input.teacher_allowed
                    && input.budget.max_teacher_turns > 0
                {
                    let mut pending_input = input.clone();
                    pending_input.budget.max_exec_steps = pending_input
                        .budget
                        .max_exec_steps
                        .saturating_sub(steps_used);
                    let request = self.teacher_request(&pending_input, interpretations)?;
                    pending_input.budget.max_teacher_turns =
                        pending_input.budget.max_teacher_turns.saturating_sub(1);
                    self.pending_cycles.insert(
                        cycle_id,
                        PendingCycle {
                            input: pending_input,
                            request: request.clone(),
                            initial_interpretations: interpretations.to_vec(),
                            prior_failure: Some(json!({
                                "error": error.to_string(),
                                "executionTrace": attempt.trace,
                                "reasoningTrace": episode.reasoning_trace,
                                "stepsUsed": steps_used,
                                "traceLen": attempt.trace.len(),
                            })),
                        },
                    );
                    return Ok(CycleProgress::NeedTeacher { cycle_id, request });
                }
                episode.evaluation = Some(Evaluation {
                    tier: VerifiabilityTier::Hard,
                    success: false,
                    details: error.to_string(),
                    surprise: None,
                });
                episode.cost.rung_reached = EscalationRung::Abstain;
                episode.reasoning_trace.steps.push(simple_step(
                    "abstain after procedure failure",
                    EscalationRung::Abstain,
                ));
                self.episodes.insert(&episode)?;
                Ok(CycleProgress::Completed(Box::new(CycleOutcome {
                    cycle_id,
                    disposition: CycleDisposition::Abstained,
                    answer: None,
                    episode,
                })))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn complete_simple(
        &self,
        cycle_id: CycleId,
        input: &CycleInput,
        disposition: CycleDisposition,
        answer: Option<Value>,
        action: &str,
        rung: EscalationRung,
        evaluation: Option<Evaluation>,
        teacher_interaction: Option<JsonValue>,
        interpretations: Vec<ResolvedInterpretation>,
    ) -> Result<CycleProgress, EngineError> {
        let mut episode = self.base_episode(input, &interpretations)?;
        let teacher_was_used = teacher_interaction.is_some();
        let (prior_reasoning, prior_trace, prior_steps_used, prior_trace_len) =
            prior_failure_material(teacher_interaction.as_ref());
        episode.execution_trace = prior_trace;
        episode.action = Some(action.into());
        episode.teacher_interaction = teacher_interaction;
        episode.evaluation = evaluation;
        if disposition == CycleDisposition::Provisional {
            episode.prediction = answer.clone();
        } else if disposition == CycleDisposition::Verified {
            episode.observed_result = answer.clone();
        }
        episode.reasoning_trace.steps = if prior_reasoning.steps.is_empty() {
            ladder_prefix(rung, teacher_was_used)
        } else {
            let mut steps = prior_reasoning.steps;
            if teacher_was_used {
                steps.push(simple_step("ask teacher", EscalationRung::Ask));
            }
            steps
        };
        if episode
            .reasoning_trace
            .steps
            .last()
            .is_none_or(|step| step.description != action)
        {
            episode
                .reasoning_trace
                .steps
                .push(simple_step(action, rung));
        }
        episode.cost = EpisodeCost {
            rung_reached: rung,
            steps_taken: prior_trace_len,
            budget_spent: if rung >= EscalationRung::Ask {
                f64::from(prior_steps_used) + 1.0
            } else {
                f64::from(prior_steps_used)
            },
        };
        self.episodes.insert(&episode)?;
        Ok(CycleProgress::Completed(Box::new(CycleOutcome {
            cycle_id,
            disposition,
            answer,
            episode,
        })))
    }

    fn base_episode(
        &self,
        input: &CycleInput,
        interpretations: &[ResolvedInterpretation],
    ) -> Result<Episode, EngineError> {
        let mut episode = Episode::new(&input.situation);
        if !interpretations.is_empty() {
            let chosen = chosen_concept(interpretations);
            let set = InterpretationSet::try_new(
                interpretations
                    .iter()
                    .map(|item| InterpretationCandidate {
                        meaning: item.concept.id,
                        weight: item.weight,
                    })
                    .collect(),
                chosen,
            )
            .map_err(|error| EngineError::InvalidInput(error.to_string()))?;
            episode.interpretations = set.to_episode_interpretations();
            let limits = ekg_reason::ContextLimits {
                max_entities: input.budget.max_context_items,
                max_relationships: input.budget.max_context_items,
                max_recent_episodes: input.budget.max_context_items,
                ..ekg_reason::ContextLimits::default()
            };
            let assembler = ContextAssembler::new(
                &self.graph,
                &self.episodes,
                ContextConfig {
                    limits,
                    ..ContextConfig::default()
                },
            )
            .map_err(|error| EngineError::InvalidInput(error.to_string()))?;
            let context = assembler
                .assemble(&ContextRequest {
                    goal: Some(input.situation.clone()),
                    goal_reason: Some("resolve the current task".into()),
                    interpretation: set,
                    entities: Vec::new(),
                    assumptions: input.assumptions.clone(),
                    environment: input.environment.clone(),
                    budget_remaining: RemainingBudget {
                        steps: input.budget.max_exec_steps,
                        teacher_calls: input.budget.max_teacher_turns,
                        cost: f64::from(input.budget.max_exec_steps),
                    },
                })
                .map_err(|error| EngineError::InvalidInput(error.to_string()))?;
            episode.context = context.to_episode_context();
            episode.knowledge_considered = context
                .interpretations
                .iter()
                .map(|candidate| KnowledgeCandidate {
                    concept: candidate.meaning,
                    relevance_score: candidate.weight,
                    was_used: chosen == Some(candidate.meaning),
                })
                .collect();
        } else {
            episode.context.goal = Some(input.situation.clone());
            episode.context.goal_reason = Some("resolve the current task".into());
            episode.context.assumptions = input.assumptions.clone();
            episode.context.environment = input.environment.clone();
            episode.context.budget_remaining = Some(ekg_core::ContextBudget {
                steps: input.budget.max_exec_steps,
                teacher_calls: input.budget.max_teacher_turns,
                cost: f64::from(input.budget.max_exec_steps),
            });
            let limits = ContextConfig::default().limits;
            episode.context.recent_episodes = self
                .episodes
                .list_recent(limits.max_recent_episodes as u32)?
                .into_iter()
                .map(|recent| ekg_core::ContextEpisode {
                    episode_id: recent.id,
                    situation: truncate_text(&recent.situation, limits.max_recent_text_chars),
                    action: recent
                        .action
                        .as_deref()
                        .map(|action| truncate_text(action, limits.max_recent_text_chars)),
                    observed_result: recent.observed_result.as_ref().map(|value| {
                        bound_core_value(
                            value,
                            limits.max_environment_value_chars,
                            limits.max_embedded_items,
                            limits.max_value_depth,
                        )
                    }),
                    succeeded: recent
                        .evaluation
                        .as_ref()
                        .map(|evaluation| evaluation.success),
                    created_at: recent.created_at,
                })
                .collect();
        }
        Ok(episode)
    }
}

fn validate_cycle_input(input: &CycleInput) -> Result<(), EngineError> {
    if input.situation.trim().is_empty() {
        return Err(EngineError::InvalidInput(
            "situation cannot be empty".into(),
        ));
    }
    if input.budget.max_context_items == 0 {
        return Err(EngineError::InvalidInput(
            "max_context_items must be positive".into(),
        ));
    }
    if input.budget.max_context_items > ekg_reason::MAX_CONTEXT_COLLECTION_ITEMS {
        return Err(EngineError::InvalidInput(format!(
            "max_context_items exceeds hard maximum {}",
            ekg_reason::MAX_CONTEXT_COLLECTION_ITEMS
        )));
    }
    if input.situation.chars().count() > ekg_reason::MAX_CONTEXT_TEXT_CHARS {
        return Err(EngineError::InvalidInput(
            "situation exceeds hard maximum".into(),
        ));
    }
    if input.assumptions.len() > ekg_reason::MAX_CONTEXT_COLLECTION_ITEMS
        || input.environment.len() > ekg_reason::MAX_CONTEXT_COLLECTION_ITEMS
    {
        return Err(EngineError::InvalidInput(
            "cycle context collection exceeds hard maximum".into(),
        ));
    }
    for assumption in &input.assumptions {
        if assumption.basis.trim().is_empty() {
            return Err(EngineError::InvalidInput(
                "assumptions must have a marked basis".into(),
            ));
        }
        if assumption.description.chars().count() > ekg_reason::MAX_CONTEXT_TEXT_CHARS
            || assumption.basis.chars().count() > ekg_reason::MAX_CONTEXT_TEXT_CHARS
        {
            return Err(EngineError::InvalidInput(
                "assumption text exceeds hard maximum".into(),
            ));
        }
    }
    for (key, value) in &input.environment {
        if key.chars().count() > ekg_reason::MAX_CONTEXT_TEXT_CHARS {
            return Err(EngineError::InvalidInput(
                "environment key exceeds hard maximum".into(),
            ));
        }
        validate_core_value(value, 0)?;
    }
    Ok(())
}

fn chosen_concept(items: &[ResolvedInterpretation]) -> Option<ConceptId> {
    let mut ranked = items.iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .weight
            .partial_cmp(&left.weight)
            .unwrap_or(Ordering::Equal)
    });
    match ranked.as_slice() {
        [] => None,
        [only] => Some(only.concept.id),
        [first, second, ..] if first.weight > second.weight => Some(first.concept.id),
        _ => None,
    }
}

fn uniquely_resolved(items: &[ResolvedInterpretation]) -> Option<&ResolvedInterpretation> {
    let chosen = chosen_concept(items)?;
    items.iter().find(|item| item.concept.id == chosen)
}

fn contains_term(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack.match_indices(needle).any(|(index, _)| {
        let before = haystack[..index].chars().next_back();
        let after = haystack[index + needle.len()..].chars().next();
        before.is_none_or(|character| !character.is_alphanumeric())
            && after.is_none_or(|character| !character.is_alphanumeric())
    })
}

fn extract_literals(situation: &str) -> Vec<Value> {
    situation
        .split(|character: char| {
            character.is_whitespace() || matches!(character, ',' | '?' | '!' | '(' | ')' | '=')
        })
        .filter_map(|token| {
            let clean = token.trim_matches(|character: char| matches!(character, '.' | ':' | ';'));
            if clean.eq_ignore_ascii_case("true") {
                Some(Value::Bool(true))
            } else if clean.eq_ignore_ascii_case("false") {
                Some(Value::Bool(false))
            } else if let Ok(value) = clean.parse::<i64>() {
                Some(Value::Int(value))
            } else {
                clean.parse::<f64>().ok().map(Value::Float)
            }
        })
        .collect()
}

fn complete_inputs(
    procedure: &Procedure,
    environment: &BTreeMap<String, Value>,
    explicit: &BTreeMap<String, Value>,
    literals: &[Value],
) -> Option<BTreeMap<String, Value>> {
    let missing = procedure
        .params
        .iter()
        .filter(|parameter| {
            !explicit.contains_key(&parameter.name) && !environment.contains_key(&parameter.name)
        })
        .count();
    if (explicit.is_empty() || missing > 0) && literals.len() != missing {
        return None;
    }
    let mut inputs = BTreeMap::new();
    let mut literal_index = 0;
    for parameter in &procedure.params {
        if let Some(value) = explicit
            .get(&parameter.name)
            .or_else(|| environment.get(&parameter.name))
        {
            inputs.insert(parameter.name.clone(), value.clone());
        } else if let Some(value) = literals.get(literal_index) {
            inputs.insert(parameter.name.clone(), value.clone());
            literal_index += 1;
        } else {
            return None;
        }
    }
    if explicit.keys().any(|name| {
        !procedure
            .params
            .iter()
            .any(|parameter| &parameter.name == name)
    }) {
        return None;
    }
    Some(inputs)
}

fn simple_step(description: &str, rung: EscalationRung) -> TraceStep {
    TraceStep {
        description: description.into(),
        procedure_used: None,
        contract_check: None,
        input: None,
        output: None,
        rung,
        status: TraceStepStatus::Succeeded,
    }
}

fn ladder_prefix(terminal_rung: EscalationRung, teacher_was_used: bool) -> Vec<TraceStep> {
    let mut steps = vec![simple_step(
        if terminal_rung == EscalationRung::Recall {
            "recall verified result"
        } else {
            "recall found no verified result"
        },
        EscalationRung::Recall,
    )];
    if terminal_rung >= EscalationRung::Run {
        steps.push(simple_step(
            if terminal_rung == EscalationRung::Run {
                "run matched procedure"
            } else {
                "run could not resolve locally"
            },
            EscalationRung::Run,
        ));
    }
    if teacher_was_used {
        steps.push(simple_step("ask teacher", EscalationRung::Ask));
    }
    steps
}

fn prior_failure_material(
    teacher_interaction: Option<&JsonValue>,
) -> (ReasoningTrace, Option<JsonValue>, u32, u32) {
    let Some(failure) = teacher_interaction
        .and_then(|interaction| interaction.get("priorFailure"))
        .filter(|failure| !failure.is_null())
    else {
        return (ReasoningTrace::default(), None, 0, 0);
    };
    let reasoning = failure
        .get("reasoningTrace")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let trace = failure.get("executionTrace").cloned();
    let steps_used = failure
        .get("stepsUsed")
        .and_then(JsonValue::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);
    let trace_len = failure
        .get("traceLen")
        .and_then(JsonValue::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);
    (reasoning, trace, steps_used, trace_len)
}

fn proposal_schema() -> JsonValue {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "interpretations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["concept", "weight", "inputs"],
                    "properties": {
                        "concept": {
                            "anyOf": [
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "properties": { "id": { "type": "string" } },
                                    "required": ["id"]
                                },
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "properties": { "name": { "type": "string" } },
                                    "required": ["name"]
                                },
                                { "type": "string" }
                            ]
                        },
                        "weight": { "type": "number", "minimum": 0, "maximum": 1 },
                        "inputs": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "name": { "type": "string" },
                                    "value": { "type": ["null", "boolean", "number", "string"] }
                                },
                                "required": ["name", "value"]
                            }
                        }
                    }
                }
            },
            "procedure": {
                "type": ["string", "null"],
                "description": "A JSON-serialized EKG Procedure proposal, or null"
            },
            "answer": { "type": ["null", "boolean", "number", "string"] },
            "abstainReason": { "type": ["string", "null"] }
        },
        "required": ["interpretations", "procedure", "answer", "abstainReason"]
    })
}

fn deserialize_optional_procedure<'de, D>(deserializer: D) -> Result<Option<Procedure>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<JsonValue>::deserialize(deserializer)?;
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(json)) => serde_json::from_str(&json)
            .map(Some)
            .map_err(serde::de::Error::custom),
        Some(value) => serde_json::from_value(value)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

fn deserialize_inputs<'de, D>(deserializer: D) -> Result<BTreeMap<String, Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    match value {
        JsonValue::Object(_) => serde_json::from_value(value).map_err(serde::de::Error::custom),
        JsonValue::Array(_) => {
            let inputs: Vec<NamedInput> =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            let mut result = BTreeMap::new();
            for input in inputs {
                if result.insert(input.name.clone(), input.value).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate input '{}'",
                        input.name
                    )));
                }
            }
            Ok(result)
        }
        _ => Err(serde::de::Error::custom(
            "inputs must be an object or named-input array",
        )),
    }
}

fn usable_lifecycle(lifecycle: ekg_core::Lifecycle) -> bool {
    matches!(
        lifecycle,
        ekg_core::Lifecycle::Active
            | ekg_core::Lifecycle::Validated
            | ekg_core::Lifecycle::Provisional
            | ekg_core::Lifecycle::UnderReview
    )
}

fn truncate_text(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

fn bound_json(
    value: JsonValue,
    depth: usize,
    collection_limit: usize,
    remaining_nodes: &mut usize,
    remaining_chars: &mut usize,
) -> JsonValue {
    if depth >= MAX_TEACHER_VALUE_DEPTH || *remaining_nodes == 0 {
        return JsonValue::Null;
    }
    *remaining_nodes -= 1;
    match value {
        JsonValue::String(text) => {
            let maximum = MAX_TEACHER_TEXT_CHARS.min(*remaining_chars);
            let bounded = truncate_text(&text, maximum);
            *remaining_chars = remaining_chars.saturating_sub(bounded.chars().count());
            JsonValue::String(bounded)
        }
        JsonValue::Array(items) => JsonValue::Array(
            items
                .into_iter()
                .take(collection_limit)
                .map_while(|item| {
                    (*remaining_nodes > 0).then(|| {
                        bound_json(
                            item,
                            depth + 1,
                            collection_limit,
                            remaining_nodes,
                            remaining_chars,
                        )
                    })
                })
                .collect(),
        ),
        JsonValue::Object(items) => JsonValue::Object(
            items
                .into_iter()
                .take(collection_limit)
                .map_while(|(key, value)| {
                    if *remaining_nodes == 0 {
                        return None;
                    }
                    let maximum = MAX_TEACHER_TEXT_CHARS.min(*remaining_chars);
                    let key = truncate_text(&key, maximum);
                    *remaining_chars = remaining_chars.saturating_sub(key.chars().count());
                    Some((
                        key,
                        bound_json(
                            value,
                            depth + 1,
                            collection_limit,
                            remaining_nodes,
                            remaining_chars,
                        ),
                    ))
                })
                .collect(),
        ),
        scalar => scalar,
    }
}

fn validate_core_value(value: &Value, depth: usize) -> Result<(), EngineError> {
    if depth > ekg_reason::MAX_CONTEXT_VALUE_DEPTH {
        return Err(EngineError::InvalidInput(
            "environment value exceeds hard depth maximum".into(),
        ));
    }
    match value {
        Value::Text(text) if text.chars().count() > ekg_reason::MAX_CONTEXT_TEXT_CHARS => Err(
            EngineError::InvalidInput("environment text exceeds hard maximum".into()),
        ),
        Value::List(items) => {
            if items.len() > ekg_reason::MAX_CONTEXT_COLLECTION_ITEMS {
                return Err(EngineError::InvalidInput(
                    "environment list exceeds hard maximum".into(),
                ));
            }
            for item in items {
                validate_core_value(item, depth + 1)?;
            }
            Ok(())
        }
        Value::Map(items) => {
            if items.len() > ekg_reason::MAX_CONTEXT_COLLECTION_ITEMS {
                return Err(EngineError::InvalidInput(
                    "environment map exceeds hard maximum".into(),
                ));
            }
            for (key, item) in items {
                if key.chars().count() > ekg_reason::MAX_CONTEXT_TEXT_CHARS {
                    return Err(EngineError::InvalidInput(
                        "environment map key exceeds hard maximum".into(),
                    ));
                }
                validate_core_value(item, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn bound_core_value(value: &Value, max_chars: usize, max_items: usize, depth: usize) -> Value {
    match value {
        Value::Text(text) => Value::Text(truncate_text(text, max_chars)),
        Value::List(_) if depth == 0 => Value::List(Vec::new()),
        Value::Map(_) if depth == 0 => Value::Map(BTreeMap::new()),
        Value::List(items) => Value::List(
            items
                .iter()
                .take(max_items)
                .map(|item| bound_core_value(item, max_chars, max_items, depth - 1))
                .collect(),
        ),
        Value::Map(items) => Value::Map(
            items
                .iter()
                .take(max_items)
                .map(|(key, item)| {
                    (
                        truncate_text(key, max_chars),
                        bound_core_value(item, max_chars, max_items, depth - 1),
                    )
                })
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}

fn valid_teacher_provenance(proposal: &TeacherProposalWire, situation: &str) -> bool {
    let Some(provenance) = proposal.provenance.as_object() else {
        return false;
    };
    let provider = provenance
        .get("provider")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let source_matches_provider = matches!(provider, "claude" | "openai" | "ollama" | "human")
        && proposal
            .source
            .strip_prefix(&format!("{provider}:"))
            .is_some_and(|suffix| !suffix.trim().is_empty());
    provenance.get("situation").and_then(JsonValue::as_str) == Some(situation)
        && provenance.get("teacher").and_then(JsonValue::as_str) == Some(proposal.source.as_str())
        && source_matches_provider
        && provenance
            .get("requestId")
            .and_then(JsonValue::as_str)
            .is_some_and(|request_id| !request_id.trim().is_empty())
        && provenance
            .get("generatedAt")
            .and_then(JsonValue::as_str)
            .is_some_and(|generated_at| !generated_at.trim().is_empty())
}
