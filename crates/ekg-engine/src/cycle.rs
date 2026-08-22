use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

use ekg_core::{
    Assumption, BinOp, Concept, ConceptId, Condition, Contract, EkgError, Episode, EpisodeCost,
    EpisodeId, EscalationRung, Evaluation, Expr, KnowledgeCandidate, Lifecycle, MutabilityClass,
    Param, Procedure, ReasoningTrace, Relationship, TraceStep, TraceStepStatus, UnOp, Value,
    VerifiabilityTier,
};
use ekg_exec::ExecTrace;
use ekg_reason::{
    ContextAssembler, ContextConfig, ContextRequest, InterpretationCandidate, InterpretationSet,
    RemainingBudget,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::engine::{Engine, EngineError, bind_inputs, is_current_executable, reasoning_trace};
use crate::lesson::DurableLessonStage;
use ekg_intuition::RecallKind;

const MAX_TEACHER_CONTEXT_ITEMS: usize = 64;
const MAX_TEACHER_TEXT_CHARS: usize = 2_048;
const MAX_TEACHER_VALUE_DEPTH: usize = 8;
const MAX_TEACHER_CONTEXT_NODES: usize = 8_192;
const MAX_TEACHER_CONTEXT_CHARS: usize = 262_144;
const MAX_LESSON_CONCEPTS: usize = 8;
const MAX_LESSON_RELATIONSHIPS: usize = 16;
const MAX_LESSON_PARAMETERS: usize = 16;
const MAX_LESSON_CONDITIONS: usize = 16;
const MAX_LESSON_INSTRUCTIONS: usize = 64;
const MAX_LESSON_KEY_CHARS: usize = 128;
const MAX_LESSON_NAME_CHARS: usize = 256;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingCycle {
    input: CycleInput,
    request: TeacherRequestWire,
    initial_interpretations: Vec<ResolvedInterpretation>,
    prior_failure: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolvedInterpretation {
    concept: Concept,
    weight: f64,
    inputs: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProposalContent {
    #[serde(default)]
    proposal_kind: Option<ProposalKind>,
    #[serde(default)]
    interpretations: Vec<ProposalInterpretation>,
    #[serde(default)]
    lesson: Option<JsonValue>,
    #[serde(default, deserialize_with = "deserialize_optional_procedure")]
    procedure: Option<Procedure>,
    #[serde(default)]
    answer: Option<Value>,
    #[serde(default, rename = "abstainReason")]
    abstain_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProposalKind {
    ReusableLesson,
    ExternalObservation,
    AnswerOnly,
    Abstain,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalInterpretation {
    concept: ProposalConcept,
    weight: f64,
    #[serde(default, deserialize_with = "deserialize_inputs")]
    inputs: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LessonDraft {
    primitive_set: String,
    concepts: Vec<ConceptDraft>,
    relationships: Vec<RelationshipDraft>,
    procedures: Vec<ProcedureDraft>,
    invocation: InvocationDraft,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConceptDraft {
    key: String,
    name: String,
    description: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RelationshipDraft {
    source: ConceptReferenceDraft,
    target: ConceptReferenceDraft,
    kind: String,
    strength: f64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProcedureDraft {
    key: String,
    name: String,
    concept: ConceptReferenceDraft,
    parameters: Vec<ParameterDraft>,
    body: ProgramDraft,
    contract: ContractDraft,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ParameterDraft {
    name: String,
    description: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InvocationDraft {
    procedure_key: String,
    inputs: Vec<NamedValueDraft>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NamedValueDraft {
    name: String,
    value: Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
enum ConceptReferenceDraft {
    NewConcept { key: String },
    ExistingConcept { id: String },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContractDraft {
    requires: Vec<ConditionDraft>,
    promises: Vec<ConditionDraft>,
    fails_when: Vec<ConditionDraft>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConditionDraft {
    description: String,
    check: ProgramDraft,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProgramDraft {
    instructions: Vec<InstructionDraft>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "op", deny_unknown_fields)]
enum InstructionDraft {
    LoadParameter { name: String },
    LoadResult,
    PushLiteral { value: Value },
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Equal,
    NotEqual,
    LessThan,
    LessOrEqual,
    GreaterThan,
    GreaterOrEqual,
    And,
    Or,
    Negate,
    Not,
}

#[derive(Debug, Clone)]
struct CompiledLesson {
    idempotency_key: String,
    concepts: Vec<Concept>,
    relationships: Vec<Relationship>,
    procedures: Vec<Procedure>,
    interpretation: ResolvedInterpretation,
    invocation_inputs: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
enum KnowledgeToLearn {
    LegacyProcedure,
    Lesson(Box<CompiledLesson>),
}

impl Engine {
    pub(crate) fn recover_pending_cycles(&mut self) -> Result<(), EngineError> {
        for (cycle_id, pending_json) in self.runtime.pending_cycles()? {
            let pending: PendingCycle = serde_json::from_str(&pending_json)?;
            self.runtime
                .claim_pending_cycle(cycle_id, self.instance_id)?;
            self.pending_cycles.insert(cycle_id, pending);
        }
        Ok(())
    }

    pub(crate) fn recover_pending_lessons(&mut self) -> Result<(), EngineError> {
        for stage in self.lesson_stages.pending()? {
            self.graph.insert_knowledge_bundle(
                &stage.bundle_key,
                &stage.concepts,
                &stage.relationships,
                &stage.procedures,
            )?;
            for concept in &stage.concepts {
                self.index_concept(concept)?;
            }
            for procedure in &stage.procedures {
                self.index_procedure(procedure)?;
            }
            let episode_exists = self
                .episodes
                .list_recent(u32::MAX)?
                .iter()
                .any(|episode| episode.id == stage.episode.id);
            if !episode_exists {
                self.persist_engine_episode(&stage.episode)?;
            }
            self.lesson_stages.complete(&stage)?;
        }
        Ok(())
    }

    pub fn begin_cycle(&mut self, input: CycleInput) -> Result<CycleProgress, EngineError> {
        let input = self.apply_durable_assumption_overrides(input)?;
        validate_cycle_input(&input)?;
        let cycle_id = CycleId::new();
        self.runtime.begin_cycle(cycle_id, self.instance_id)?;
        let result = self.begin_cycle_inner(cycle_id, input);
        self.persist_cycle_result(cycle_id, &result)?;
        result
    }

    fn begin_cycle_inner(
        &mut self,
        cycle_id: CycleId,
        input: CycleInput,
    ) -> Result<CycleProgress, EngineError> {
        if let Some(answer) = self.recall(&input)? {
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
            && let Some(procedure) = self.procedure_for(resolved.concept.id, &input.environment)?
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
                None,
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

    fn apply_durable_assumption_overrides(
        &self,
        mut input: CycleInput,
    ) -> Result<CycleInput, EngineError> {
        let mut overrides = self.assumption_overrides()?.into_iter().collect::<Vec<_>>();
        overrides.sort_by(|left, right| left.0.cmp(&right.0));
        for (key, replacement) in overrides {
            let matching = input
                .assumptions
                .iter()
                .filter(|assumption| assumption.description == key)
                .collect::<Vec<_>>();
            // Fresh observed reality outranks a historical correction. The
            // durable correction otherwise replaces inferred, assumed, or
            // teacher-provided context with one marked corrected assumption.
            if matching
                .iter()
                .any(|assumption| assumption.basis.eq_ignore_ascii_case("observed"))
            {
                continue;
            }
            let concept = matching.iter().find_map(|assumption| assumption.concept);
            input
                .assumptions
                .retain(|assumption| assumption.description != key);
            input.assumptions.push(Assumption {
                description: format!("{key} = {replacement}"),
                basis: "corrected".into(),
                concept,
            });
        }
        Ok(input)
    }

    pub fn resume_cycle(
        &mut self,
        cycle_id: CycleId,
        proposal: TeacherProposalWire,
    ) -> Result<CycleProgress, EngineError> {
        self.runtime
            .assert_cycle_owner(cycle_id, self.instance_id)?;
        let result = self.resume_cycle_inner(cycle_id, proposal);
        self.persist_cycle_result(cycle_id, &result)?;
        result
    }

    fn resume_cycle_inner(
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

        if content.proposal_kind == Some(ProposalKind::ExternalObservation)
            && let Some(answer) = content.answer.clone()
        {
            return self.complete_simple(
                cycle_id,
                &pending.input,
                CycleDisposition::Provisional,
                Some(answer),
                "teacher-observation:provisional",
                EscalationRung::Ask,
                None,
                Some(teacher_json),
                Vec::new(),
            );
        }

        if content.proposal_kind == Some(ProposalKind::ReusableLesson) {
            let compiled = content
                .lesson
                .as_ref()
                .ok_or_else(|| {
                    EngineError::InvalidInput("reusable lesson is missing lesson content".into())
                })
                .and_then(|value| {
                    serde_json::from_value::<LessonDraft>(value.clone()).map_err(|error| {
                        EngineError::InvalidInput(format!("invalid reusable lesson: {error}"))
                    })
                })
                .and_then(|draft| self.compile_lesson(&draft));
            match compiled {
                Ok(lesson) => {
                    let procedure = lesson.procedures[0].clone();
                    let inputs = lesson.invocation_inputs.clone();
                    return self.execute_cycle_procedure(
                        cycle_id,
                        &pending.input,
                        &[],
                        &procedure,
                        inputs,
                        EscalationRung::Ask,
                        Some(teacher_json),
                        Some(KnowledgeToLearn::Lesson(Box::new(lesson))),
                        content.answer,
                    );
                }
                Err(error) if pending.input.budget.max_teacher_turns > 0 => {
                    return self.retry_reusable_lesson(
                        cycle_id,
                        pending,
                        teacher_json,
                        &error.to_string(),
                    );
                }
                Err(_) => {
                    // The teacher budget is exhausted. A separately useful
                    // answer remains provisional, but unsafe knowledge is not
                    // inserted.
                    if let Some(answer) = content.answer {
                        return self.complete_simple(
                            cycle_id,
                            &pending.input,
                            CycleDisposition::Provisional,
                            Some(answer),
                            "teacher-answer:provisional-lesson-rejected",
                            EscalationRung::Ask,
                            None,
                            Some(teacher_json),
                            Vec::new(),
                        );
                    }
                }
            }
        }
        let interpretations = match self.resolve_teacher_interpretations(&content.interpretations) {
            Ok(values) => values,
            Err(_) if content.answer.is_some() && content.procedure.is_none() => Vec::new(),
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
                    Some(KnowledgeToLearn::LegacyProcedure),
                    expected_answer.clone(),
                );
            }
        }

        if let Some(resolved) = uniquely_resolved(&interpretations)
            && let Some(procedure) =
                self.procedure_for(resolved.concept.id, &pending.input.environment)?
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
                None,
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
        self.runtime
            .assert_cycle_owner(cycle_id, self.instance_id)?;
        let result = self.abort_cycle_inner(cycle_id, reason.into());
        self.persist_cycle_result(cycle_id, &result)?;
        result
    }

    fn abort_cycle_inner(
        &mut self,
        cycle_id: CycleId,
        reason: String,
    ) -> Result<CycleProgress, EngineError> {
        let pending = self.pending_cycles.remove(&cycle_id).ok_or_else(|| {
            EngineError::InvalidInput(format!("cycle {cycle_id} is unknown or already consumed"))
        })?;
        let reason = truncate_text(&reason, MAX_TEACHER_TEXT_CHARS);
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

    fn persist_cycle_result(
        &mut self,
        cycle_id: CycleId,
        result: &Result<CycleProgress, EngineError>,
    ) -> Result<(), EngineError> {
        match result {
            Ok(CycleProgress::NeedTeacher { .. }) => {
                let pending = self.pending_cycles.get(&cycle_id).ok_or_else(|| {
                    EngineError::InvalidInput(format!(
                        "cycle {cycle_id} requested a teacher without durable continuation state"
                    ))
                })?;
                self.runtime.save_pending_cycle(
                    cycle_id,
                    self.instance_id,
                    &serde_json::to_string(pending)?,
                )?;
            }
            Ok(CycleProgress::Completed(_)) => {
                self.runtime.complete_cycle(cycle_id)?;
            }
            Err(_) => {
                // Preserve an existing pending continuation after a transient
                // resume failure. A failed initial running cycle is safe to
                // release because no continuation was exposed to the caller.
                if self
                    .runtime
                    .pending_cycles()?
                    .iter()
                    .any(|(id, _)| *id == cycle_id)
                {
                    self.recover_pending_cycles()?;
                } else {
                    self.runtime.complete_cycle(cycle_id)?;
                }
            }
        }
        Ok(())
    }

    fn recall(&self, input: &CycleInput) -> Result<Option<Value>, EngineError> {
        let candidates = self.recall_candidates(&input.situation, 64)?;
        for candidate in candidates {
            let Some(id) = candidate.id.strip_prefix("episode:") else {
                continue;
            };
            let Ok(uuid) = Uuid::parse_str(id) else {
                continue;
            };
            let episode = match self.episodes.get(EpisodeId(uuid)) {
                Ok(episode) => episode,
                Err(EkgError::NotFound(_)) => continue,
                Err(error) => return Err(error.into()),
            };
            if episode.situation != input.situation
                || episode.context.environment != input.environment
                || self.trust_receipt_for_episode(&episode)?.is_none()
                || !episode.evaluation.as_ref().is_some_and(|evaluation| {
                    evaluation.success
                        && matches!(
                            evaluation.tier,
                            VerifiabilityTier::Hard | VerifiabilityTier::Consensus
                        )
                })
                || episode.observed_result.is_none()
            {
                continue;
            }
            let mut held = false;
            for fact in &episode.observed_facts {
                if !self
                    .held_contradictions_for_predicate(&fact.predicate)?
                    .is_empty()
                {
                    held = true;
                    break;
                }
            }
            if !held {
                return Ok(episode.observed_result);
            }
        }
        Ok(None)
    }

    fn local_interpretations(
        &self,
        situation: &str,
    ) -> Result<Vec<ResolvedInterpretation>, EngineError> {
        let mut matches = Vec::new();
        for candidate in self.rank_recall_candidates(situation, 64)? {
            if candidate.kind != RecallKind::Concept {
                continue;
            }
            let Some(id) = candidate.id.strip_prefix("concept:") else {
                continue;
            };
            let Ok(uuid) = Uuid::parse_str(id) else {
                continue;
            };
            let Some(concept) = self.graph.get_concept(ConceptId(uuid))? else {
                continue;
            };
            if usable_lifecycle(concept.lifecycle) {
                matches.push((concept, candidate.learned_score));
            }
        }
        matches.sort_by(|(left_concept, left_score), (right_concept, right_score)| {
            right_score
                .partial_cmp(left_score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left_concept.name.cmp(&right_concept.name))
        });
        let best = matches.first().map(|(_, score)| *score);
        let matches = matches
            .into_iter()
            .filter(|(_, score)| Some(*score) == best)
            .map(|(concept, _)| concept)
            .collect::<Vec<_>>();
        let weight = 1.0 / matches.len().max(1) as f64;
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

    fn procedure_for(
        &self,
        concept: ConceptId,
        environment: &BTreeMap<String, Value>,
    ) -> Result<Option<Procedure>, EngineError> {
        let predicate = format!("concept:{concept}");
        let refinement = self.refinement_context_for_predicate(&predicate, environment)?;
        if !refinement.unresolved.is_empty() {
            return Ok(None);
        }
        let mut refined = Vec::new();
        for applied in refinement.applied {
            for episode_id in applied.claim.supporting_episodes {
                let episode = self.episodes.get(episode_id)?;
                let Some(procedure_id) = episode.action.as_deref().and_then(action_procedure_id)
                else {
                    continue;
                };
                let Some(procedure) = self.graph.get_procedure(procedure_id)? else {
                    continue;
                };
                if procedure.concept == Some(concept)
                    && is_current_executable(procedure.lifecycle)
                    && !refined
                        .iter()
                        .any(|candidate: &Procedure| candidate.id == procedure.id)
                {
                    refined.push(procedure);
                }
            }
        }
        if refined.len() == 1 {
            return Ok(refined.pop());
        }
        if refined.len() > 1 {
            return Ok(None);
        }
        Ok(self.graph.list_procedures()?.into_iter().find(|procedure| {
            procedure.concept == Some(concept) && is_current_executable(procedure.lifecycle)
        }))
    }

    fn retry_reusable_lesson(
        &mut self,
        cycle_id: CycleId,
        mut pending: PendingCycle,
        rejected_interaction: JsonValue,
        reason: &str,
    ) -> Result<CycleProgress, EngineError> {
        let mut request = self.teacher_request(&pending.input, &pending.initial_interpretations)?;
        request.specific_question = Some(format!(
            "The reusable lesson could not be safely compiled: {}. Return one corrected pure_rpn_v1 reusable_lesson using only the advertised grammar, or answer/abstain without a lesson.",
            truncate_text(reason, MAX_TEACHER_TEXT_CHARS)
        ));
        if let Some(context) = request.context.as_object_mut() {
            context.insert(
                "lessonRetry".into(),
                json!({
                    "reason": truncate_text(reason, MAX_TEACHER_TEXT_CHARS),
                    "remainingAttempts": pending.input.budget.max_teacher_turns,
                }),
            );
        }
        pending.input.budget.max_teacher_turns =
            pending.input.budget.max_teacher_turns.saturating_sub(1);
        pending.request = request.clone();
        pending.prior_failure = Some(json!({
            "rejectedTeacherInteraction": rejected_interaction,
            "lessonRejection": truncate_text(reason, MAX_TEACHER_TEXT_CHARS),
        }));
        self.pending_cycles.insert(cycle_id, pending);
        Ok(CycleProgress::NeedTeacher { cycle_id, request })
    }

    fn compile_lesson(&self, draft: &LessonDraft) -> Result<CompiledLesson, EngineError> {
        if draft.primitive_set != "pure_rpn_v1" {
            return Err(lesson_error("unsupported primitive set"));
        }
        if draft.concepts.is_empty() || draft.concepts.len() > MAX_LESSON_CONCEPTS {
            return Err(lesson_error("lesson must introduce 1..=8 concepts"));
        }
        if draft.relationships.len() > MAX_LESSON_RELATIONSHIPS || draft.procedures.len() != 1 {
            return Err(lesson_error(
                "lesson must contain 0..=16 relationships and exactly one procedure",
            ));
        }

        // The knowledge identity excludes the one-off invocation. Replaying
        // the same reusable teaching payload after a crash regenerates the
        // exact IDs even if the example input differs.
        let canonical = serde_json::to_vec(&(
            &draft.primitive_set,
            &draft.concepts,
            &draft.relationships,
            &draft.procedures,
        ))?;
        let digest = stable_lesson_digest(&canonical);
        let idempotency_key = format!("teacher-lesson:{digest}");

        let mut concept_keys = HashMap::new();
        let mut concept_names = HashSet::new();
        let mut concepts = Vec::with_capacity(draft.concepts.len());
        for concept_draft in &draft.concepts {
            validate_lesson_token(&concept_draft.key, "concept key", MAX_LESSON_KEY_CHARS)?;
            validate_lesson_token(&concept_draft.name, "concept name", MAX_LESSON_NAME_CHARS)?;
            if concept_draft.description.trim().is_empty()
                || concept_draft.description.chars().count() > MAX_TEACHER_TEXT_CHARS
            {
                return Err(lesson_error(
                    "concept description must be nonempty and bounded",
                ));
            }
            if concept_keys.contains_key(&concept_draft.key)
                || !concept_names.insert(concept_draft.name.to_lowercase())
            {
                return Err(lesson_error("concept keys and names must be unique"));
            }
            if self
                .graph
                .get_concept_by_name(&concept_draft.name)?
                .is_some()
            {
                return Err(lesson_error(
                    "a new concept may not overwrite or reactivate an existing concept",
                ));
            }
            let mut concept = Concept::new(&concept_draft.name, MutabilityClass::Procedural)
                .with_description(&concept_draft.description);
            concept.id = ConceptId(deterministic_lesson_uuid(
                &canonical,
                "concept",
                &concept_draft.key,
            ));
            concept.lifecycle = Lifecycle::Provisional;
            concept_keys.insert(concept_draft.key.clone(), concept.clone());
            concepts.push(concept);
        }

        let mut referenced_new_concepts = HashSet::new();
        let mut relationships = Vec::with_capacity(draft.relationships.len());
        for (index, relationship_draft) in draft.relationships.iter().enumerate() {
            validate_lesson_token(
                &relationship_draft.kind,
                "relationship kind",
                MAX_LESSON_NAME_CHARS,
            )?;
            if !relationship_draft.strength.is_finite()
                || !(0.0..=1.0).contains(&relationship_draft.strength)
            {
                return Err(lesson_error(
                    "relationship strength must be between zero and one",
                ));
            }
            let source = self.resolve_lesson_concept(
                &relationship_draft.source,
                &concept_keys,
                &mut referenced_new_concepts,
            )?;
            let target = self.resolve_lesson_concept(
                &relationship_draft.target,
                &concept_keys,
                &mut referenced_new_concepts,
            )?;
            let mut relationship =
                Relationship::new(source.id, target.id, &relationship_draft.kind);
            relationship.id = ekg_core::RelationshipId(deterministic_lesson_uuid(
                &canonical,
                "relationship",
                &index.to_string(),
            ));
            relationship.strength = relationship_draft.strength;
            relationship.lifecycle = Lifecycle::Provisional;
            relationships.push(relationship);
        }

        let procedure_draft = &draft.procedures[0];
        validate_lesson_token(&procedure_draft.key, "procedure key", MAX_LESSON_KEY_CHARS)?;
        validate_lesson_token(
            &procedure_draft.name,
            "procedure name",
            MAX_LESSON_NAME_CHARS,
        )?;
        if self
            .graph
            .list_procedures()?
            .iter()
            .any(|procedure| procedure.name.eq_ignore_ascii_case(&procedure_draft.name))
        {
            return Err(lesson_error(
                "a learned procedure may not overwrite or reactivate existing knowledge",
            ));
        }
        if procedure_draft.parameters.is_empty()
            || procedure_draft.parameters.len() > MAX_LESSON_PARAMETERS
        {
            return Err(lesson_error(
                "a reusable procedure must declare 1..=16 parameters",
            ));
        }
        let mut parameter_names = HashSet::new();
        let mut parameters = Vec::with_capacity(procedure_draft.parameters.len());
        for parameter_draft in &procedure_draft.parameters {
            validate_lesson_token(
                &parameter_draft.name,
                "parameter name",
                MAX_LESSON_KEY_CHARS,
            )?;
            if parameter_draft.description.chars().count() > MAX_TEACHER_TEXT_CHARS
                || !parameter_names.insert(parameter_draft.name.clone())
            {
                return Err(lesson_error("parameter names must be unique and bounded"));
            }
            parameters.push(Param {
                name: parameter_draft.name.clone(),
                description: Some(parameter_draft.description.clone()),
            });
        }
        let (body, used_parameters) =
            compile_lesson_program(&procedure_draft.body, &parameter_names, false)?;
        if parameter_names != used_parameters {
            return Err(lesson_error(
                "a reusable procedure body must use every declared parameter",
            ));
        }
        let contract = compile_lesson_contract(&procedure_draft.contract, &parameter_names)?;
        let attached = self.resolve_lesson_concept(
            &procedure_draft.concept,
            &concept_keys,
            &mut referenced_new_concepts,
        )?;
        if !matches!(
            procedure_draft.concept,
            ConceptReferenceDraft::NewConcept { .. }
        ) {
            return Err(lesson_error(
                "a bootstrap lesson procedure must bind its newly introduced concept",
            ));
        }
        let mut procedure = Procedure::new(&procedure_draft.name, parameters, body)
            .with_contract(contract)
            .with_concept(attached.id);
        procedure.id = ekg_core::ProcedureId(deterministic_lesson_uuid(
            &canonical,
            "procedure",
            &procedure_draft.key,
        ));
        procedure.lifecycle = Lifecycle::Provisional;

        if referenced_new_concepts.len() != concepts.len() {
            return Err(lesson_error(
                "every introduced concept must be used by the lesson",
            ));
        }
        if draft.invocation.procedure_key != procedure_draft.key {
            return Err(lesson_error(
                "invocation must target the proposed procedure",
            ));
        }
        let mut invocation_inputs = BTreeMap::new();
        for input in &draft.invocation.inputs {
            if !lesson_scalar(&input.value) {
                return Err(lesson_error("lesson inputs must be scalar values"));
            }
            if invocation_inputs
                .insert(input.name.clone(), input.value.clone())
                .is_some()
            {
                return Err(lesson_error("invocation input names must be unique"));
            }
        }
        let invocation_inputs =
            complete_inputs(&procedure, &BTreeMap::new(), &invocation_inputs, &[]).ok_or_else(
                || lesson_error("invocation must supply every declared parameter exactly once"),
            )?;
        let interpretation = ResolvedInterpretation {
            concept: attached,
            weight: 1.0,
            inputs: invocation_inputs.clone(),
        };
        Ok(CompiledLesson {
            idempotency_key,
            concepts,
            relationships,
            procedures: vec![procedure],
            interpretation,
            invocation_inputs,
        })
    }

    fn resolve_lesson_concept(
        &self,
        reference: &ConceptReferenceDraft,
        new_concepts: &HashMap<String, Concept>,
        referenced_new: &mut HashSet<String>,
    ) -> Result<Concept, EngineError> {
        match reference {
            ConceptReferenceDraft::NewConcept { key } => {
                let concept = new_concepts
                    .get(key)
                    .cloned()
                    .ok_or_else(|| lesson_error("lesson referenced an unknown concept key"))?;
                referenced_new.insert(key.clone());
                Ok(concept)
            }
            ConceptReferenceDraft::ExistingConcept { id } => {
                let uuid = Uuid::parse_str(id)
                    .map_err(|_| lesson_error("lesson contains an invalid existing concept id"))?;
                let concept = self
                    .graph
                    .get_concept(ConceptId(uuid))?
                    .ok_or_else(|| lesson_error("lesson referenced an absent existing concept"))?;
                if !bootstrap_reference_lifecycle(concept.lifecycle) {
                    return Err(lesson_error(
                        "lesson referenced inactive or unreviewed existing knowledge",
                    ));
                }
                Ok(concept)
            }
        }
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
            "authoringProtocol": authoring_protocol(),
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
                "For deterministic generalizable tasks, return one safe reusable_lesson in pure_rpn_v1 and its example answer. For external observations without a trusted primitive, return external_observation with no lesson. Otherwise answer or explicitly abstain."
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
        knowledge_to_learn: Option<KnowledgeToLearn>,
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
                    self.persist_engine_episode(&episode)?;
                    return Ok(CycleProgress::Completed(Box::new(CycleOutcome {
                        cycle_id,
                        disposition: CycleDisposition::Abstained,
                        answer: None,
                        episode,
                    })));
                }
                let semantic_verified = rung == EscalationRung::Run
                    && knowledge_to_learn.is_none()
                    && episode.context.held_contradictions.is_empty()
                    && episode.context.unresolved_refinements.is_empty()
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
                if semantic_verified {
                    episode
                        .observed_facts
                        .push(self.observed_fact_for_procedure(
                            procedure,
                            value.clone(),
                            inputs.clone(),
                        ));
                }
                let durable_lesson_stage = if let Some(KnowledgeToLearn::Lesson(lesson)) =
                    &knowledge_to_learn
                {
                    enrich_episode_for_lesson(&mut episode, &lesson.interpretation)?;
                    let binding_bytes = serde_json::to_vec(
                        episode
                            .teacher_interaction
                            .as_ref()
                            .unwrap_or(&JsonValue::Null),
                    )?;
                    let request_binding_digest = format!(
                        "sha256:{}",
                        hex_bytes(&lesson_sha256(
                            b"ekg:teacher-lesson:request-binding:v1",
                            &binding_bytes,
                        ))
                    );
                    let stage_identity =
                        serde_json::to_vec(&(&lesson.idempotency_key, &request_binding_digest))?;
                    let stage_id = format!(
                        "lesson-stage:{}",
                        hex_bytes(&lesson_sha256(
                            b"ekg:teacher-lesson:stage:v1",
                            &stage_identity,
                        ))
                    );
                    let stage = DurableLessonStage {
                        stage_id,
                        bundle_key: lesson.idempotency_key.clone(),
                        request_binding_digest,
                        concepts: lesson.concepts.clone(),
                        relationships: lesson.relationships.clone(),
                        procedures: lesson.procedures.clone(),
                        episode: episode.clone(),
                    };
                    self.lesson_stages.stage(&stage)?;
                    Some(stage)
                } else {
                    None
                };
                let integration = match &knowledge_to_learn {
                    None => Ok(()),
                    Some(KnowledgeToLearn::LegacyProcedure) => {
                        self.graph.insert_procedure(procedure)?;
                        self.index_procedure(procedure)
                    }
                    Some(KnowledgeToLearn::Lesson(lesson)) => {
                        self.graph.insert_knowledge_bundle(
                            &lesson.idempotency_key,
                            &lesson.concepts,
                            &lesson.relationships,
                            &lesson.procedures,
                        )?;
                        for concept in &lesson.concepts {
                            self.index_concept(concept)?;
                        }
                        for procedure in &lesson.procedures {
                            self.index_procedure(procedure)?;
                        }
                        Ok(())
                    }
                };
                if let Err(error) = integration {
                    if let Some(stage) = &durable_lesson_stage {
                        self.lesson_stages.discard(stage)?;
                    }
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
                    self.persist_engine_episode(&episode)?;
                    return Ok(CycleProgress::Completed(Box::new(CycleOutcome {
                        cycle_id,
                        disposition: CycleDisposition::Abstained,
                        answer: None,
                        episode,
                    })));
                }
                self.persist_engine_episode(&episode)?;
                if let Some(stage) = &durable_lesson_stage {
                    self.lesson_stages.complete(stage)?;
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
                    episode.evaluation = Some(Evaluation {
                        tier: VerifiabilityTier::Hard,
                        success: false,
                        details: error.to_string(),
                        surprise: Some(1.0),
                    });
                    episode.action = Some("failed:awaiting-teacher".into());
                    episode.reasoning_trace.steps.push(simple_step(
                        "persist failed local attempt before teacher escalation",
                        EscalationRung::Ask,
                    ));
                    let mut pending_input = input.clone();
                    pending_input.budget.max_exec_steps = pending_input
                        .budget
                        .max_exec_steps
                        .saturating_sub(steps_used);
                    let request = self.teacher_request(&pending_input, interpretations)?;
                    pending_input.budget.max_teacher_turns =
                        pending_input.budget.max_teacher_turns.saturating_sub(1);
                    let pending = PendingCycle {
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
                    };
                    // The failed attempt saga and pending continuation are
                    // staged together before the episode/trust work begins.
                    // A crash can therefore lose neither side independently.
                    self.persist_engine_episode_with_pending(&episode, cycle_id, &pending)?;
                    self.pending_cycles.insert(cycle_id, pending);
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
                self.persist_engine_episode(&episode)?;
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
        self.persist_engine_episode(&episode)?;
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
        let mut held = Vec::new();
        let mut applied_refinements = Vec::new();
        let mut unresolved_refinements = Vec::new();
        for interpretation in interpretations {
            let predicate = format!("concept:{}", interpretation.concept.id);
            held.extend(
                self.held_contradictions_for_predicate(&predicate)?
                    .into_iter()
                    .map(|id| id.0),
            );
            if let ekg_adapt::Uncertainty::HeldContradictions(inherited) =
                self.uncertainty_for_claim(&predicate)?
            {
                held.extend(inherited.into_iter().map(|id| id.0));
            }
            let refinement =
                self.refinement_context_for_predicate(&predicate, &input.environment)?;
            applied_refinements.extend(refinement.applied.into_iter().map(|applied| {
                ekg_core::ContextRefinement {
                    contradiction_id: applied.contradiction_id.0,
                    claim_id: applied.claim.id,
                    predicate: applied.claim.implication.predicate,
                    value: applied.claim.implication.value,
                }
            }));
            unresolved_refinements.extend(
                refinement
                    .unresolved
                    .into_iter()
                    .map(|contradiction| contradiction.0),
            );
        }
        held.sort_unstable();
        held.dedup();
        applied_refinements.sort_by(|left, right| {
            left.contradiction_id
                .cmp(&right.contradiction_id)
                .then_with(|| left.claim_id.cmp(&right.claim_id))
        });
        applied_refinements.dedup_by(|left, right| {
            left.contradiction_id == right.contradiction_id && left.claim_id == right.claim_id
        });
        unresolved_refinements.sort_unstable();
        unresolved_refinements.dedup();
        episode.context.held_contradictions = held;
        episode.context.applied_refinements = applied_refinements;
        episode.context.unresolved_refinements = unresolved_refinements;
        Ok(episode)
    }
}

fn action_procedure_id(action: &str) -> Option<ekg_core::ProcedureId> {
    let value = action.strip_prefix("procedure:")?.split('@').next()?;
    uuid::Uuid::parse_str(value).ok().map(ekg_core::ProcedureId)
}

fn compile_lesson_contract(
    draft: &ContractDraft,
    parameters: &HashSet<String>,
) -> Result<Contract, EngineError> {
    if draft.requires.len() > MAX_LESSON_CONDITIONS
        || draft.promises.len() > MAX_LESSON_CONDITIONS
        || draft.fails_when.len() > MAX_LESSON_CONDITIONS
    {
        return Err(lesson_error(
            "contract condition collection exceeds its bound",
        ));
    }
    Ok(Contract {
        requires: compile_lesson_conditions(&draft.requires, parameters, false)?,
        promises: compile_lesson_conditions(&draft.promises, parameters, true)?,
        fails_when: compile_lesson_conditions(&draft.fails_when, parameters, false)?,
        ..Contract::default()
    })
}

fn enrich_episode_for_lesson(
    episode: &mut Episode,
    interpretation: &ResolvedInterpretation,
) -> Result<(), EngineError> {
    let set = InterpretationSet::try_new(
        vec![InterpretationCandidate {
            meaning: interpretation.concept.id,
            weight: 1.0,
        }],
        Some(interpretation.concept.id),
    )
    .map_err(|error| EngineError::InvalidInput(error.to_string()))?;
    let persisted = set.to_episode_interpretations();
    episode.interpretations = persisted.clone();
    episode.context.interpretations = persisted;
    episode.context.entities = vec![interpretation.concept.id];
    episode.knowledge_considered = vec![KnowledgeCandidate {
        concept: interpretation.concept.id,
        relevance_score: 1.0,
        was_used: true,
    }];
    Ok(())
}

fn compile_lesson_conditions(
    drafts: &[ConditionDraft],
    parameters: &HashSet<String>,
    allow_result: bool,
) -> Result<Vec<Condition>, EngineError> {
    drafts
        .iter()
        .map(|draft| {
            if draft.description.trim().is_empty()
                || draft.description.chars().count() > MAX_TEACHER_TEXT_CHARS
            {
                return Err(lesson_error(
                    "contract descriptions must be nonempty and bounded",
                ));
            }
            let (check, _) = compile_lesson_program(&draft.check, parameters, allow_result)?;
            Ok(Condition::described(&draft.description).with_check(check))
        })
        .collect()
}

fn compile_lesson_program(
    draft: &ProgramDraft,
    parameters: &HashSet<String>,
    allow_result: bool,
) -> Result<(Expr, HashSet<String>), EngineError> {
    if draft.instructions.is_empty() || draft.instructions.len() > MAX_LESSON_INSTRUCTIONS {
        return Err(lesson_error(
            "RPN programs must contain 1..=64 instructions",
        ));
    }
    let mut stack = Vec::new();
    let mut used_parameters = HashSet::new();
    for instruction in &draft.instructions {
        match instruction {
            InstructionDraft::LoadParameter { name } => {
                if !parameters.contains(name) {
                    return Err(lesson_error(
                        "RPN program referenced an undeclared parameter",
                    ));
                }
                used_parameters.insert(name.clone());
                stack.push(Expr::Var(name.clone()));
            }
            InstructionDraft::LoadResult => {
                if !allow_result {
                    return Err(lesson_error(
                        "load_result is allowed only in promise checks",
                    ));
                }
                stack.push(Expr::Var("result".into()));
            }
            InstructionDraft::PushLiteral { value } => {
                if !lesson_scalar(value) {
                    return Err(lesson_error("RPN literals must be scalar values"));
                }
                stack.push(Expr::Literal(value.clone()));
            }
            InstructionDraft::Add => push_binary(&mut stack, BinOp::Add)?,
            InstructionDraft::Subtract => push_binary(&mut stack, BinOp::Sub)?,
            InstructionDraft::Multiply => push_binary(&mut stack, BinOp::Mul)?,
            InstructionDraft::Divide => push_binary(&mut stack, BinOp::Div)?,
            InstructionDraft::Modulo => push_binary(&mut stack, BinOp::Mod)?,
            InstructionDraft::Equal => push_binary(&mut stack, BinOp::Eq)?,
            InstructionDraft::NotEqual => push_binary(&mut stack, BinOp::Ne)?,
            InstructionDraft::LessThan => push_binary(&mut stack, BinOp::Lt)?,
            InstructionDraft::LessOrEqual => push_binary(&mut stack, BinOp::Le)?,
            InstructionDraft::GreaterThan => push_binary(&mut stack, BinOp::Gt)?,
            InstructionDraft::GreaterOrEqual => push_binary(&mut stack, BinOp::Ge)?,
            InstructionDraft::And => push_binary(&mut stack, BinOp::And)?,
            InstructionDraft::Or => push_binary(&mut stack, BinOp::Or)?,
            InstructionDraft::Negate => push_unary(&mut stack, UnOp::Neg)?,
            InstructionDraft::Not => push_unary(&mut stack, UnOp::Not)?,
        }
    }
    if stack.len() != 1 {
        return Err(lesson_error(
            "RPN program must leave exactly one value on the stack",
        ));
    }
    Ok((stack.pop().expect("stack length checked"), used_parameters))
}

fn push_binary(stack: &mut Vec<Expr>, op: BinOp) -> Result<(), EngineError> {
    let right = stack
        .pop()
        .ok_or_else(|| lesson_error("RPN binary operation underflow"))?;
    let left = stack
        .pop()
        .ok_or_else(|| lesson_error("RPN binary operation underflow"))?;
    stack.push(Expr::BinOp {
        op,
        left: Box::new(left),
        right: Box::new(right),
    });
    Ok(())
}

fn push_unary(stack: &mut Vec<Expr>, op: UnOp) -> Result<(), EngineError> {
    let operand = stack
        .pop()
        .ok_or_else(|| lesson_error("RPN unary operation underflow"))?;
    stack.push(Expr::UnOp {
        op,
        operand: Box::new(operand),
    });
    Ok(())
}

fn lesson_error(message: impl Into<String>) -> EngineError {
    EngineError::InvalidInput(format!("unsafe reusable lesson: {}", message.into()))
}

fn validate_lesson_token(value: &str, label: &str, maximum: usize) -> Result<(), EngineError> {
    if value.trim().is_empty() || value.chars().count() > maximum {
        return Err(lesson_error(format!(
            "{label} must be nonempty and at most {maximum} characters"
        )));
    }
    Ok(())
}

fn lesson_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Text(_)
    )
}

fn bootstrap_reference_lifecycle(lifecycle: Lifecycle) -> bool {
    matches!(
        lifecycle,
        Lifecycle::Active | Lifecycle::Validated | Lifecycle::Provisional
    )
}

fn stable_lesson_digest(bytes: &[u8]) -> String {
    let digest = lesson_sha256(b"ekg:teacher-lesson:idempotency:v1", bytes);
    format!("sha256:{}", hex_bytes(&digest))
}

fn deterministic_lesson_uuid(canonical: &[u8], entity_kind: &str, key: &str) -> Uuid {
    let mut identity = Vec::with_capacity(canonical.len() + entity_kind.len() + key.len() + 2);
    identity.extend_from_slice(entity_kind.as_bytes());
    identity.push(0);
    identity.extend_from_slice(key.as_bytes());
    identity.push(0);
    identity.extend_from_slice(canonical);
    let digest = lesson_sha256(b"ekg:teacher-lesson:entity-uuid:v1", &identity);
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 9562 variant with a locally defined/version-8 deterministic UUID.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn lesson_sha256(domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    hasher.finalize().into()
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to a string cannot fail");
    }
    output
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
    let scalar = json!({ "type": ["null", "boolean", "number", "string"] });
    let named_value = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "name": { "type": "string" },
            "value": scalar,
        },
        "required": ["name", "value"],
    });
    let concept_reference = json!({
        "anyOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string", "const": "new_concept" },
                    "key": { "type": "string" },
                },
                "required": ["kind", "key"],
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string", "const": "existing_concept" },
                    "id": { "type": "string" },
                },
                "required": ["kind", "id"],
            },
        ]
    });
    let program = lesson_program_schema();
    let condition = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "description": { "type": "string" },
            "check": program,
        },
        "required": ["description", "check"],
    });
    let lesson = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "primitiveSet": { "type": "string", "const": "pure_rpn_v1" },
            "concepts": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_LESSON_CONCEPTS,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "key": { "type": "string" },
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                    },
                    "required": ["key", "name", "description"],
                },
            },
            "relationships": {
                "type": "array",
                "maxItems": MAX_LESSON_RELATIONSHIPS,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "source": concept_reference,
                        "target": concept_reference,
                        "kind": { "type": "string" },
                        "strength": { "type": "number", "minimum": 0, "maximum": 1 },
                    },
                    "required": ["source", "target", "kind", "strength"],
                },
            },
            "procedures": {
                "type": "array",
                "minItems": 1,
                "maxItems": 1,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "key": { "type": "string" },
                        "name": { "type": "string" },
                        "concept": concept_reference,
                        "parameters": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": MAX_LESSON_PARAMETERS,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "name": { "type": "string" },
                                    "description": { "type": "string" },
                                },
                                "required": ["name", "description"],
                            },
                        },
                        "body": program,
                        "contract": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "requires": { "type": "array", "maxItems": MAX_LESSON_CONDITIONS, "items": condition },
                                "promises": { "type": "array", "maxItems": MAX_LESSON_CONDITIONS, "items": condition },
                                "failsWhen": { "type": "array", "maxItems": MAX_LESSON_CONDITIONS, "items": condition },
                            },
                            "required": ["requires", "promises", "failsWhen"],
                        },
                    },
                    "required": ["key", "name", "concept", "parameters", "body", "contract"],
                },
            },
            "invocation": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "procedureKey": { "type": "string" },
                    "inputs": { "type": "array", "maxItems": MAX_LESSON_PARAMETERS, "items": named_value },
                },
                "required": ["procedureKey", "inputs"],
            },
        },
        "required": ["primitiveSet", "concepts", "relationships", "procedures", "invocation"],
    });
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "proposalKind": {
                "type": "string",
                "enum": ["reusable_lesson", "external_observation", "answer_only", "abstain"]
            },
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
            "lesson": { "anyOf": [lesson, { "type": "null" }] },
            "procedure": {
                "type": "null",
                "description": "Compatibility field. Trusted procedure fields are engine-authored; always null."
            },
            "answer": { "type": ["null", "boolean", "number", "string"] },
            "abstainReason": { "type": ["string", "null"] }
        },
        "required": ["proposalKind", "interpretations", "lesson", "procedure", "answer", "abstainReason"]
    })
}

fn lesson_program_schema() -> JsonValue {
    let scalar = json!({ "type": ["null", "boolean", "number", "string"] });
    let named = |op: &str| {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "op": { "type": "string", "const": op }, "name": { "type": "string" } },
            "required": ["op", "name"],
        })
    };
    let unit = |op: &str| {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "op": { "type": "string", "const": op } },
            "required": ["op"],
        })
    };
    let mut instructions = vec![
        named("load_parameter"),
        unit("load_result"),
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "op": { "type": "string", "const": "push_literal" }, "value": scalar },
            "required": ["op", "value"],
        }),
    ];
    for op in [
        "add",
        "subtract",
        "multiply",
        "divide",
        "modulo",
        "equal",
        "not_equal",
        "less_than",
        "less_or_equal",
        "greater_than",
        "greater_or_equal",
        "and",
        "or",
        "negate",
        "not",
    ] {
        instructions.push(unit(op));
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "instructions": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_LESSON_INSTRUCTIONS,
                "items": { "anyOf": instructions },
            },
        },
        "required": ["instructions"],
    })
}

fn authoring_protocol() -> JsonValue {
    json!({
        "primitiveSet": "pure_rpn_v1",
        "proposalKinds": ["reusable_lesson", "external_observation", "answer_only", "abstain"],
        "instructions": [
            "load_parameter", "load_result", "push_literal", "add", "subtract",
            "multiply", "divide", "modulo", "equal", "not_equal", "less_than",
            "less_or_equal", "greater_than", "greater_or_equal", "and", "or",
            "negate", "not"
        ],
        "teacherProvides": [
            "concept name and description", "relationship claim", "procedure parameters",
            "pure RPN body", "executable contract checks", "example invocation and answer"
        ],
        "engineProvides": [
            "ids", "mutability", "lifecycle", "version", "confidence", "timestamps", "test cases"
        ],
        "constraints": [
            "pure deterministic scalar expressions only",
            "body must use every declared parameter",
            "no calls, effects, sensors, clocks, network, files, randomness, or opaque code",
            "external observations without a trusted sensor remain provisional answers and must not include a lesson"
        ]
    })
}

fn deserialize_optional_procedure<'de, D>(deserializer: D) -> Result<Option<Procedure>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<JsonValue>::deserialize(deserializer)?;
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        // An invalid optional procedure must never be executed or learned, but
        // it also must not discard an independently useful provisional answer.
        Some(JsonValue::String(json)) => Ok(serde_json::from_str(&json).ok()),
        Some(value) => Ok(serde_json::from_value(value).ok()),
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
    let source_matches_provider =
        matches!(provider, "claude" | "codex" | "openai" | "ollama" | "human")
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
