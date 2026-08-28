use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use spoon_core::{
    Assumption, BinOp, Concept, ConceptId, Condition, Contract, Episode, EpisodeCost, EpisodeId,
    EscalationRung, Evaluation, Expr, IntentDisposition, InterpretationProposal, IntrinsicOp,
    KnowledgeCandidate, Lifecycle, MutabilityClass, Param, ParamType, Procedure, ProcedureId,
    ReasoningTrace, Relationship, SessionId, SessionVisibility, SpoonError, TokenKind, TokenRange,
    TokenStream, TraceStep, TraceStepStatus, UnOp, Value, VerifiabilityTier, tokenize,
};
use spoon_episode::EpisodeRecallMode;
use spoon_exec::ExecTrace;
use spoon_reason::{
    ContextAssembler, ContextConfig, ContextRequest, InterpretationCandidate, InterpretationSet,
    RemainingBudget,
};
use uuid::Uuid;

use crate::engine::{
    Engine, EngineError, bind_inputs, is_current_executable, reasoning_trace, routing_description,
};
use crate::lesson::DurableLessonStage;
use spoon_intuition::RecallKind;

const MAX_TEACHER_CONTEXT_ITEMS: usize = 64;
const MAX_TEACHER_TEXT_CHARS: usize = 2_048;
const MAX_TEACHER_VALUE_DEPTH: usize = 8;
const MAX_TEACHER_CONTEXT_NODES: usize = 8_192;
const MAX_TEACHER_CONTEXT_CHARS: usize = 262_144;
const MAX_INTERPRETER_PRIOR_TURNS: usize = 8;
const MAX_ROUTING_RECALL_CANDIDATES: usize = 1_024;
const MAX_LESSON_CONCEPTS: usize = 8;
const MAX_LESSON_RELATIONSHIPS: usize = 16;
const MAX_LESSON_PROCEDURES: usize = 4;
const MAX_LESSON_PARAMETERS: usize = 16;
const MAX_LESSON_CONDITIONS: usize = 16;
const MAX_LESSON_INSTRUCTIONS: usize = 64;
const MAX_LESSON_KEY_CHARS: usize = 128;
const MAX_LESSON_NAME_CHARS: usize = 256;
const MAX_LESSON_EXPR_NODES: usize = 512;
const MAX_LESSON_EXPR_DEPTH: usize = 32;
const MAX_LESSON_EXPR_CHILDREN: usize = 64;
const MAX_LESSON_VALUE_ITEMS: usize = 64;
const MAX_LESSON_VALUE_DEPTH: usize = 8;
const MAX_LESSON_DEPENDENCIES: usize = 16;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecallMode {
    #[default]
    Global,
    Session,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleInput {
    pub situation: String,
    #[serde(default)]
    pub working_directory: Option<String>,
    pub environment: BTreeMap<String, Value>,
    pub assumptions: Vec<Assumption>,
    pub budget: CycleBudget,
    pub teacher_allowed: bool,
    #[serde(default)]
    pub interpreter_allowed: bool,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub recall_mode: RecallMode,
    #[serde(default)]
    pub permission_mode: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub procedure_ir: Option<JsonValue>,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentRequestWire {
    pub situation: String,
    pub token_stream: TokenStream,
    pub context: JsonValue,
    pub desired_output: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentProposalWire {
    pub content: InterpretationProposal,
    pub source: String,
    pub status: String,
    pub provenance: JsonValue,
    #[serde(default)]
    pub raw_content: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CycleProgress {
    NeedIntent {
        cycle_id: CycleId,
        request: IntentRequestWire,
    },
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
    #[serde(default)]
    dependency_allowlist: Vec<TeacherDependency>,
    initial_interpretations: Vec<ResolvedInterpretation>,
    prior_failure: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingIntentCycle {
    input: CycleInput,
    request: IntentRequestWire,
    bindings: Vec<IntentCandidateBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntentCandidateBinding {
    alias: String,
    concept: Concept,
    procedure: Procedure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "state", rename_all = "snake_case")]
enum PersistedPendingCycle {
    Teacher(PendingCycle),
    Intent(PendingIntentCycle),
}

/// An engine-owned reference captured with the Teacher request. This is never
/// supplied by the Teacher: it maps a short request-local alias to the exact
/// stored revision that was eligible when the request was issued.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TeacherDependency {
    alias: String,
    procedure: ProcedureId,
    version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolvedInterpretation {
    concept: Concept,
    weight: f64,
    inputs: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProposalContent {
    #[serde(default)]
    proposal_kind: Option<ProposalKind>,
    #[serde(default)]
    interpretations: Vec<ProposalInterpretation>,
    #[serde(default)]
    lesson: Option<JsonValue>,
    #[serde(default)]
    source: Option<String>,
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
    #[serde(default)]
    mutability: LessonConceptMutability,
}

/// A teacher may introduce an explicit definition or a defeasible general
/// claim alongside executable knowledge. It cannot mint particulars,
/// normative goals, or core machinery through a reusable lesson.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
enum LessonConceptMutability {
    Definitional,
    DefeasibleGeneral,
    #[default]
    Procedural,
}

impl From<LessonConceptMutability> for MutabilityClass {
    fn from(value: LessonConceptMutability) -> Self {
        match value {
            LessonConceptMutability::Definitional => Self::Definitional,
            LessonConceptMutability::DefeasibleGeneral => Self::DefeasibleGeneral,
            LessonConceptMutability::Procedural => Self::Procedural,
        }
    }
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
    body: JsonValue,
    contract: ContractDraft,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ParameterDraft {
    name: String,
    description: String,
    #[serde(default = "default_parameter_type")]
    value_type: ParamType,
}

fn default_parameter_type() -> ParamType {
    ParamType::Any
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
    check: JsonValue,
}

/// The versioned recursive grammar used by `pure_expr_v2`. The name is kept
/// for wire compatibility, but the grammar now has one explicit effect node:
/// a capability call is data-only until the host re-authorizes it at runtime.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
enum ExprDraft {
    Literal {
        value: Value,
    },
    Parameter {
        name: String,
    },
    Result,
    Binary {
        op: BinaryOpDraft,
        left: Box<ExprDraft>,
        right: Box<ExprDraft>,
    },
    Unary {
        op: UnaryOpDraft,
        operand: Box<ExprDraft>,
    },
    If {
        condition: Box<ExprDraft>,
        then: Box<ExprDraft>,
        #[serde(rename = "else")]
        else_: Box<ExprDraft>,
    },
    Let {
        name: String,
        value: Box<ExprDraft>,
        body: Box<ExprDraft>,
    },
    List {
        items: Vec<ExprDraft>,
    },
    Index {
        collection: Box<ExprDraft>,
        index: Box<ExprDraft>,
    },
    Field {
        object: Box<ExprDraft>,
        field: String,
    },
    Map {
        collection: Box<ExprDraft>,
        var: String,
        body: Box<ExprDraft>,
    },
    Filter {
        collection: Box<ExprDraft>,
        var: String,
        predicate: Box<ExprDraft>,
    },
    Reduce {
        collection: Box<ExprDraft>,
        init: Box<ExprDraft>,
        acc: String,
        var: String,
        body: Box<ExprDraft>,
    },
    Intrinsic {
        version: u16,
        op: IntrinsicOpDraft,
        args: Vec<ExprDraft>,
    },
    Dependency {
        alias: String,
        args: Vec<ExprDraft>,
    },
    CapabilityCall {
        #[serde(rename = "contentId")]
        content_id: String,
        #[serde(rename = "procedureId")]
        procedure_id: String,
        input: Box<ExprDraft>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum BinaryOpDraft {
    Add,
    #[serde(alias = "sub")]
    Subtract,
    #[serde(alias = "mul")]
    Multiply,
    #[serde(alias = "div")]
    Divide,
    #[serde(alias = "mod")]
    Modulo,
    Equal,
    NotEqual,
    LessThan,
    LessOrEqual,
    GreaterThan,
    GreaterOrEqual,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum UnaryOpDraft {
    #[serde(alias = "neg")]
    Negate,
    Not,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum IntrinsicOpDraft {
    Length,
    TextByteLength,
    TextScalarLength,
    TextGraphemeLength,
    TextTokenize,
    TextSplit,
    TextJoin,
    TextTrim,
    TextLowercase,
    TextUppercase,
    TextContains,
    TextStartsWith,
    TextEndsWith,
    TextReplace,
    TextUrlEncode,
    TextRegexCapture,
    CollectionContains,
    CollectionFindIndex,
    CountEqual,
    MapKeys,
    MapValues,
    JsonParse,
    JsonStringify,
    PathGet,
    PathGetOptional,
    JsonPointerGet,
    JsonPointerGetOptional,
    JsonPointerSet,
    JsonPointerDelete,
    Coalesce,
    TextNormalizeNfc,
    TextNormalizeNfd,
    TextNormalizeNfkc,
    TextNormalizeNfkd,
    TextTrimStart,
    TextTrimEnd,
    TextGraphemeSubstring,
    TextIndexOf,
    TextCount,
    TextRepeat,
    TextConcatMany,
    MapEntries,
    MapFromEntries,
    MapSet,
    MapDelete,
    MapMerge,
    CollectionSlice,
    CollectionReverse,
    CollectionSort,
    CollectionUnique,
    CollectionFlatten,
    CollectionZip,
    Range,
    TypeName,
    ParseInt,
    ParseFloat,
    ParseBool,
    ToText,
    NumericAbs,
    NumericSign,
    NumericMin,
    NumericMax,
    NumericClamp,
    NumericFloor,
    NumericCeil,
    NumericRound,
    NumericTruncate,
    NumericPowInt,
    NumericPowFloat,
    IntegerQuotient,
    IntegerRemainder,
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
    allows_structured_values: bool,
    concepts: Vec<Concept>,
    relationships: Vec<Relationship>,
    procedures: Vec<Procedure>,
    invocation_procedure: Procedure,
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
            self.runtime
                .claim_pending_cycle(cycle_id, self.instance_id)?;
            match serde_json::from_str::<PersistedPendingCycle>(&pending_json) {
                Ok(PersistedPendingCycle::Teacher(pending)) => {
                    self.pending_cycles.insert(cycle_id, pending);
                }
                Ok(PersistedPendingCycle::Intent(pending)) => {
                    self.pending_intents.insert(cycle_id, pending);
                }
                Err(_) => {
                    // Backward compatibility for pending Teacher continuations
                    // written before the continuation kind was explicit.
                    let pending: PendingCycle = serde_json::from_str(&pending_json)?;
                    self.pending_cycles.insert(cycle_id, pending);
                }
            }
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
        if !is_explicit_teaching_request(&input.situation) {
            if let Some(answer) = self.system_capability_answer(&input.situation)? {
                return self.complete_simple(
                    cycle_id,
                    &input,
                    CycleDisposition::Verified,
                    Some(Value::Text(answer)),
                    "system:self-capability-query",
                    EscalationRung::Run,
                    Some(Evaluation {
                        tier: VerifiabilityTier::Hard,
                        success: true,
                        details: "reported the Engine-owned capability registry".into(),
                        surprise: None,
                    }),
                    None,
                    Vec::new(),
                );
            }
        }

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

        // `teach` is an explicit request to author or revise durable
        // knowledge. It must not silently substitute a semantically nearby
        // existing procedure before the Teacher sees the user's instruction.
        if is_explicit_teaching_request(&input.situation)
            && input.teacher_allowed
            && input.budget.max_teacher_turns > 0
        {
            let interpretations = Vec::new();
            let dependency_allowlist = self.pure_teacher_dependencies()?;
            let request = self.teacher_request(&input, &interpretations, &dependency_allowlist)?;
            let mut pending_input = input;
            pending_input.budget.max_teacher_turns =
                pending_input.budget.max_teacher_turns.saturating_sub(1);
            self.pending_cycles.insert(
                cycle_id,
                PendingCycle {
                    input: pending_input,
                    request: request.clone(),
                    dependency_allowlist,
                    initial_interpretations: interpretations,
                    prior_failure: None,
                },
            );
            return Ok(CycleProgress::NeedTeacher { cycle_id, request });
        }

        // A bare correction such as "incorrect" is feedback about the last
        // result, not a fresh request. It must not be resolved locally and
        // accidentally execute a semantically unrelated stored procedure.
        if is_explicit_incorrectness_feedback(&input.situation)
            && input.teacher_allowed
            && input.budget.max_teacher_turns > 0
        {
            let interpretations = Vec::new();
            let dependency_allowlist = self.pure_teacher_dependencies()?;
            let request = self.teacher_request(&input, &interpretations, &dependency_allowlist)?;
            let mut pending_input = input;
            pending_input.budget.max_teacher_turns =
                pending_input.budget.max_teacher_turns.saturating_sub(1);
            self.pending_cycles.insert(
                cycle_id,
                PendingCycle {
                    input: pending_input,
                    request: request.clone(),
                    dependency_allowlist,
                    initial_interpretations: interpretations,
                    prior_failure: None,
                },
            );
            return Ok(CycleProgress::NeedTeacher { cycle_id, request });
        }

        let interpretations = self.local_interpretations(&input.situation)?;
        // When an interpreter is enabled, keep the local match as a
        // request-local candidate and require independent language grounding
        // before execution. The deterministic fast path remains available to
        // hosts that explicitly disable interpretation.
        if !input.interpreter_allowed
            && let Some(resolved) = uniquely_resolved(&interpretations)
            && let Some(procedure) = self.procedure_for(resolved.concept.id, &input.environment)?
            && let Some(inputs) = complete_inputs(
                &procedure,
                &input.environment,
                &resolved.inputs,
                &extract_literals(&input.situation),
            )
            && procedure_has_language_support(
                &input.situation,
                &IntentCandidateBinding {
                    alias: "local".into(),
                    concept: resolved.concept.clone(),
                    procedure: procedure.clone(),
                },
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

        // Candidate Laboratory (P0F.4): attempt to compose known procedures
        // before escalating to the interpreter or teacher.
        if let Some(progress) = self.attempt_compose_and_execute(
            cycle_id,
            &input,
            &interpretations,
        )? {
            return Ok(progress);
        }

        if input.interpreter_allowed {
            let (request, bindings) = self.intent_request(&input)?;
            if !bindings.is_empty() {
                self.pending_intents.insert(
                    cycle_id,
                    PendingIntentCycle {
                        input,
                        request: request.clone(),
                        bindings,
                    },
                );
                return Ok(CycleProgress::NeedIntent { cycle_id, request });
            }
        }

        if input.teacher_allowed && input.budget.max_teacher_turns > 0 {
            let dependency_allowlist = self.pure_teacher_dependencies()?;
            let request = self.teacher_request(&input, &interpretations, &dependency_allowlist)?;
            let mut pending_input = input;
            pending_input.budget.max_teacher_turns =
                pending_input.budget.max_teacher_turns.saturating_sub(1);
            self.pending_cycles.insert(
                cycle_id,
                PendingCycle {
                    input: pending_input,
                    request: request.clone(),
                    dependency_allowlist,
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

    fn intent_request(
        &self,
        input: &CycleInput,
    ) -> Result<(IntentRequestWire, Vec<IntentCandidateBinding>), EngineError> {
        let token_stream = tokenize(&input.situation)
            .map_err(|error| EngineError::InvalidInput(error.to_string()))?;
        let mut bindings = self
            .graph
            .list_procedures()?
            .into_iter()
            .filter(|procedure| is_current_executable(procedure.lifecycle))
            // A language model may only route into procedures whose slot
            // contract carries an actual type. Legacy procedures remain
            // directly callable, but prose descriptions are not enough to
            // make interpreter admission safe.
            .filter(|procedure| {
                procedure
                    .params
                    .iter()
                    .all(|param| param.value_type.is_some())
            })
            .filter_map(|procedure| {
                let concept_id = procedure.concept?;
                match self.graph.get_concept(concept_id) {
                    Ok(Some(concept)) if usable_lifecycle(concept.lifecycle) => {
                        Some(Ok((concept, procedure)))
                    }
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Multiple learning episodes may admit byte-for-byte equivalent IR
        // under differently worded concepts. Showing every duplicate wastes
        // scarce model context and makes tiny interpreters choose between
        // aliases that execute identically. Collapse only exact executable
        // semantics; differently contracted or implemented procedures remain.
        let mut seen_semantics = HashSet::new();
        let mut unique_bindings = Vec::with_capacity(bindings.len());
        for (concept, procedure) in bindings {
            let semantic_key =
                serde_json::to_string(&(&procedure.params, &procedure.body, &procedure.contract))?;
            if seen_semantics.insert(semantic_key) {
                unique_bindings.push((concept, procedure));
            }
        }
        bindings = unique_bindings;
        bindings.sort_by(
            |(left_concept, left_procedure), (right_concept, right_procedure)| {
                left_concept
                    .name
                    .cmp(&right_concept.name)
                    .then_with(|| left_procedure.name.cmp(&right_procedure.name))
                    .then_with(|| left_procedure.version.cmp(&right_procedure.version))
            },
        );
        let routing_strategy = if bindings.len() <= input.budget.max_context_items {
            "all"
        } else {
            let recall_limit = bindings
                .len()
                .saturating_mul(3)
                .max(input.budget.max_context_items)
                .min(MAX_ROUTING_RECALL_CANDIDATES);
            let ranked = self.rank_recall_candidates(&input.situation, recall_limit)?;
            let ranks = ranked
                .into_iter()
                .filter(|candidate| candidate.kind == RecallKind::Procedure)
                .enumerate()
                .map(|(rank, candidate)| (candidate.id, rank))
                .collect::<BTreeMap<_, _>>();
            bindings.sort_by(
                |(left_concept, left_procedure), (right_concept, right_procedure)| {
                    let left_id =
                        format!("procedure:{}:{}", left_procedure.id, left_procedure.version);
                    let right_id = format!(
                        "procedure:{}:{}",
                        right_procedure.id, right_procedure.version
                    );
                    ranks
                        .get(&left_id)
                        .copied()
                        .unwrap_or(usize::MAX)
                        .cmp(&ranks.get(&right_id).copied().unwrap_or(usize::MAX))
                        .then_with(|| left_concept.name.cmp(&right_concept.name))
                        .then_with(|| left_procedure.name.cmp(&right_procedure.name))
                        .then_with(|| left_procedure.version.cmp(&right_procedure.version))
                },
            );
            "hybrid"
        };
        bindings.truncate(input.budget.max_context_items);
        let mut bindings = bindings
            .into_iter()
            .enumerate()
            .map(|(index, (concept, procedure))| IntentCandidateBinding {
                alias: format!("candidate_{index}"),
                concept,
                procedure,
            })
            .collect::<Vec<_>>();

        let session_id = input
            .session_id
            .as_deref()
            .map(|value| Uuid::parse_str(value).map(SessionId))
            .transpose()
            .map_err(|_| EngineError::InvalidInput("session_id is not a UUID".into()))?;
        let recall_mode = match input.recall_mode {
            RecallMode::Global => EpisodeRecallMode::Global,
            RecallMode::Session => EpisodeRecallMode::Session,
            RecallMode::None => EpisodeRecallMode::None,
        };
        let recent = self.episodes.list_recent_for_recall(
            session_id,
            recall_mode,
            input.budget.max_context_items.min(u32::MAX as usize) as u32,
        )?;
        let reconsideration = if is_reconsideration_situation(&input.situation) {
            recent.iter().find_map(|episode| {
                let procedure = action_procedure_id(episode.action.as_deref()?)?;
                if !episode
                    .evaluation
                    .as_ref()
                    .is_some_and(|evaluation| evaluation.success)
                    || episode.observed_result.is_none()
                    || !episode
                        .action
                        .as_deref()
                        .is_some_and(|action| action.starts_with("procedure:"))
                {
                    return None;
                }
                Some((
                    procedure,
                    episode.situation.clone(),
                    episode
                        .observed_result
                        .clone()
                        .or_else(|| episode.prediction.clone()),
                    episode.execution_trace.as_ref().and_then(execution_inputs),
                ))
            })
        } else {
            None
        };
        let reconsideration_procedure = reconsideration.as_ref().map(|(id, _, _, _)| *id);
        if let Some(procedure_id) = reconsideration_procedure {
            let narrowed = bindings
                .iter()
                .filter(|binding| binding.procedure.id == procedure_id)
                .cloned()
                .collect::<Vec<_>>();
            if !narrowed.is_empty() {
                bindings = narrowed;
            }
        }
        let prior_turns = recent
            .into_iter()
            .take(MAX_INTERPRETER_PRIOR_TURNS)
            .enumerate()
            .map(|(index, episode)| {
                json!({
                    "alias": format!("turn_{index}"),
                    "situation": truncate_text(&episode.situation, MAX_TEACHER_TEXT_CHARS),
                    "succeeded": episode.evaluation.as_ref().map(|evaluation| evaluation.success),
                    "answer": episode.observed_result.as_ref().or(episode.prediction.as_ref()),
                    "actionKind": episode.action.as_deref().map(episode_action_kind),
                })
            })
            .collect::<Vec<_>>();
        let candidates = bindings
            .iter()
            .map(|binding| {
                let uses_primitives = procedure_intrinsic_names(&binding.procedure.body);
                json!({
                    "alias": binding.alias,
                    "name": truncate_text(&binding.concept.name, MAX_TEACHER_TEXT_CHARS),
                    "description": binding.concept.description.as_deref().map(|value| {
                        truncate_text(&routing_description(Some(value)), MAX_TEACHER_TEXT_CHARS)
                    }),
                    "procedure": {
                        "name": truncate_text(&binding.procedure.name, MAX_TEACHER_TEXT_CHARS),
                        "version": binding.procedure.version,
                        "usesPrimitives": uses_primitives,
                        "slots": binding.procedure.params.iter().map(|param| json!({
                            "name": truncate_text(&param.name, MAX_TEACHER_TEXT_CHARS),
                            "description": param.description.as_deref().map(|value| truncate_text(value, MAX_TEACHER_TEXT_CHARS)),
                            "valueType": param.value_type.map(|value| format!("{value:?}").to_ascii_lowercase()),
                        })).collect::<Vec<_>>(),
                    },
                })
            })
            .collect::<Vec<_>>();
        let procedure_catalog = candidates
            .iter()
            .map(|candidate| {
                json!({
                    "kind": "procedure",
                    "directlySelectable": true,
                    "alias": candidate["alias"],
                    "name": candidate["name"],
                    "description": candidate["description"],
                    "procedure": candidate["procedure"],
                    "usesPrimitives": candidate["procedure"]["usesPrimitives"],
                })
            })
            .collect::<Vec<_>>();
        let catalog_primitive_names = bindings
            .iter()
            .flat_map(|binding| procedure_intrinsic_names(&binding.procedure.body))
            .collect::<BTreeSet<_>>();
        // The interpreter may only ground slots in request-local literal spans.
        // Numbers alone made the constrained schema impossible for text-valued
        // procedures (for example, counting `"r"` in `Strawberry`). Include
        // bare words and complete quoted strings as well; Spoon still derives
        // the value locally from the selected token range.
        let literal_ranges = intent_literal_ranges(&token_stream)
            .into_iter()
            .enumerate()
            .map(|(index, range)| {
                let text = token_range_text(&token_stream, range).unwrap_or_default();
                json!({
                    "alias": format!("literal_{index}"),
                    "tokenRange": range,
                    "text": text,
                    "value": intent_literal_value(text),
                })
            })
            .collect::<Vec<_>>();
        let context = json!({
            "schemaVersion": 1,
            "recallScope": match input.recall_mode {
                RecallMode::Global => "global",
                RecallMode::Session => "session",
                RecallMode::None => "none",
            },
            "priorTurns": prior_turns,
            "reconsideration": reconsideration.as_ref().map(|(_, situation, answer, inputs)| json!({
                "candidateProcedure": reconsideration_procedure,
                "previousSituation": truncate_text(situation, MAX_TEACHER_TEXT_CHARS),
                "previousAnswer": answer,
                "previousInputs": inputs,
            })),
            "candidates": candidates,
            "catalog": {
                "procedures": procedure_catalog,
                "primitives": interpreter_primitive_catalog(&catalog_primitive_names),
                "capabilities": interpreter_capability_catalog(),
                "selectionRule": "Only procedure aliases marked directlySelectable may be selected. Primitives compose stored procedures; capabilities require a locally validated procedure and permission boundary.",
            },
            "retrieval": {
                "strategy": routing_strategy,
                "authority": "candidate_generation_only",
            },
            "literalCandidates": literal_ranges,
            "truncation": {
                "candidateLimit": input.budget.max_context_items,
                "priorTurnLimit": MAX_INTERPRETER_PRIOR_TURNS,
            },
        });
        Ok((
            IntentRequestWire {
                situation: input.situation.clone(),
                desired_output: intent_output_schema(
                    &bindings,
                    token_stream.tokens.len(),
                    &literal_ranges,
                ),
                token_stream,
                context,
            },
            bindings,
        ))
    }

    pub fn resume_intent(
        &mut self,
        cycle_id: CycleId,
        proposal: IntentProposalWire,
    ) -> Result<CycleProgress, EngineError> {
        self.runtime
            .assert_cycle_owner(cycle_id, self.instance_id)?;
        let result = self.resume_intent_inner(cycle_id, proposal);
        self.persist_cycle_result(cycle_id, &result)?;
        result
    }

    pub fn skip_intent(
        &mut self,
        cycle_id: CycleId,
        reason: impl Into<String>,
    ) -> Result<CycleProgress, EngineError> {
        self.skip_intent_with_diagnostic(cycle_id, reason, None)
    }

    pub fn skip_intent_with_diagnostic(
        &mut self,
        cycle_id: CycleId,
        reason: impl Into<String>,
        diagnostic: Option<JsonValue>,
    ) -> Result<CycleProgress, EngineError> {
        self.runtime
            .assert_cycle_owner(cycle_id, self.instance_id)?;
        let pending = self.pending_intents.remove(&cycle_id).ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "cycle {cycle_id} has no pending language interpretation"
            ))
        })?;
        let mut interaction = json!({
            "languageInterpreter": {
                "request": pending.request,
                "providerError": truncate_text(&reason.into(), MAX_TEACHER_TEXT_CHARS),
            }
        });
        if let Some(diagnostic) = diagnostic {
            if let Some(language_interpreter) = interaction
                .get_mut("languageInterpreter")
                .and_then(JsonValue::as_object_mut)
            {
                language_interpreter.insert("rejectedProposal".into(), diagnostic);
            }
        }
        let result = self.continue_after_intent(cycle_id, pending.input, interaction);
        self.persist_cycle_result(cycle_id, &result)?;
        result
    }

    fn resume_intent_inner(
        &mut self,
        cycle_id: CycleId,
        proposal: IntentProposalWire,
    ) -> Result<CycleProgress, EngineError> {
        let pending = self.pending_intents.remove(&cycle_id).ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "cycle {cycle_id} has no pending language interpretation"
            ))
        })?;
        if proposal.status != "unverified"
            || proposal.source.trim().is_empty()
            || !proposal.provenance.is_object()
        {
            return self.continue_after_intent(
                cycle_id,
                pending.input,
                rejected_intent_interaction(
                    &pending.request,
                    &proposal,
                    "language interpreter proposal provenance is invalid",
                ),
            );
        }
        let frames = match proposal.content.ground_for(
            &pending.request.token_stream,
            &spoon_core::LanguageLimits::default(),
        ) {
            Ok(frames) => frames,
            Err(error) => {
                return self.continue_after_intent(
                    cycle_id,
                    pending.input,
                    rejected_intent_interaction(&pending.request, &proposal, error.to_string()),
                );
            }
        };
        let mut interaction = json!({
            "languageInterpreter": {
                "request": pending.request,
                "source": proposal.source,
                "status": proposal.status,
                "provenance": proposal.provenance,
                "frames": frames,
            }
        });

        match frames.disposition {
            IntentDisposition::Execute => {
                let selected = frames.selected.expect("validated execute selection");
                let frame = &frames.candidates[selected];
                let binding = pending
                    .bindings
                    .iter()
                    .find(|binding| binding.alias == frame.name);
                let Some(binding) = binding else {
                    return self.continue_after_intent(
                        cycle_id,
                        pending.input,
                        rejected_intent_interaction(
                            &pending.request,
                            &proposal,
                            "interpreter selected an unknown request-local candidate",
                        ),
                    );
                };
                let current = self
                    .graph
                    .get_procedure_version(binding.procedure.id, binding.procedure.version)?;
                let Some(current) = current else {
                    return self.continue_after_intent(
                        cycle_id,
                        pending.input,
                        rejected_intent_interaction(
                            &pending.request,
                            &proposal,
                            "captured interpreter procedure revision is unavailable",
                        ),
                    );
                };
                if !is_current_executable(current.lifecycle)
                    || current.concept != Some(binding.concept.id)
                {
                    return self.continue_after_intent(
                        cycle_id,
                        pending.input,
                        rejected_intent_interaction(
                            &pending.request,
                            &proposal,
                            "captured interpreter candidate is no longer executable",
                        ),
                    );
                }
                if !procedure_has_language_support(&pending.input.situation, binding) {
                    return self.continue_after_intent(
                        cycle_id,
                        pending.input,
                        rejected_intent_interaction(
                            &pending.request,
                            &proposal,
                            "interpreter selected a procedure without enough language support",
                        ),
                    );
                }
                let allowed_slots = current
                    .params
                    .iter()
                    .map(|param| param.name.as_str())
                    .collect::<HashSet<_>>();
                let mut proposed_inputs = BTreeMap::new();
                for slot in &frame.slots {
                    if !allowed_slots.contains(slot.name.as_str()) {
                        return self.continue_after_intent(
                            cycle_id,
                            pending.input,
                            rejected_intent_interaction(
                                &pending.request,
                                &proposal,
                                format!("interpreter proposed unknown slot {:?}", slot.name),
                            ),
                        );
                    }
                    if proposed_inputs
                        .insert(slot.name.clone(), slot.value.clone())
                        .is_some()
                    {
                        return self.continue_after_intent(
                            cycle_id,
                            pending.input,
                            rejected_intent_interaction(
                                &pending.request,
                                &proposal,
                                format!("interpreter proposed duplicate slot {:?}", slot.name),
                            ),
                        );
                    }
                }
                let inputs = complete_inputs(
                    &current,
                    &pending.input.environment,
                    &proposed_inputs,
                    &extract_literals(&pending.input.situation),
                );
                let Some(inputs) = inputs else {
                    return self.continue_after_intent(
                        cycle_id,
                        pending.input,
                        rejected_intent_interaction(
                            &pending.request,
                            &proposal,
                            "interpreter proposal did not bind the exact procedure inputs",
                        ),
                    );
                };
                let interpretations = vec![ResolvedInterpretation {
                    concept: binding.concept.clone(),
                    weight: 1.0,
                    inputs: proposed_inputs.clone(),
                }];
                interaction["selectionReason"] = json!({
                    "path": "interpreter",
                    "summary": format!(
                        "Language interpreter selected {} (concept {:?}) with disposition Execute",
                        frame.name, binding.concept.id
                    ),
                    "source": proposal.source,
                    "selectedCandidate": frame.name,
                    "concept": binding.concept.id.to_string(),
                    "procedure": binding.procedure.id.to_string(),
                    "version": binding.procedure.version,
                    "disposition": "Execute",
                    "slotBindings": proposed_inputs,
                });
                self.execute_cycle_procedure(
                    cycle_id,
                    &pending.input,
                    &interpretations,
                    &current,
                    inputs,
                    EscalationRung::Run,
                    Some(interaction),
                    None,
                    None,
                )
            }
            IntentDisposition::Clarify => {
                let ambiguity = frames
                    .candidates
                    .iter()
                    .flat_map(|frame| frame.ambiguities.iter())
                    .next()
                    .map(String::as_str)
                    .unwrap_or("the request is ambiguous");
                self.complete_simple(
                    cycle_id,
                    &pending.input,
                    CycleDisposition::Abstained,
                    None,
                    &format!(
                        "clarify:{}",
                        truncate_text(ambiguity, MAX_TEACHER_TEXT_CHARS)
                    ),
                    EscalationRung::Abstain,
                    None,
                    Some(interaction),
                    Vec::new(),
                )
            }
            IntentDisposition::Abstain => {
                self.continue_after_intent(cycle_id, pending.input, interaction)
            }
        }
    }

    fn system_capability_answer(&self, situation: &str) -> Result<Option<String>, EngineError> {
        let lower = situation.to_ascii_lowercase();
        let asks_about_capabilities = lower.contains("capabilit")
            || lower.contains("what can you do")
            || lower.contains("what tools do you have")
            || (lower.contains("can you")
                && ["web", "internet", "search", "file", "network", "tool"]
                    .iter()
                    .any(|term| lower.contains(term)));
        if !asks_about_capabilities {
            return Ok(None);
        }

        let imported = self
            .capabilities
            .list_imported()
            .map_err(|error| EngineError::InvalidInput(format!("capability inventory: {error}")))?;
        let imported_summary = if imported.is_empty() {
            "No external capability bundles are currently imported.".to_owned()
        } else {
            imported
                .iter()
                .map(|capability| {
                    format!(
                        "{} ({}, locally validated: {})",
                        capability.name,
                        format!("{:?}", capability.status).to_ascii_lowercase(),
                        capability.locally_validated
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        };

        let asks_about_web = ["web", "internet", "search"]
            .iter()
            .any(|term| lower.contains(term));
        if asks_about_web {
            return Ok(Some(format!(
                "Web search is unavailable: no web-search adapter is registered. Spoon has a generic policy-authorized network_request boundary, but it requires an imported, locally validated capability procedure and permission. {imported_summary}"
            )));
        }

        Ok(Some(format!(
            "Spoon has built-in capability boundaries for policy-authorized network requests, file reads, file writes, external observation, and bounded sandbox execution. These are not automatically available; each requires a locally validated capability procedure and permission. {imported_summary}"
        )))
    }

    fn continue_after_intent(
        &mut self,
        cycle_id: CycleId,
        input: CycleInput,
        interaction: JsonValue,
    ) -> Result<CycleProgress, EngineError> {
        let interpretations = self.local_interpretations(&input.situation)?;
        if input.teacher_allowed && input.budget.max_teacher_turns > 0 {
            let dependency_allowlist = self.pure_teacher_dependencies()?;
            let request = self.teacher_request(&input, &interpretations, &dependency_allowlist)?;
            let mut pending_input = input;
            pending_input.budget.max_teacher_turns =
                pending_input.budget.max_teacher_turns.saturating_sub(1);
            self.pending_cycles.insert(
                cycle_id,
                PendingCycle {
                    input: pending_input,
                    request: request.clone(),
                    dependency_allowlist,
                    initial_interpretations: interpretations,
                    prior_failure: Some(interaction),
                },
            );
            return Ok(CycleProgress::NeedTeacher { cycle_id, request });
        }
        self.complete_simple(
            cycle_id,
            &input,
            CycleDisposition::Abstained,
            None,
            "abstain:language-interpreter",
            EscalationRung::Abstain,
            None,
            Some(interaction),
            interpretations,
        )
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

        let mut content: ProposalContent = match serde_json::from_value(proposal.content.clone()) {
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
        if let Err(error) = apply_spoonlang_source(&mut content) {
            if pending.input.budget.max_teacher_turns > 0 {
                return self.retry_reusable_lesson(
                    cycle_id,
                    pending,
                    teacher_json,
                    &error.to_string(),
                );
            }
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
                .and_then(|draft| self.compile_lesson(&draft, &pending.dependency_allowlist));
            match compiled {
                Ok(lesson) => {
                    if let Some(answer) = content.answer.as_ref()
                        && ((lesson.allows_structured_values
                            && validate_lesson_value(answer, 0).is_err())
                            || (!lesson.allows_structured_values && !lesson_scalar(answer)))
                    {
                        if pending.input.budget.max_teacher_turns > 0 {
                            return self.retry_reusable_lesson(
                                cycle_id,
                                pending,
                                teacher_json,
                                "lesson answer does not satisfy the selected primitive set",
                            );
                        }
                        return self.complete_simple(
                            cycle_id,
                            &pending.input,
                            CycleDisposition::Abstained,
                            None,
                            "abstain:unsafe-lesson-answer",
                            EscalationRung::Abstain,
                            None,
                            Some(teacher_json),
                            Vec::new(),
                        );
                    }
                    let procedure = lesson.invocation_procedure.clone();
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
            procedure.lifecycle = spoon_core::Lifecycle::Provisional;
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
            Ok(CycleProgress::NeedIntent { .. }) => {
                let pending = self.pending_intents.get(&cycle_id).ok_or_else(|| {
                    EngineError::InvalidInput(format!(
                        "cycle {cycle_id} requested interpretation without durable continuation state"
                    ))
                })?;
                self.runtime.save_pending_cycle(
                    cycle_id,
                    self.instance_id,
                    &serde_json::to_string(&PersistedPendingCycle::Intent(pending.clone()))?,
                )?;
            }
            Ok(CycleProgress::NeedTeacher { .. }) => {
                let pending = self.pending_cycles.get(&cycle_id).ok_or_else(|| {
                    EngineError::InvalidInput(format!(
                        "cycle {cycle_id} requested a teacher without durable continuation state"
                    ))
                })?;
                self.runtime.save_pending_cycle(
                    cycle_id,
                    self.instance_id,
                    &serde_json::to_string(&PersistedPendingCycle::Teacher(pending.clone()))?,
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
        if input.recall_mode == RecallMode::None {
            return Ok(None);
        }
        let session_id = input
            .session_id
            .as_deref()
            .map(|value| Uuid::parse_str(value).map(SessionId))
            .transpose()
            .map_err(|_| EngineError::InvalidInput("session_id is not a UUID".into()))?;
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
                Err(SpoonError::NotFound(_)) => continue,
                Err(error) => return Err(error.into()),
            };
            if episode.situation != input.situation
                || episode.context.environment != input.environment
                || (input.recall_mode == RecallMode::Global
                    && episode.session_visibility == SessionVisibility::Isolated)
                || (input.recall_mode == RecallMode::Session && episode.session_id != session_id)
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
        pending.dependency_allowlist = self.pure_teacher_dependencies()?;
        let mut request = self.teacher_request(
            &pending.input,
            &pending.initial_interpretations,
            &pending.dependency_allowlist,
        )?;
        request.specific_question = Some(format!(
            "The previous source was not valid spoonlang: {}. Put spoonlang text in source, not tagged IR JSON. If this situation is a stable fact with no inputs to transform, use kind answer_only. Do not copy an example lesson from the prompt.",
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

    fn compile_lesson(
        &self,
        draft: &LessonDraft,
        dependency_allowlist: &[TeacherDependency],
    ) -> Result<CompiledLesson, EngineError> {
        if !matches!(draft.primitive_set.as_str(), "pure_rpn_v1" | "pure_expr_v2") {
            return Err(lesson_error("unsupported primitive set"));
        }
        if draft.concepts.is_empty() || draft.concepts.len() > MAX_LESSON_CONCEPTS {
            return Err(lesson_error("lesson must introduce 1..=8 concepts"));
        }
        if draft.relationships.len() > MAX_LESSON_RELATIONSHIPS
            || draft.procedures.is_empty()
            || draft.procedures.len() > MAX_LESSON_PROCEDURES
        {
            return Err(lesson_error(
                "lesson must contain 0..=16 relationships and 1..=4 procedures",
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
            let mut concept = Concept::new(&concept_draft.name, concept_draft.mutability.into())
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
            relationship.id = spoon_core::RelationshipId(deterministic_lesson_uuid(
                &canonical,
                "relationship",
                &index.to_string(),
            ));
            relationship.strength = relationship_draft.strength;
            relationship.lifecycle = Lifecycle::Provisional;
            relationships.push(relationship);
        }

        let external_dependencies = dependency_allowlist
            .iter()
            .map(|dependency| (dependency.alias.clone(), dependency.clone()))
            .collect::<HashMap<_, _>>();
        if external_dependencies.len() != dependency_allowlist.len()
            || external_dependencies.len() > MAX_LESSON_DEPENDENCIES
        {
            return Err(lesson_error("invalid engine dependency allow-list"));
        }
        let existing_procedure_names = self
            .graph
            .list_procedures()?
            .into_iter()
            .map(|procedure| procedure.name.to_lowercase())
            .collect::<HashSet<_>>();
        let mut procedure_keys = HashSet::new();
        let mut procedure_names = HashSet::new();
        let mut lesson_dependencies = HashMap::new();
        for procedure_draft in &draft.procedures {
            validate_lesson_token(&procedure_draft.key, "procedure key", MAX_LESSON_KEY_CHARS)?;
            validate_lesson_token(
                &procedure_draft.name,
                "procedure name",
                MAX_LESSON_NAME_CHARS,
            )?;
            if !procedure_keys.insert(procedure_draft.key.clone())
                || !procedure_names.insert(procedure_draft.name.to_lowercase())
                || existing_procedure_names.contains(&procedure_draft.name.to_lowercase())
            {
                return Err(lesson_error(
                    "lesson procedure keys/names must be unique and may not overwrite existing knowledge",
                ));
            }
            lesson_dependencies.insert(
                format!("lesson:{}", procedure_draft.key),
                TeacherDependency {
                    alias: format!("lesson:{}", procedure_draft.key),
                    procedure: spoon_core::ProcedureId(deterministic_lesson_uuid(
                        &canonical,
                        "procedure",
                        &procedure_draft.key,
                    )),
                    version: 1,
                },
            );
        }
        let mut dependencies = external_dependencies.clone();
        dependencies.extend(lesson_dependencies.clone());

        let mut procedures = Vec::with_capacity(draft.procedures.len());
        let mut procedures_by_key = HashMap::new();
        for procedure_draft in &draft.procedures {
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
                    value_type: Some(parameter_draft.value_type),
                });
            }
            let (body, used_parameters, mut used_dependencies) = compile_lesson_body(
                &draft.primitive_set,
                &procedure_draft.body,
                &parameter_names,
                false,
                &dependencies,
            )?;
            if parameter_names != used_parameters {
                return Err(lesson_error(
                    "a reusable procedure body must use every declared parameter",
                ));
            }
            let (contract, contract_dependencies) = compile_lesson_contract(
                &draft.primitive_set,
                &procedure_draft.contract,
                &parameter_names,
                &dependencies,
            )?;
            if contract
                .requires
                .iter()
                .chain(&contract.promises)
                .chain(&contract.fails_when)
                .filter_map(|condition| condition.check.as_ref())
                .any(|check| {
                    Procedure::new("contract-check", Vec::new(), check.clone()).is_effectful()
                })
            {
                return Err(lesson_error(
                    "capability calls are allowed in procedure bodies, not contract checks",
                ));
            }
            used_dependencies.extend(contract_dependencies);
            let used_external_dependencies = used_dependencies
                .iter()
                .filter(|alias| external_dependencies.contains_key(*alias))
                .cloned()
                .collect::<HashSet<_>>();
            self.validate_used_teacher_dependencies(
                &external_dependencies,
                &used_external_dependencies,
            )?;
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
            procedure.id = lesson_dependencies
                .get(&format!("lesson:{}", procedure_draft.key))
                .expect("lesson procedure aliases were built above")
                .procedure;
            self.validate_capability_dependencies(&procedure)?;
            procedure.lifecycle = Lifecycle::Provisional;
            procedures_by_key.insert(procedure_draft.key.clone(), procedure.clone());
            procedures.push(procedure);
        }
        if !lesson_procedures_are_acyclic(&procedures) {
            return Err(lesson_error(
                "lesson procedures may not form a dependency cycle",
            ));
        }
        let procedure = procedures_by_key
            .get(&draft.invocation.procedure_key)
            .cloned()
            .ok_or_else(|| lesson_error("invocation must target a proposed procedure"))?;
        let mut invocation_inputs = BTreeMap::new();
        for input in &draft.invocation.inputs {
            if draft.primitive_set == "pure_rpn_v1" && !lesson_scalar(&input.value) {
                return Err(lesson_error(
                    "pure_rpn_v1 lesson inputs must be scalar values",
                ));
            }
            if draft.primitive_set == "pure_expr_v2" {
                validate_lesson_value(&input.value, 0)?;
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
        let attached = concepts
            .iter()
            .find(|concept| Some(concept.id) == procedure.concept)
            .cloned()
            .ok_or_else(|| lesson_error("invocation procedure must attach to a lesson concept"))?;
        let interpretation = ResolvedInterpretation {
            concept: attached,
            weight: 1.0,
            inputs: invocation_inputs.clone(),
        };
        Ok(CompiledLesson {
            idempotency_key,
            allows_structured_values: draft.primitive_set == "pure_expr_v2",
            concepts,
            relationships,
            procedures,
            invocation_procedure: procedure,
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

    fn pure_teacher_dependencies(&self) -> Result<Vec<TeacherDependency>, EngineError> {
        let mut procedures = self
            .graph
            .list_procedures()?
            .into_iter()
            .filter(|procedure| {
                matches!(
                    procedure.lifecycle,
                    Lifecycle::Active | Lifecycle::Validated
                )
            })
            .collect::<Vec<_>>();
        procedures.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.0.cmp(&right.id.0))
        });

        let mut dependencies = Vec::new();
        for procedure in procedures {
            let mut visiting = HashSet::new();
            if !self.procedure_is_closed_pure(&procedure, &mut visiting)? {
                continue;
            }
            let alias = format!("pure_dependency_{}", dependencies.len());
            dependencies.push(TeacherDependency {
                alias,
                procedure: procedure.id,
                version: procedure.version,
            });
            if dependencies.len() == MAX_LESSON_DEPENDENCIES {
                break;
            }
        }
        Ok(dependencies)
    }

    fn validate_used_teacher_dependencies(
        &self,
        dependencies: &HashMap<String, TeacherDependency>,
        used: &HashSet<String>,
    ) -> Result<(), EngineError> {
        for alias in used {
            let dependency = dependencies
                .get(alias)
                .ok_or_else(|| lesson_error("lesson referenced an unadvertised procedure alias"))?;
            let current = self
                .graph
                .get_procedure(dependency.procedure)?
                .ok_or_else(|| lesson_error("advertised procedure dependency is absent"))?;
            if current.version != dependency.version {
                return Err(lesson_error(
                    "advertised procedure dependency changed revision before lesson admission",
                ));
            }
            let mut visiting = HashSet::new();
            if !matches!(current.lifecycle, Lifecycle::Active | Lifecycle::Validated)
                || !self.procedure_is_closed_pure(&current, &mut visiting)?
            {
                return Err(lesson_error(
                    "advertised procedure dependency is no longer a pure executable procedure",
                ));
            }
        }
        Ok(())
    }

    fn procedure_is_closed_pure(
        &self,
        procedure: &Procedure,
        visiting: &mut HashSet<(ProcedureId, u32)>,
    ) -> Result<bool, EngineError> {
        if !visiting.insert((procedure.id, procedure.version)) {
            return Ok(false);
        }
        let checks = procedure
            .contract
            .requires
            .iter()
            .chain(&procedure.contract.promises)
            .chain(&procedure.contract.fails_when)
            .filter_map(|condition| condition.check.as_ref());
        let pure = self.expression_is_closed_pure(&procedure.body, visiting)?
            && checks
                .map(|check| self.expression_is_closed_pure(check, visiting))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .all(|item| item);
        visiting.remove(&(procedure.id, procedure.version));
        Ok(pure)
    }

    fn expression_is_closed_pure(
        &self,
        expression: &Expr,
        visiting: &mut HashSet<(ProcedureId, u32)>,
    ) -> Result<bool, EngineError> {
        match expression {
            Expr::Literal(_) | Expr::Var(_) => Ok(true),
            Expr::BinOp { left, right, .. } => Ok(self
                .expression_is_closed_pure(left, visiting)?
                && self.expression_is_closed_pure(right, visiting)?),
            Expr::UnOp { operand, .. } => self.expression_is_closed_pure(operand, visiting),
            // Old unconstrained calls have no exact provenance pin and are
            // therefore never legal dependency candidates.
            Expr::Call { .. } => Ok(false),
            Expr::CapabilityCall { .. } => Ok(false),
            Expr::CallExact {
                procedure,
                version,
                args,
            } => {
                if !args
                    .iter()
                    .map(|argument| self.expression_is_closed_pure(argument, visiting))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .all(|item| item)
                {
                    return Ok(false);
                }
                let Some(exact) = self.graph.get_procedure_version(*procedure, *version)? else {
                    return Ok(false);
                };
                self.procedure_is_closed_pure(&exact, visiting)
            }
            Expr::If { cond, then, else_ } => Ok(self.expression_is_closed_pure(cond, visiting)?
                && self.expression_is_closed_pure(then, visiting)?
                && self.expression_is_closed_pure(else_, visiting)?),
            Expr::Let { value, body, .. } => Ok(self.expression_is_closed_pure(value, visiting)?
                && self.expression_is_closed_pure(body, visiting)?),
            Expr::Block(expressions) | Expr::ListExpr(expressions) => expressions
                .iter()
                .map(|item| self.expression_is_closed_pure(item, visiting))
                .collect::<Result<Vec<_>, _>>()
                .map(|items| items.into_iter().all(|item| item)),
            Expr::Index { collection, index } => Ok(self
                .expression_is_closed_pure(collection, visiting)?
                && self.expression_is_closed_pure(index, visiting)?),
            Expr::FieldAccess { object, .. } => self.expression_is_closed_pure(object, visiting),
            Expr::Map {
                collection, body, ..
            } => Ok(self.expression_is_closed_pure(collection, visiting)?
                && self.expression_is_closed_pure(body, visiting)?),
            Expr::Filter {
                collection,
                predicate,
                ..
            } => Ok(self.expression_is_closed_pure(collection, visiting)?
                && self.expression_is_closed_pure(predicate, visiting)?),
            Expr::Reduce {
                collection,
                init,
                body,
                ..
            } => Ok(self.expression_is_closed_pure(collection, visiting)?
                && self.expression_is_closed_pure(init, visiting)?
                && self.expression_is_closed_pure(body, visiting)?),
            Expr::Intrinsic { args, .. } => args
                .iter()
                .map(|argument| self.expression_is_closed_pure(argument, visiting))
                .collect::<Result<Vec<_>, _>>()
                .map(|items| items.into_iter().all(|item| item)),
        }
    }

    fn teacher_request(
        &self,
        input: &CycleInput,
        interpretations: &[ResolvedInterpretation],
        dependencies: &[TeacherDependency],
    ) -> Result<TeacherRequestWire, EngineError> {
        let item_limit = input
            .budget
            .max_context_items
            .min(MAX_TEACHER_CONTEXT_ITEMS);
        let session_id = input
            .session_id
            .as_deref()
            .map(|value| Uuid::parse_str(value).map(SessionId))
            .transpose()
            .map_err(|_| EngineError::InvalidInput("session_id is not a UUID".into()))?;
        let recall_mode = match input.recall_mode {
            RecallMode::Global => EpisodeRecallMode::Global,
            RecallMode::Session => EpisodeRecallMode::Session,
            RecallMode::None => EpisodeRecallMode::None,
        };
        let recent_episodes =
            self.episodes
                .list_recent_for_recall(session_id, recall_mode, item_limit as u32)?;
        let correction_target = is_explicit_incorrectness_feedback(&input.situation)
            .then(|| recent_episodes.first())
            .flatten()
            .map(teacher_episode_context);
        let recent_episodes = recent_episodes
            .into_iter()
            .map(|episode| teacher_episode_context(&episode))
            .collect::<Vec<_>>();
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
        let mut capability_procedures = Vec::new();
        for imported in self
            .capabilities
            .list_imported()
            .map_err(|error| EngineError::InvalidInput(format!("capability registry: {error}")))?
            .into_iter()
            .filter(|capability| capability.locally_validated)
        {
            let Ok(bundle) = self.capabilities.reconstruct(&imported.content_id) else {
                continue;
            };
            for procedure in bundle.procedures.into_iter().take(item_limit) {
                capability_procedures.push(json!({
                    "contentId": imported.content_id.clone(),
                    "procedureId": procedure.id,
                    "name": truncate_text(&procedure.name, MAX_TEACHER_TEXT_CHARS),
                    "primitive": procedure.primitive,
                    "inputSchema": procedure.input_schema,
                    "outputSchema": procedure.output_schema,
                    "effects": procedure.effects,
                    "permissions": procedure.permissions,
                }));
                if capability_procedures.len() >= item_limit {
                    break;
                }
            }
            if capability_procedures.len() >= item_limit {
                break;
            }
        }
        // Native boundaries are always authorable.  They deliberately appear
        // even when this process has no adapter configured yet: configuration
        // and consent are runtime questions, never a reason to suppress a
        // procedure from the language the teacher may express.
        for native in native_teacher_capability_procedures() {
            if capability_procedures.len() >= item_limit {
                break;
            }
            capability_procedures.push(native);
        }
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
            "capabilityProcedures": capability_procedures,
            "pureProcedureDependencies": dependencies.iter().map(|dependency| {
                let procedure = self.graph.get_procedure_version(dependency.procedure, dependency.version)
                    .ok().flatten();
                json!({
                    "alias": dependency.alias,
                    "name": procedure.as_ref().map(|item| truncate_text(&item.name, MAX_TEACHER_TEXT_CHARS)).unwrap_or_else(|| "unavailable".into()),
                    "parameters": procedure.as_ref().map(|item| item.params.iter().take(item_limit).map(|parameter| json!({
                        "name": truncate_text(&parameter.name, MAX_TEACHER_TEXT_CHARS),
                        "description": parameter.description.as_deref().map(|description| truncate_text(description, MAX_TEACHER_TEXT_CHARS)),
                    })).collect::<Vec<_>>()).unwrap_or_default(),
                })
            }).collect::<Vec<_>>(),
            "environment": environment,
            "assumptions": assumptions,
            "recentEpisodes": recent_episodes,
            "correctionTarget": correction_target,
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
        let correction_guidance = if is_explicit_incorrectness_feedback(&input.situation) {
            " The user is reporting a prior answer as incorrect. correctionTarget is the immediately preceding episode to inspect; use it rather than a merely similar older episode in recentEpisodes. Correct that target. Do not replay its prior procedure merely because it ran successfully; only propose reusable knowledge if it actually matches the target request."
        } else {
            ""
        };
        Ok(TeacherRequestWire {
            situation: input.situation.clone(),
            context,
            specific_question: Some(format!(
                "Write spoonlang for THIS situation in JSON field source. Do not copy prompt examples. For a stable general fact with no user-supplied value to transform (how many eyes, spelling a word), use kind answer_only. For deterministic generalizable tasks over inputs, return kind reusable_lesson. Programmatic work on user-supplied data is deterministic: field/index/path extraction, JSON parsing, text or collection transforms, counting, sorting, and arithmetic should teach a reusable procedure. For effectful work, use cap(\"<contentId>\", \"<procedureId>\", input) only with advertised capabilityProcedures. Native capabilities are authorable even if an adapter is not currently configured. For a field or indexed path, teach path_get or path_get_optional; use json_parse only when the input is JSON text. Use dep(\"lesson:<procedure-key>\", ...) only for acyclic composition. For external observations without a trusted primitive, return kind external_observation. Otherwise answer or abstain.{correction_guidance}"
            )),
            desired_output: proposal_schema(),
        })
    }

    /// Candidate Laboratory (P0F.4): try to compose known procedures
    /// into a sequential chain and execute it in quarantine.
    fn attempt_compose_and_execute(
        &mut self,
        cycle_id: CycleId,
        input: &CycleInput,
        interpretations: &[ResolvedInterpretation],
    ) -> Result<Option<CycleProgress>, EngineError> {
        let procedures: Vec<Procedure> = self
            .graph
            .list_procedures()?
            .into_iter()
            .filter(|p| is_current_executable(p.lifecycle))
            .collect();

        let literals = extract_literals(&input.situation);

        let candidate = match crate::compose::attempt_composition(
            &input.situation,
            &procedures,
            &literals,
        ) {
            Some(candidate) => candidate,
            None => return Ok(None),
        };

        let composed = candidate.procedure.clone();
        let mut evaluator = self
            .current_evaluator()?
            .with_budget(input.budget.max_exec_steps.min(self.max_steps));
        evaluator.register_procedure(composed.clone());
        let mut capability_runtime = crate::engine::EngineCapabilityInvoker {
            engine: self,
            permission_mode: input.permission_mode.clone(),
        };
        let mut evaluator = evaluator.with_capability_invoker(&mut capability_runtime);

        let args: Vec<Value> = composed
            .params
            .iter()
            .enumerate()
            .map(|(i, _param)| {
                candidate
                    .inputs
                    .get(i)
                    .cloned()
                    .unwrap_or(Value::Null)
            })
            .collect();

        let attempt = evaluator.exec_procedure_captured(&composed.id, args);
        let steps_used = evaluator.budget().steps_used;
        drop(evaluator);
        drop(capability_runtime);

        match &attempt.result {
            Ok(value) => {
                // Composition succeeded - persist the new procedure and record episode
                self.graph.insert_procedure(&composed)?;
                self.index_procedure(&composed)?;

                let chain_names: Vec<&str> = candidate.chain.iter().map(|c| c.name.as_str()).collect();
                let mut episode = self.base_episode(input, interpretations)?;
                episode.action = Some(format!(
                    "compose:{}@{}",
                    composed.id, composed.version,
                ));
                episode.reasoning_trace = reasoning_trace(&attempt.trace);
                let mut prefix = ladder_prefix(EscalationRung::Compose, false);
                prefix.push(simple_step(
                    &format!(
                        "composed {} from known procedures",
                        chain_names.join(" then "),
                    ),
                    EscalationRung::Compose,
                ));
                prefix.append(&mut episode.reasoning_trace.steps);
                episode.reasoning_trace.steps = prefix;
                episode.execution_trace = Some(serde_json::to_value(&attempt.trace)?);
                episode.teacher_interaction = Some(json!({
                    "selectionReason": {
                        "path": "composition",
                        "summary": format!(
                            "Composed new procedure by chaining: {}",
                            chain_names.join(" -> ")
                        ),
                        "chain": chain_names,
                        "composedProcedure": composed.id.to_string(),
                        "inputs": candidate.inputs,
                    }
                }));
                episode.observed_result = Some(value.clone());
                episode.evaluation = Some(Evaluation {
                    tier: VerifiabilityTier::Deferred,
                    success: true,
                    details: "composed procedure executes; semantic fit is provisional".into(),
                    surprise: None,
                });
                episode.cost = EpisodeCost {
                    rung_reached: EscalationRung::Compose,
                    steps_taken: attempt.trace.len() as u32,
                    budget_spent: f64::from(steps_used),
                };
                self.persist_engine_episode(&episode)?;

                Ok(Some(CycleProgress::Completed(Box::new(CycleOutcome {
                    cycle_id,
                    disposition: CycleDisposition::Provisional,
                    answer: Some(value.clone()),
                    procedure_ir: Some(serde_json::to_value(&composed)?),
                    episode,
                }))))
            }
            Err(_) => {
                // Composition execution failed - fall through to interpreter/teacher
                Ok(None)
            }
        }
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
        // Installing knowledge and exercising an effect are different phases.
        // A teacher's worked example may require a real network/file/etc.
        // operation, but its inability to run *now* must never decide whether
        // the procedure is admissible.  Runtime authorization happens only
        // when a later request actually reaches the capability call.
        if let Some(KnowledgeToLearn::Lesson(lesson)) = &knowledge_to_learn
            && procedure.is_effectful()
        {
            return self.install_effectful_lesson_without_execution(
                cycle_id,
                input,
                procedure,
                rung,
                teacher_interaction,
                lesson,
                expected_answer,
            );
        }
        let (prior_reasoning, prior_execution_trace, prior_steps_used, prior_trace_len) =
            prior_failure_material(teacher_interaction.as_ref());
        let args = bind_inputs(procedure, &inputs, None)?;
        let mut evaluator = match &knowledge_to_learn {
            Some(KnowledgeToLearn::Lesson(_)) => self.current_evaluator()?,
            _ => self.evaluator_for_procedure(procedure)?,
        }
        .with_budget(input.budget.max_exec_steps.min(self.max_steps));
        if let Some(KnowledgeToLearn::Lesson(lesson)) = &knowledge_to_learn {
            for lesson_procedure in &lesson.procedures {
                evaluator.register_procedure(lesson_procedure.clone());
            }
        }
        evaluator.register_procedure(procedure.clone());
        let mut capability_runtime = crate::engine::EngineCapabilityInvoker {
            engine: self,
            permission_mode: input.permission_mode.clone(),
        };
        let mut evaluator = evaluator.with_capability_invoker(&mut capability_runtime);
        let attempt = evaluator.exec_procedure_captured(&procedure.id, args);
        let steps_used = evaluator.budget().steps_used;
        drop(evaluator);
        drop(capability_runtime);
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
            steps.push(simple_step(
                "ask: escalated to teacher for guidance",
                EscalationRung::Ask,
            ));
            steps
        };
        for step in &mut prefix {
            if step.rung == rung && step.procedure_used.is_none() {
                step.procedure_used = Some(procedure.id);
                if step.rung == EscalationRung::Run
                    && step.description.contains("uniquely matched")
                {
                    step.description = format!(
                        "run: uniquely matched procedure {}@{}, executing",
                        procedure.id, procedure.version
                    );
                }
            }
        }
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
                        procedure_ir: Some(serde_json::to_value(procedure)?),
                        episode,
                    })));
                }
                let semantic_verified = rung == EscalationRung::Run
                    && knowledge_to_learn.is_none()
                    && episode.context.held_contradictions.is_empty()
                    && episode.context.unresolved_refinements.is_empty()
                    && matches!(
                        procedure.lifecycle,
                        spoon_core::Lifecycle::Active | spoon_core::Lifecycle::Validated
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
                            b"spoon:teacher-lesson:request-binding:v1",
                            &binding_bytes,
                        ))
                    );
                    let stage_identity =
                        serde_json::to_vec(&(&lesson.idempotency_key, &request_binding_digest))?;
                    let stage_id = format!(
                        "lesson-stage:{}",
                        hex_bytes(&lesson_sha256(
                            b"spoon:teacher-lesson:stage:v1",
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
                        procedure_ir: Some(serde_json::to_value(procedure)?),
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
                    procedure_ir: Some(serde_json::to_value(procedure)?),
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
                    let dependency_allowlist = self.pure_teacher_dependencies()?;
                    let request = self.teacher_request(
                        &pending_input,
                        interpretations,
                        &dependency_allowlist,
                    )?;
                    pending_input.budget.max_teacher_turns =
                        pending_input.budget.max_teacher_turns.saturating_sub(1);
                    let pending = PendingCycle {
                        input: pending_input,
                        request: request.clone(),
                        dependency_allowlist,
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
                    procedure_ir: Some(serde_json::to_value(procedure)?),
                    episode,
                })))
            }
        }
    }

    /// Admit an effectful teacher lesson without invoking its selected sample.
    /// This intentionally performs no permission, adapter, host, or network
    /// check: all of those are execution-time concerns.
    #[allow(clippy::too_many_arguments)]
    fn install_effectful_lesson_without_execution(
        &mut self,
        cycle_id: CycleId,
        input: &CycleInput,
        procedure: &Procedure,
        rung: EscalationRung,
        teacher_interaction: Option<JsonValue>,
        lesson: &CompiledLesson,
        expected_answer: Option<Value>,
    ) -> Result<CycleProgress, EngineError> {
        let mut episode = self.base_episode(input, &[])?;
        episode.action = Some("teacher-procedure-installed:awaiting-runtime-authorization".into());
        episode.prediction = expected_answer;
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Deferred,
            success: true,
            details: "effectful procedure was installed without executing its worked example; capability authorization is deferred to runtime".into(),
            surprise: None,
        });
        episode.teacher_interaction = teacher_interaction;
        episode.reasoning_trace.steps = ladder_prefix(rung, rung == EscalationRung::Ask);
        episode.reasoning_trace.steps.push(simple_step(
            "install effectful teacher procedure without execution",
            rung,
        ));
        episode.cost = EpisodeCost {
            rung_reached: rung,
            steps_taken: 0,
            budget_spent: if rung == EscalationRung::Ask {
                1.0
            } else {
                0.0
            },
        };
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
                b"spoon:teacher-lesson:request-binding:v1",
                &binding_bytes,
            ))
        );
        let stage_identity =
            serde_json::to_vec(&(&lesson.idempotency_key, &request_binding_digest))?;
        let stage = DurableLessonStage {
            stage_id: format!(
                "lesson-stage:{}",
                hex_bytes(&lesson_sha256(
                    b"spoon:teacher-lesson:stage:v1",
                    &stage_identity,
                ))
            ),
            bundle_key: lesson.idempotency_key.clone(),
            request_binding_digest,
            concepts: lesson.concepts.clone(),
            relationships: lesson.relationships.clone(),
            procedures: lesson.procedures.clone(),
            episode: episode.clone(),
        };
        self.lesson_stages.stage(&stage)?;
        let integration = (|| {
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
            Ok::<(), EngineError>(())
        })();
        if let Err(error) = integration {
            self.lesson_stages.discard(&stage)?;
            return Err(error);
        }
        self.persist_engine_episode(&episode)?;
        self.lesson_stages.complete(&stage)?;
        Ok(CycleProgress::Completed(Box::new(CycleOutcome {
            cycle_id,
            disposition: CycleDisposition::Provisional,
            // The teacher's example is retained as a prediction in the
            // episode, not asserted as the live result of an unrun effect.
            answer: None,
            procedure_ir: Some(serde_json::to_value(procedure)?),
            episode,
        })))
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
            procedure_ir: None,
            episode,
        })))
    }

    fn base_episode(
        &self,
        input: &CycleInput,
        interpretations: &[ResolvedInterpretation],
    ) -> Result<Episode, EngineError> {
        let mut episode = Episode::new(&input.situation);
        episode.working_directory = input.working_directory.clone();
        let session_id = input
            .session_id
            .as_deref()
            .map(|value| Uuid::parse_str(value).map(SessionId))
            .transpose()
            .map_err(|_| EngineError::InvalidInput("session_id is not a UUID".into()))?;
        if let Some(session_id) = session_id {
            let session = self
                .episodes
                .get_session(&session_id.to_string())?
                .ok_or_else(|| EngineError::InvalidInput("session does not exist".into()))?;
            if session.state != spoon_core::SessionState::Active {
                return Err(EngineError::InvalidInput("session has ended".into()));
            }
            episode.session_id = Some(session_id);
            episode.session_visibility = session.visibility;
            episode.turn_index = Some(self.episodes.next_turn_index(session_id)?);
        }
        let recall_mode = match input.recall_mode {
            RecallMode::Global => EpisodeRecallMode::Global,
            RecallMode::Session => EpisodeRecallMode::Session,
            RecallMode::None => EpisodeRecallMode::None,
        };
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
            let limits = spoon_reason::ContextLimits {
                max_entities: input.budget.max_context_items,
                max_relationships: input.budget.max_context_items,
                max_recent_episodes: input.budget.max_context_items,
                ..spoon_reason::ContextLimits::default()
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
                .assemble_for_recall(
                    &ContextRequest {
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
                    },
                    session_id,
                    recall_mode,
                )
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
            episode.context.budget_remaining = Some(spoon_core::ContextBudget {
                steps: input.budget.max_exec_steps,
                teacher_calls: input.budget.max_teacher_turns,
                cost: f64::from(input.budget.max_exec_steps),
            });
            let limits = ContextConfig::default().limits;
            episode.context.recent_episodes = self
                .episodes
                .list_recent_for_recall(session_id, recall_mode, limits.max_recent_episodes as u32)?
                .into_iter()
                .map(|recent| spoon_core::ContextEpisode {
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
            if let spoon_adapt::Uncertainty::HeldContradictions(inherited) =
                self.uncertainty_for_claim(&predicate)?
            {
                held.extend(inherited.into_iter().map(|id| id.0));
            }
            let refinement =
                self.refinement_context_for_predicate(&predicate, &input.environment)?;
            applied_refinements.extend(refinement.applied.into_iter().map(|applied| {
                spoon_core::ContextRefinement {
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

fn intent_output_schema(
    bindings: &[IntentCandidateBinding],
    token_count: usize,
    literal_candidates: &[JsonValue],
) -> JsonValue {
    let max_token_start = token_count.saturating_sub(1);
    let candidate_schemas = bindings
        .iter()
        .map(|binding| {
            let slot_schemas = binding
                .procedure
                .params
                .iter()
                .map(|param| {
                    let permitted_literal_ranges =
                        literal_ranges_for_slot(&param.name, literal_candidates);
                    json!({
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["name", "confidence", "sourceTokens"],
                        "properties": {
                            "name": { "type": "string", "enum": [param.name] },
                            "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
                            "sourceTokens": {
                                "type": "array", "maxItems": 1,
                                "items": { "type": "object", "enum": permitted_literal_ranges },
                            },
                            "inferredValue": {
                                "type": ["null", "boolean", "number", "string"],
                            },
                        },
                    })
                })
                .collect::<Vec<_>>();
            let slot_items = if slot_schemas.is_empty() {
                json!(false)
            } else {
                json!({ "oneOf": slot_schemas })
            };
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["name", "confidence", "scope", "sourceTokens", "slots", "ambiguities"],
                "properties": {
                    "name": { "type": "string", "enum": [binding.alias] },
                    "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
                    "scope": { "enum": ["CurrentTurn", "Conversation", "Workspace", "External"] },
                    "sourceTokens": {
                        "type": "array", "minItems": 1, "maxItems": 1,
                        "items": token_range_schema(max_token_start, token_count),
                    },
                    "slots": {
                        "type": "array",
                        "minItems": binding.procedure.params.len(),
                        "maxItems": binding.procedure.params.len(),
                        "uniqueItems": true,
                        "items": slot_items,
                    },
                    "ambiguities": {
                        "type": "array",
                        "maxItems": 4,
                        "items": { "type": "string", "maxLength": 256 },
                    },
                },
            })
        })
        .collect::<Vec<_>>();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["candidates", "selected", "disposition"],
        "properties": {
            "candidates": {
                "type": "array",
                // The interpreter's only executable outcome is one selected
                // procedure. Letting a small constrained model fill sixteen
                // alternative frames makes `selected` needlessly fragile and
                // does not add execution capability.
                "maxItems": 1,
                "items": { "oneOf": candidate_schemas },
            },
            // `selected` indexes the emitted frame array, not the catalog.
            // Since that array has at most one item, zero is the only valid
            // executable selection regardless of the chosen alias.
            "selected": { "type": ["integer", "null"], "minimum": 0, "maximum": 0 },
            "disposition": { "enum": ["execute", "clarify", "abstain"] },
        },
    })
}

fn intent_literal_ranges(token_stream: &TokenStream) -> Vec<TokenRange> {
    let mut ranges = token_stream
        .tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| matches!(token.kind, TokenKind::Word | TokenKind::Number))
        .map(|(index, _)| TokenRange::new(index, index + 1))
        .collect::<Vec<_>>();

    let mut opening_quote = None;
    for (index, token) in token_stream.tokens.iter().enumerate() {
        let Some(quote) = token_stream
            .slice(&token.span)
            .filter(|text| *text == "\"" || *text == "'")
        else {
            continue;
        };
        match opening_quote {
            Some((opening, start)) if opening == quote => {
                ranges.push(TokenRange::new(start, index + 1));
                opening_quote = None;
            }
            _ => opening_quote = Some((quote, index)),
        }
    }
    ranges.sort_by_key(|range| (range.start_token, range.end_token));
    ranges.dedup_by_key(|range| (range.start_token, range.end_token));
    ranges
}

fn literal_ranges_for_slot(slot_name: &str, candidates: &[JsonValue]) -> Vec<JsonValue> {
    let slot = slot_name.to_ascii_lowercase();
    let mut ranked = candidates
        .iter()
        .filter_map(|candidate| {
            let range = candidate.get("tokenRange")?.clone();
            let text = candidate.get("text")?.as_str().unwrap_or_default();
            let trimmed = text.trim_matches(['\'', '"']);
            let start = range
                .get("startToken")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            let quoted = text.len() >= 2
                && ((text.starts_with('"') && text.ends_with('"'))
                    || (text.starts_with('\'') && text.ends_with('\'')));
            let scalar_count = trimmed.chars().count();
            let is_number = candidate
                .get("value")
                .is_some_and(|value| value.is_number());
            let score = if matches!(
                slot.as_str(),
                "target" | "needle" | "character" | "letter" | "substring" | "pattern"
            ) {
                i64::from(quoted) * 1_000 + i64::from(scalar_count == 1) * 800
                    - scalar_count.min(256) as i64
            } else if matches!(
                slot.as_str(),
                "text" | "source" | "haystack" | "document" | "content"
            ) {
                i64::from(quoted && scalar_count > 1) * 1_000
                    + scalar_count.min(256) as i64 * 10
                    + start.min(i64::MAX as u64) as i64
            } else if matches!(
                slot.as_str(),
                "x" | "n" | "number" | "count" | "index" | "amount" | "a" | "b"
            ) {
                i64::from(is_number) * 1_000 + start.min(i64::MAX as u64) as i64
            } else {
                start.min(i64::MAX as u64) as i64
            };
            Some((score, start, range))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    ranked.into_iter().map(|(_, _, range)| range).collect()
}

fn token_range_text<'a>(token_stream: &'a TokenStream, range: TokenRange) -> Option<&'a str> {
    let start = token_stream.tokens.get(range.start_token)?.span.start_byte;
    let end = token_stream
        .tokens
        .get(range.end_token.checked_sub(1)?)?
        .span
        .end_byte;
    token_stream.document.text.get(start..end)
}

fn intent_literal_value(text: &str) -> Value {
    match extract_literals(text).as_slice() {
        [value] => value.clone(),
        _ => Value::Text(text.trim().to_owned()),
    }
}

fn episode_action_kind(action: &str) -> &str {
    if action.starts_with("procedure:") {
        "procedure"
    } else if action.starts_with("abstain:") {
        "abstain"
    } else if action.starts_with("failed:") {
        "failed"
    } else {
        "other"
    }
}

fn execution_inputs(trace: &JsonValue) -> Option<Vec<JsonValue>> {
    trace
        .get("steps")
        .and_then(JsonValue::as_array)
        .and_then(|steps| {
            steps.iter().rev().find_map(|step| {
                if step.get("procedure_used").is_none() && step.get("procedure_called").is_none() {
                    return None;
                }
                step.get("input")?.as_array().cloned()
            })
        })
}

fn is_reconsideration_situation(situation: &str) -> bool {
    let words = language_words(situation);
    words.contains("sure")
        || words.contains("recheck")
        || words.contains("again")
        || (words
            .iter()
            .any(|word| matches!(word.as_str(), "wrong" | "incorrect" | "mistake" | "correct"))
            && words.iter().any(|word| {
                matches!(
                    word.as_str(),
                    "last" | "previous" | "earlier" | "before" | "answer" | "result"
                )
            }))
}

/// Explicit authoring requests may mention capability terms as the subject of
/// the procedure being taught. They must reach the Teacher rather than being
/// answered by the deterministic self-capability introspection shortcut.
fn is_explicit_teaching_request(situation: &str) -> bool {
    situation
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("the user explicitly requested that spoon teach")
}

fn is_explicit_incorrectness_feedback(situation: &str) -> bool {
    let words = language_words(situation);
    words.iter().any(|word| {
        matches!(
            word.as_str(),
            "wrong" | "incorrect" | "mistake" | "mistaken"
        )
    }) || situation.to_ascii_lowercase().contains("not correct")
}

fn teacher_episode_context(episode: &Episode) -> JsonValue {
    json!({
        "situation": truncate_text(&episode.situation, MAX_TEACHER_TEXT_CHARS),
        "action": episode.action.as_deref().map(|action| truncate_text(action, MAX_TEACHER_TEXT_CHARS)),
        "answer": episode.observed_result.as_ref().or(episode.prediction.as_ref()),
        "succeeded": episode.evaluation.as_ref().map(|evaluation| evaluation.success),
    })
}

fn rejected_intent_interaction(
    request: &IntentRequestWire,
    proposal: &IntentProposalWire,
    reason: impl Into<String>,
) -> JsonValue {
    json!({
        "languageInterpreter": {
            "request": request,
            "source": proposal.source,
            "status": proposal.status,
            "provenance": proposal.provenance,
            "rejection": truncate_text(&reason.into(), MAX_TEACHER_TEXT_CHARS),
            // Preserve the wire response, including rawContent when supplied,
            // so a malformed local-model response remains inspectable.
            "rejectedProposal": proposal,
        }
    })
}

fn procedure_has_language_support(situation: &str, binding: &IntentCandidateBinding) -> bool {
    let query_terms = meaningful_language_words(situation);
    if query_terms.is_empty() {
        return false;
    }
    let procedure_name_terms = meaningful_language_words(&binding.procedure.name);
    let concept_name_terms = meaningful_language_words(&binding.concept.name);
    if query_terms
        .iter()
        .any(|term| procedure_name_terms.contains(term) || concept_name_terms.contains(term))
    {
        return true;
    }
    let action_hints = procedure_language_hints(&binding.procedure.body);
    if query_terms.iter().any(|term| action_hints.contains(term)) {
        return true;
    }
    let mut candidate_text = format!(
        "{} {} {} {:?}",
        binding.concept.name,
        routing_description(binding.concept.description.as_deref()),
        binding.procedure.name,
        binding.procedure.body
    );
    for parameter in &binding.procedure.params {
        candidate_text.push(' ');
        candidate_text.push_str(&parameter.name);
        if let Some(description) = &parameter.description {
            candidate_text.push(' ');
            candidate_text.push_str(description);
        }
    }
    let candidate_terms = meaningful_language_words(&candidate_text);
    query_terms.intersection(&candidate_terms).next().is_some()
}

fn meaningful_language_words(value: &str) -> BTreeSet<String> {
    language_words(value)
        .into_iter()
        .filter(|word| {
            !matches!(
                word.as_str(),
                "a" | "an"
                    | "and"
                    | "are"
                    | "as"
                    | "at"
                    | "be"
                    | "by"
                    | "can"
                    | "do"
                    | "does"
                    | "for"
                    | "from"
                    | "get"
                    | "how"
                    | "i"
                    | "in"
                    | "into"
                    | "is"
                    | "it"
                    | "me"
                    | "my"
                    | "of"
                    | "on"
                    | "or"
                    | "please"
                    | "supplied"
                    | "that"
                    | "the"
                    | "their"
                    | "then"
                    | "this"
                    | "to"
                    | "was"
                    | "what"
                    | "with"
                    | "you"
                    | "your"
            )
        })
        .collect()
}

fn language_words(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase())
        .collect()
}

fn procedure_language_hints(expr: &Expr) -> BTreeSet<String> {
    fn visit(expr: &Expr, hints: &mut BTreeSet<String>) {
        match expr {
            Expr::Literal(_) | Expr::Var(_) => {}
            Expr::BinOp { op, left, right } => {
                if *op == BinOp::Mul {
                    hints.extend(["multiply", "times", "product"].map(str::to_owned));
                    if matches!(left.as_ref(), Expr::Literal(Value::Int(2)))
                        || matches!(right.as_ref(), Expr::Literal(Value::Int(2)))
                    {
                        hints.extend(["double", "twice"].map(str::to_owned));
                    }
                }
                visit(left, hints);
                visit(right, hints);
            }
            Expr::UnOp { operand, .. } => visit(operand, hints),
            Expr::CapabilityCall { input, .. } => {
                hints.extend(["capability", "external", "fetch"].map(str::to_owned));
                visit(input, hints);
            }
            Expr::Call { args, .. } | Expr::CallExact { args, .. } => {
                for arg in args {
                    visit(arg, hints);
                }
            }
            Expr::If { cond, then, else_ } => {
                visit(cond, hints);
                visit(then, hints);
                visit(else_, hints);
            }
            Expr::Let { value, body, .. } => {
                visit(value, hints);
                visit(body, hints);
            }
            Expr::Block(items) | Expr::ListExpr(items) => {
                for item in items {
                    visit(item, hints);
                }
            }
            Expr::Index { collection, index } => {
                hints.extend(["index", "element", "position"].map(str::to_owned));
                visit(collection, hints);
                visit(index, hints);
            }
            Expr::FieldAccess { object, .. } => {
                hints.extend(["field", "property", "attribute"].map(str::to_owned));
                visit(object, hints);
            }
            Expr::Map {
                collection, body, ..
            } => {
                visit(collection, hints);
                visit(body, hints);
            }
            Expr::Filter {
                collection,
                predicate,
                ..
            } => {
                visit(collection, hints);
                visit(predicate, hints);
            }
            Expr::Reduce {
                collection,
                init,
                body,
                ..
            } => {
                visit(collection, hints);
                visit(init, hints);
                visit(body, hints);
            }
            Expr::Intrinsic { op, args, .. } => {
                if *op == IntrinsicOp::TextCount {
                    hints.extend(
                        [
                            "count",
                            "occurrence",
                            "occurrences",
                            "often",
                            "many",
                            "frequency",
                        ]
                        .map(str::to_owned),
                    );
                }
                if matches!(op, IntrinsicOp::PathGet | IntrinsicOp::PathGetOptional) {
                    hints.extend(["path", "get", "lookup", "retrieve"].map(str::to_owned));
                }
                for arg in args {
                    visit(arg, hints);
                }
            }
        }
    }

    let mut hints = BTreeSet::new();
    visit(expr, &mut hints);
    hints
}

fn token_range_schema(max_start: usize, max_end: usize) -> JsonValue {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["startToken", "endToken"],
        "properties": {
            "startToken": { "type": "integer", "minimum": 0, "maximum": max_start },
            "endToken": { "type": "integer", "minimum": 1, "maximum": max_end },
        },
    })
}

fn action_procedure_id(action: &str) -> Option<spoon_core::ProcedureId> {
    let value = action.strip_prefix("procedure:")?.split('@').next()?;
    uuid::Uuid::parse_str(value)
        .ok()
        .map(spoon_core::ProcedureId)
}

fn compile_lesson_contract(
    primitive_set: &str,
    draft: &ContractDraft,
    parameters: &HashSet<String>,
    dependencies: &HashMap<String, TeacherDependency>,
) -> Result<(Contract, HashSet<String>), EngineError> {
    if draft.requires.len() > MAX_LESSON_CONDITIONS
        || draft.promises.len() > MAX_LESSON_CONDITIONS
        || draft.fails_when.len() > MAX_LESSON_CONDITIONS
    {
        return Err(lesson_error(
            "contract condition collection exceeds its bound",
        ));
    }
    let (requires, mut used_dependencies) = compile_lesson_conditions(
        primitive_set,
        &draft.requires,
        parameters,
        false,
        dependencies,
    )?;
    let (promises, promise_dependencies) = compile_lesson_conditions(
        primitive_set,
        &draft.promises,
        parameters,
        true,
        dependencies,
    )?;
    let (fails_when, failure_dependencies) = compile_lesson_conditions(
        primitive_set,
        &draft.fails_when,
        parameters,
        false,
        dependencies,
    )?;
    used_dependencies.extend(promise_dependencies);
    used_dependencies.extend(failure_dependencies);
    Ok((
        Contract {
            requires,
            promises,
            fails_when,
            ..Contract::default()
        },
        used_dependencies,
    ))
}

fn native_teacher_capability_procedures() -> Vec<JsonValue> {
    let content_id = crate::engine::NATIVE_CAPABILITY_CONTENT_ID;
    vec![
        json!({
            "contentId": content_id,
            "procedureId": "web.fetch",
            "name": "Fetch a web URL",
            "primitive": "network_request",
            "inputSchema": {"type": "object", "required": ["url"], "properties": {"url": {"type": "string"}}},
            "outputSchema": {"description": "host-provided bounded response"},
            "effects": ["network"],
            "permissions": ["runtime URL approval"],
            "authoring": "always available; URL consent and adapter checks happen at execution"
        }),
        json!({
            "contentId": content_id,
            "procedureId": "file.read",
            "name": "Read a scoped file",
            "primitive": "file_read",
            "inputSchema": {"type": "object", "required": ["path"], "properties": {"path": {"type": "string"}}},
            "outputSchema": {"description": "host-provided bounded file content"},
            "effects": ["file_read"],
            "permissions": ["runtime path approval"],
            "authoring": "always available; path consent and adapter checks happen at execution"
        }),
        json!({
            "contentId": content_id,
            "procedureId": "file.write",
            "name": "Write a scoped file",
            "primitive": "file_write",
            "inputSchema": {"type": "object", "required": ["path", "content"], "properties": {"path": {"type": "string"}, "content": {}}},
            "outputSchema": {"description": "host-provided write receipt"},
            "effects": ["file_write"],
            "permissions": ["runtime path approval"],
            "authoring": "always available; path consent and adapter checks happen at execution"
        }),
        json!({
            "contentId": content_id,
            "procedureId": "observe",
            "name": "Observe a host-provided target",
            "primitive": "observe",
            "inputSchema": {"type": "object", "required": ["target"], "properties": {"target": {"type": "string"}}},
            "outputSchema": {"description": "host-provided observation"},
            "effects": ["observation"],
            "permissions": ["runtime target approval"],
            "authoring": "always available; target consent and adapter checks happen at execution"
        }),
        json!({
            "contentId": content_id,
            "procedureId": "sandbox.execute",
            "name": "Execute in a host sandbox",
            "primitive": "sandbox_execute",
            "inputSchema": {"type": "object", "required": ["command"], "properties": {"command": {"type": "string"}}},
            "outputSchema": {"description": "host-provided bounded execution result"},
            "effects": ["sandboxed_execution"],
            "permissions": ["runtime sandbox approval"],
            "authoring": "always available; sandbox consent and adapter checks happen at execution"
        }),
    ]
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
    primitive_set: &str,
    drafts: &[ConditionDraft],
    parameters: &HashSet<String>,
    allow_result: bool,
    dependencies: &HashMap<String, TeacherDependency>,
) -> Result<(Vec<Condition>, HashSet<String>), EngineError> {
    let mut used_dependencies = HashSet::new();
    let conditions = drafts
        .iter()
        .map(|draft| {
            if draft.description.trim().is_empty()
                || draft.description.chars().count() > MAX_TEACHER_TEXT_CHARS
            {
                return Err(lesson_error(
                    "contract descriptions must be nonempty and bounded",
                ));
            }
            let (check, _, dependencies_used_by_check) = compile_lesson_body(
                primitive_set,
                &draft.check,
                parameters,
                allow_result,
                dependencies,
            )?;
            used_dependencies.extend(dependencies_used_by_check);
            Ok(Condition::described(&draft.description).with_check(check))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((conditions, used_dependencies))
}

fn compile_lesson_body(
    primitive_set: &str,
    value: &JsonValue,
    parameters: &HashSet<String>,
    allow_result: bool,
    dependencies: &HashMap<String, TeacherDependency>,
) -> Result<(Expr, HashSet<String>, HashSet<String>), EngineError> {
    match primitive_set {
        "pure_rpn_v1" => {
            let draft: ProgramDraft = serde_json::from_value(value.clone())
                .map_err(|error| lesson_error(format!("invalid pure_rpn_v1 program: {error}")))?;
            let (expression, parameters) =
                compile_lesson_program(&draft, parameters, allow_result)?;
            Ok((expression, parameters, HashSet::new()))
        }
        "pure_expr_v2" => {
            let value = if let Some(source) = value.as_str() {
                spoon_core::spoonlang::parse_expr(source).map_err(|error| {
                    lesson_error(format!("invalid spoonlang expression: {error}"))
                })?
            } else {
                value.clone()
            };
            let draft: ExprDraft = serde_json::from_value(value).map_err(|error| {
                lesson_error(format!("invalid pure_expr_v2 expression: {error}"))
            })?;
            compile_lesson_expression(&draft, parameters, allow_result, dependencies)
        }
        _ => Err(lesson_error("unsupported primitive set")),
    }
}

struct ExprCompileState<'a> {
    parameters: &'a HashSet<String>,
    dependencies: &'a HashMap<String, TeacherDependency>,
    scopes: Vec<HashSet<String>>,
    used_parameters: HashSet<String>,
    used_dependencies: HashSet<String>,
    binder_names: HashSet<String>,
    nodes: usize,
    allow_result: bool,
}

fn compile_lesson_expression(
    draft: &ExprDraft,
    parameters: &HashSet<String>,
    allow_result: bool,
    dependencies: &HashMap<String, TeacherDependency>,
) -> Result<(Expr, HashSet<String>, HashSet<String>), EngineError> {
    let mut state = ExprCompileState {
        parameters,
        dependencies,
        scopes: Vec::new(),
        used_parameters: HashSet::new(),
        used_dependencies: HashSet::new(),
        binder_names: HashSet::new(),
        nodes: 0,
        allow_result,
    };
    let expression = compile_expr_node(draft, &mut state, 0)?;
    Ok((expression, state.used_parameters, state.used_dependencies))
}

fn compile_expr_node(
    draft: &ExprDraft,
    state: &mut ExprCompileState<'_>,
    depth: usize,
) -> Result<Expr, EngineError> {
    if depth > MAX_LESSON_EXPR_DEPTH {
        return Err(lesson_error(
            "pure_expr_v2 expression exceeds maximum depth",
        ));
    }
    state.nodes += 1;
    if state.nodes > MAX_LESSON_EXPR_NODES {
        return Err(lesson_error(
            "pure_expr_v2 expression exceeds maximum node count",
        ));
    }
    match draft {
        ExprDraft::Literal { value } => {
            validate_lesson_value(value, 0)?;
            Ok(Expr::Literal(value.clone()))
        }
        ExprDraft::Parameter { name } => {
            validate_lesson_token(name, "parameter or binder name", MAX_LESSON_KEY_CHARS)?;
            if state.parameters.contains(name) {
                state.used_parameters.insert(name.clone());
            } else if !state.scopes.iter().rev().any(|scope| scope.contains(name)) {
                return Err(lesson_error(
                    "pure_expr_v2 expression referenced an undeclared parameter or binder",
                ));
            }
            Ok(Expr::Var(name.clone()))
        }
        ExprDraft::Result => {
            if !state.allow_result {
                return Err(lesson_error(
                    "result is allowed only in pure_expr_v2 promise checks",
                ));
            }
            Ok(Expr::Var("result".into()))
        }
        ExprDraft::Binary { op, left, right } => Ok(Expr::BinOp {
            op: compile_binary_op(*op),
            left: Box::new(compile_expr_node(left, state, depth + 1)?),
            right: Box::new(compile_expr_node(right, state, depth + 1)?),
        }),
        ExprDraft::Unary { op, operand } => Ok(Expr::UnOp {
            op: compile_unary_op(*op),
            operand: Box::new(compile_expr_node(operand, state, depth + 1)?),
        }),
        ExprDraft::If {
            condition,
            then,
            else_,
        } => Ok(Expr::If {
            cond: Box::new(compile_expr_node(condition, state, depth + 1)?),
            then: Box::new(compile_expr_node(then, state, depth + 1)?),
            else_: Box::new(compile_expr_node(else_, state, depth + 1)?),
        }),
        ExprDraft::Let { name, value, body } => {
            let value = compile_expr_node(value, state, depth + 1)?;
            bind_expr_name(name, state)?;
            state.scopes.push(HashSet::from([name.clone()]));
            let body = compile_expr_node(body, state, depth + 1)?;
            state.scopes.pop();
            Ok(Expr::Let {
                name: name.clone(),
                value: Box::new(value),
                body: Box::new(body),
            })
        }
        ExprDraft::List { items } => {
            validate_expr_children(items.len())?;
            Ok(Expr::ListExpr(
                items
                    .iter()
                    .map(|item| compile_expr_node(item, state, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        ExprDraft::Index { collection, index } => Ok(Expr::Index {
            collection: Box::new(compile_expr_node(collection, state, depth + 1)?),
            index: Box::new(compile_expr_node(index, state, depth + 1)?),
        }),
        ExprDraft::Field { object, field } => {
            validate_lesson_token(field, "field name", MAX_LESSON_KEY_CHARS)?;
            Ok(Expr::FieldAccess {
                object: Box::new(compile_expr_node(object, state, depth + 1)?),
                field: field.clone(),
            })
        }
        ExprDraft::Map {
            collection,
            var,
            body,
        } => {
            let collection = compile_expr_node(collection, state, depth + 1)?;
            bind_expr_name(var, state)?;
            state.scopes.push(HashSet::from([var.clone()]));
            let body = compile_expr_node(body, state, depth + 1)?;
            state.scopes.pop();
            Ok(Expr::Map {
                collection: Box::new(collection),
                var: var.clone(),
                body: Box::new(body),
            })
        }
        ExprDraft::Filter {
            collection,
            var,
            predicate,
        } => {
            let collection = compile_expr_node(collection, state, depth + 1)?;
            bind_expr_name(var, state)?;
            state.scopes.push(HashSet::from([var.clone()]));
            let predicate = compile_expr_node(predicate, state, depth + 1)?;
            state.scopes.pop();
            Ok(Expr::Filter {
                collection: Box::new(collection),
                var: var.clone(),
                predicate: Box::new(predicate),
            })
        }
        ExprDraft::Reduce {
            collection,
            init,
            acc,
            var,
            body,
        } => {
            let collection = compile_expr_node(collection, state, depth + 1)?;
            let init = compile_expr_node(init, state, depth + 1)?;
            bind_expr_name(acc, state)?;
            bind_expr_name(var, state)?;
            state.scopes.push(HashSet::from([acc.clone(), var.clone()]));
            let body = compile_expr_node(body, state, depth + 1)?;
            state.scopes.pop();
            Ok(Expr::Reduce {
                collection: Box::new(collection),
                init: Box::new(init),
                acc: acc.clone(),
                var: var.clone(),
                body: Box::new(body),
            })
        }
        ExprDraft::Intrinsic { version, op, args } => {
            if *version != 1 {
                return Err(lesson_error(
                    "pure_expr_v2 supports only intrinsic version 1",
                ));
            }
            validate_expr_children(args.len())?;
            Ok(Expr::Intrinsic {
                version: 1,
                op: compile_intrinsic_op(*op),
                args: args
                    .iter()
                    .map(|arg| compile_expr_node(arg, state, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        ExprDraft::Dependency { alias, args } => {
            validate_lesson_token(alias, "dependency alias", MAX_LESSON_KEY_CHARS)?;
            validate_expr_children(args.len())?;
            let dependency = state.dependencies.get(alias).ok_or_else(|| {
                lesson_error("pure_expr_v2 expression referenced an unadvertised procedure alias")
            })?;
            state.used_dependencies.insert(alias.clone());
            Ok(Expr::CallExact {
                procedure: dependency.procedure,
                version: dependency.version,
                args: args
                    .iter()
                    .map(|argument| compile_expr_node(argument, state, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        ExprDraft::CapabilityCall {
            content_id,
            procedure_id,
            input,
        } => {
            validate_lesson_token(content_id, "capability content id", MAX_TEACHER_TEXT_CHARS)?;
            validate_lesson_token(
                procedure_id,
                "capability procedure id",
                MAX_LESSON_KEY_CHARS,
            )?;
            Ok(Expr::CapabilityCall {
                content_id: content_id.clone(),
                procedure_id: procedure_id.clone(),
                input: Box::new(compile_expr_node(input, state, depth + 1)?),
            })
        }
    }
}

fn bind_expr_name(name: &str, state: &mut ExprCompileState<'_>) -> Result<(), EngineError> {
    validate_lesson_token(name, "binder name", MAX_LESSON_KEY_CHARS)?;
    if state.parameters.contains(name) || !state.binder_names.insert(name.to_owned()) {
        return Err(lesson_error(
            "pure_expr_v2 binder names must be unique and cannot shadow parameters",
        ));
    }
    Ok(())
}

fn validate_expr_children(length: usize) -> Result<(), EngineError> {
    if length > MAX_LESSON_EXPR_CHILDREN {
        return Err(lesson_error(
            "pure_expr_v2 expression exceeds maximum child count",
        ));
    }
    Ok(())
}

fn compile_binary_op(op: BinaryOpDraft) -> BinOp {
    match op {
        BinaryOpDraft::Add => BinOp::Add,
        BinaryOpDraft::Subtract => BinOp::Sub,
        BinaryOpDraft::Multiply => BinOp::Mul,
        BinaryOpDraft::Divide => BinOp::Div,
        BinaryOpDraft::Modulo => BinOp::Mod,
        BinaryOpDraft::Equal => BinOp::Eq,
        BinaryOpDraft::NotEqual => BinOp::Ne,
        BinaryOpDraft::LessThan => BinOp::Lt,
        BinaryOpDraft::LessOrEqual => BinOp::Le,
        BinaryOpDraft::GreaterThan => BinOp::Gt,
        BinaryOpDraft::GreaterOrEqual => BinOp::Ge,
        BinaryOpDraft::And => BinOp::And,
        BinaryOpDraft::Or => BinOp::Or,
    }
}

fn compile_unary_op(op: UnaryOpDraft) -> UnOp {
    match op {
        UnaryOpDraft::Negate => UnOp::Neg,
        UnaryOpDraft::Not => UnOp::Not,
    }
}

const PURE_PRIMITIVE_NAMES: &[&str] = &[
    "add",
    "sub",
    "mul",
    "div",
    "mod",
    "eq",
    "ne",
    "lt",
    "le",
    "gt",
    "ge",
    "and",
    "or",
    "neg",
    "not",
    "index",
    "field_access",
    "length",
    "text_byte_length",
    "text_scalar_length",
    "text_grapheme_length",
    "text_tokenize",
    "text_split",
    "text_join",
    "text_trim",
    "text_lowercase",
    "text_uppercase",
    "text_contains",
    "text_starts_with",
    "text_ends_with",
    "text_replace",
    "collection_contains",
    "collection_find_index",
    "count_equal",
    "map_keys",
    "map_values",
    "json_parse",
    "json_stringify",
    "path_get",
    "path_get_optional",
    "json_pointer_get",
    "json_pointer_get_optional",
    "json_pointer_set",
    "json_pointer_delete",
    "coalesce",
    "text_normalize_nfc",
    "text_normalize_nfd",
    "text_normalize_nfkc",
    "text_normalize_nfkd",
    "text_trim_start",
    "text_trim_end",
    "text_grapheme_substring",
    "text_index_of",
    "text_count",
    "text_repeat",
    "text_concat_many",
    "map_entries",
    "map_from_entries",
    "map_set",
    "map_delete",
    "map_merge",
    "collection_slice",
    "collection_reverse",
    "collection_sort",
    "collection_unique",
    "collection_flatten",
    "collection_zip",
    "range",
    "type_name",
    "parse_int",
    "parse_float",
    "parse_bool",
    "to_text",
    "numeric_abs",
    "numeric_sign",
    "numeric_min",
    "numeric_max",
    "numeric_clamp",
    "numeric_floor",
    "numeric_ceil",
    "numeric_round",
    "numeric_truncate",
    "numeric_pow_int",
    "numeric_pow_float",
    "integer_quotient",
    "integer_remainder",
];

fn interpreter_primitive_catalog(relevant_names: &BTreeSet<String>) -> Vec<JsonValue> {
    PURE_PRIMITIVE_NAMES
        .iter()
        .filter(|name| relevant_names.contains(**name))
        .map(|name| {
            let description = match *name {
                "text_count" => {
                    "Count exact substring, character, or letter occurrences; answer how often text appears"
                }
                "count_equal" => "Count collection items equal to a target value",
                "path_get" | "path_get_optional" => {
                    "Read nested object fields or array indexes using a path such as arr[0].name"
                }
                "json_parse" => "Parse JSON text into structured data",
                "json_pointer_get" | "json_pointer_get_optional" => {
                    "Read structured data using an RFC 6901 JSON pointer"
                }
                "mul" => "Multiply two numeric values",
                "div" => "Divide one numeric value by another",
                "add" => "Add numeric values",
                "sub" => "Subtract numeric values",
                _ => "Pure portable Spoon IR operation",
            };
            json!({
                "kind": "primitive",
                "name": name,
                "description": description,
                "directlySelectable": false,
            })
        })
        .collect()
}

fn interpreter_capability_catalog() -> Vec<JsonValue> {
    [
        (
            "web.fetch",
            "Fetch an HTTP(S) URL through a locally validated, host-allowlisted network capability procedure",
        ),
        (
            "file_read",
            "Read a policy-authorized file through a locally validated capability procedure",
        ),
        (
            "file_write",
            "Write a policy-authorized file through a locally validated capability procedure",
        ),
        (
            "observe",
            "Observe a policy-authorized external target through a native boundary",
        ),
        (
            "sandbox_execute",
            "Run bounded work in a policy-authorized sandbox profile",
        ),
    ]
    .into_iter()
    .map(|(name, description)| {
        json!({
            "kind": "capability",
            "name": name,
            "description": description,
            "directlySelectable": false,
            "requires": "locally_validated_procedure_and_permission",
        })
    })
    .collect()
}

fn procedure_intrinsic_names(expr: &Expr) -> Vec<String> {
    fn visit(expr: &Expr, names: &mut BTreeSet<String>) {
        match expr {
            Expr::Literal(_) | Expr::Var(_) => {}
            Expr::BinOp { op, left, right } => {
                names.insert(snake_case_variant(&format!("{op:?}")));
                visit(left, names);
                visit(right, names);
            }
            Expr::UnOp { op, operand } => {
                names.insert(snake_case_variant(&format!("{op:?}")));
                visit(operand, names);
            }
            Expr::CapabilityCall { input, .. } => visit(input, names),
            Expr::Call { args, .. } | Expr::CallExact { args, .. } => {
                for arg in args {
                    visit(arg, names);
                }
            }
            Expr::If { cond, then, else_ } => {
                visit(cond, names);
                visit(then, names);
                visit(else_, names);
            }
            Expr::Let { value, body, .. } => {
                visit(value, names);
                visit(body, names);
            }
            Expr::Block(items) | Expr::ListExpr(items) => {
                for item in items {
                    visit(item, names);
                }
            }
            Expr::Index { collection, index } => {
                names.insert("index".into());
                visit(collection, names);
                visit(index, names);
            }
            Expr::FieldAccess { object, .. } => {
                names.insert("field_access".into());
                visit(object, names);
            }
            Expr::Map {
                collection, body, ..
            } => {
                visit(collection, names);
                visit(body, names);
            }
            Expr::Filter {
                collection,
                predicate,
                ..
            } => {
                visit(collection, names);
                visit(predicate, names);
            }
            Expr::Reduce {
                collection,
                init,
                body,
                ..
            } => {
                visit(collection, names);
                visit(init, names);
                visit(body, names);
            }
            Expr::Intrinsic { op, args, .. } => {
                names.insert(snake_case_variant(&format!("{op:?}")));
                for arg in args {
                    visit(arg, names);
                }
            }
        }
    }

    let mut names = BTreeSet::new();
    visit(expr, &mut names);
    names.into_iter().collect()
}

fn snake_case_variant(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for (index, character) in value.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                result.push('_');
            }
            result.push(character.to_ascii_lowercase());
        } else {
            result.push(character);
        }
    }
    result
}

fn compile_intrinsic_op(op: IntrinsicOpDraft) -> IntrinsicOp {
    match op {
        IntrinsicOpDraft::Length => IntrinsicOp::Length,
        IntrinsicOpDraft::TextByteLength => IntrinsicOp::TextByteLength,
        IntrinsicOpDraft::TextScalarLength => IntrinsicOp::TextScalarLength,
        IntrinsicOpDraft::TextGraphemeLength => IntrinsicOp::TextGraphemeLength,
        IntrinsicOpDraft::TextTokenize => IntrinsicOp::TextTokenize,
        IntrinsicOpDraft::TextSplit => IntrinsicOp::TextSplit,
        IntrinsicOpDraft::TextJoin => IntrinsicOp::TextJoin,
        IntrinsicOpDraft::TextTrim => IntrinsicOp::TextTrim,
        IntrinsicOpDraft::TextLowercase => IntrinsicOp::TextLowercase,
        IntrinsicOpDraft::TextUppercase => IntrinsicOp::TextUppercase,
        IntrinsicOpDraft::TextContains => IntrinsicOp::TextContains,
        IntrinsicOpDraft::TextStartsWith => IntrinsicOp::TextStartsWith,
        IntrinsicOpDraft::TextEndsWith => IntrinsicOp::TextEndsWith,
        IntrinsicOpDraft::TextReplace => IntrinsicOp::TextReplace,
        IntrinsicOpDraft::TextUrlEncode => IntrinsicOp::TextUrlEncode,
        IntrinsicOpDraft::TextRegexCapture => IntrinsicOp::TextRegexCapture,
        IntrinsicOpDraft::CollectionContains => IntrinsicOp::CollectionContains,
        IntrinsicOpDraft::CollectionFindIndex => IntrinsicOp::CollectionFindIndex,
        IntrinsicOpDraft::CountEqual => IntrinsicOp::CountEqual,
        IntrinsicOpDraft::MapKeys => IntrinsicOp::MapKeys,
        IntrinsicOpDraft::MapValues => IntrinsicOp::MapValues,
        IntrinsicOpDraft::JsonParse => IntrinsicOp::JsonParse,
        IntrinsicOpDraft::JsonStringify => IntrinsicOp::JsonStringify,
        IntrinsicOpDraft::PathGet => IntrinsicOp::PathGet,
        IntrinsicOpDraft::PathGetOptional => IntrinsicOp::PathGetOptional,
        IntrinsicOpDraft::JsonPointerGet => IntrinsicOp::JsonPointerGet,
        IntrinsicOpDraft::JsonPointerGetOptional => IntrinsicOp::JsonPointerGetOptional,
        IntrinsicOpDraft::JsonPointerSet => IntrinsicOp::JsonPointerSet,
        IntrinsicOpDraft::JsonPointerDelete => IntrinsicOp::JsonPointerDelete,
        IntrinsicOpDraft::Coalesce => IntrinsicOp::Coalesce,
        IntrinsicOpDraft::TextNormalizeNfc => IntrinsicOp::TextNormalizeNfc,
        IntrinsicOpDraft::TextNormalizeNfd => IntrinsicOp::TextNormalizeNfd,
        IntrinsicOpDraft::TextNormalizeNfkc => IntrinsicOp::TextNormalizeNfkc,
        IntrinsicOpDraft::TextNormalizeNfkd => IntrinsicOp::TextNormalizeNfkd,
        IntrinsicOpDraft::TextTrimStart => IntrinsicOp::TextTrimStart,
        IntrinsicOpDraft::TextTrimEnd => IntrinsicOp::TextTrimEnd,
        IntrinsicOpDraft::TextGraphemeSubstring => IntrinsicOp::TextGraphemeSubstring,
        IntrinsicOpDraft::TextIndexOf => IntrinsicOp::TextIndexOf,
        IntrinsicOpDraft::TextCount => IntrinsicOp::TextCount,
        IntrinsicOpDraft::TextRepeat => IntrinsicOp::TextRepeat,
        IntrinsicOpDraft::TextConcatMany => IntrinsicOp::TextConcatMany,
        IntrinsicOpDraft::MapEntries => IntrinsicOp::MapEntries,
        IntrinsicOpDraft::MapFromEntries => IntrinsicOp::MapFromEntries,
        IntrinsicOpDraft::MapSet => IntrinsicOp::MapSet,
        IntrinsicOpDraft::MapDelete => IntrinsicOp::MapDelete,
        IntrinsicOpDraft::MapMerge => IntrinsicOp::MapMerge,
        IntrinsicOpDraft::CollectionSlice => IntrinsicOp::CollectionSlice,
        IntrinsicOpDraft::CollectionReverse => IntrinsicOp::CollectionReverse,
        IntrinsicOpDraft::CollectionSort => IntrinsicOp::CollectionSort,
        IntrinsicOpDraft::CollectionUnique => IntrinsicOp::CollectionUnique,
        IntrinsicOpDraft::CollectionFlatten => IntrinsicOp::CollectionFlatten,
        IntrinsicOpDraft::CollectionZip => IntrinsicOp::CollectionZip,
        IntrinsicOpDraft::Range => IntrinsicOp::Range,
        IntrinsicOpDraft::TypeName => IntrinsicOp::TypeName,
        IntrinsicOpDraft::ParseInt => IntrinsicOp::ParseInt,
        IntrinsicOpDraft::ParseFloat => IntrinsicOp::ParseFloat,
        IntrinsicOpDraft::ParseBool => IntrinsicOp::ParseBool,
        IntrinsicOpDraft::ToText => IntrinsicOp::ToText,
        IntrinsicOpDraft::NumericAbs => IntrinsicOp::NumericAbs,
        IntrinsicOpDraft::NumericSign => IntrinsicOp::NumericSign,
        IntrinsicOpDraft::NumericMin => IntrinsicOp::NumericMin,
        IntrinsicOpDraft::NumericMax => IntrinsicOp::NumericMax,
        IntrinsicOpDraft::NumericClamp => IntrinsicOp::NumericClamp,
        IntrinsicOpDraft::NumericFloor => IntrinsicOp::NumericFloor,
        IntrinsicOpDraft::NumericCeil => IntrinsicOp::NumericCeil,
        IntrinsicOpDraft::NumericRound => IntrinsicOp::NumericRound,
        IntrinsicOpDraft::NumericTruncate => IntrinsicOp::NumericTruncate,
        IntrinsicOpDraft::NumericPowInt => IntrinsicOp::NumericPowInt,
        IntrinsicOpDraft::NumericPowFloat => IntrinsicOp::NumericPowFloat,
        IntrinsicOpDraft::IntegerQuotient => IntrinsicOp::IntegerQuotient,
        IntrinsicOpDraft::IntegerRemainder => IntrinsicOp::IntegerRemainder,
    }
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

fn lesson_procedures_are_acyclic(procedures: &[Procedure]) -> bool {
    let ids = procedures
        .iter()
        .map(|procedure| procedure.id)
        .collect::<HashSet<_>>();
    let mut dependencies = HashMap::<ProcedureId, HashSet<ProcedureId>>::new();
    for procedure in procedures {
        let mut calls = HashSet::new();
        crate::engine::collect_exact_calls(&procedure.body, &mut calls);
        for condition in procedure
            .contract
            .requires
            .iter()
            .chain(&procedure.contract.promises)
            .chain(&procedure.contract.fails_when)
        {
            if let Some(check) = &condition.check {
                crate::engine::collect_exact_calls(check, &mut calls);
            }
        }
        dependencies.insert(
            procedure.id,
            calls
                .into_iter()
                .filter_map(|(id, _)| ids.contains(&id).then_some(id))
                .collect(),
        );
    }

    fn visit(
        current: ProcedureId,
        dependencies: &HashMap<ProcedureId, HashSet<ProcedureId>>,
        visiting: &mut HashSet<ProcedureId>,
        visited: &mut HashSet<ProcedureId>,
    ) -> bool {
        if visited.contains(&current) {
            return true;
        }
        if !visiting.insert(current) {
            return false;
        }
        let valid = dependencies.get(&current).is_none_or(|items| {
            items
                .iter()
                .all(|item| visit(*item, dependencies, visiting, visited))
        });
        visiting.remove(&current);
        if valid {
            visited.insert(current);
        }
        valid
    }

    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    ids.into_iter()
        .all(|id| visit(id, &dependencies, &mut visiting, &mut visited))
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

fn validate_lesson_value(value: &Value, depth: usize) -> Result<(), EngineError> {
    if depth > MAX_LESSON_VALUE_DEPTH {
        return Err(lesson_error("pure_expr_v2 value exceeds maximum depth"));
    }
    match value {
        Value::Text(text) if text.chars().count() > MAX_TEACHER_TEXT_CHARS => Err(lesson_error(
            "pure_expr_v2 text value exceeds maximum length",
        )),
        Value::List(items) => {
            if items.len() > MAX_LESSON_VALUE_ITEMS {
                return Err(lesson_error(
                    "pure_expr_v2 list value exceeds maximum item count",
                ));
            }
            for item in items {
                validate_lesson_value(item, depth + 1)?;
            }
            Ok(())
        }
        Value::Map(items) => {
            if items.len() > MAX_LESSON_VALUE_ITEMS {
                return Err(lesson_error(
                    "pure_expr_v2 map value exceeds maximum item count",
                ));
            }
            for (key, item) in items {
                validate_lesson_token(key, "map key", MAX_LESSON_KEY_CHARS)?;
                validate_lesson_value(item, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn bootstrap_reference_lifecycle(lifecycle: Lifecycle) -> bool {
    matches!(
        lifecycle,
        Lifecycle::Active | Lifecycle::Validated | Lifecycle::Provisional
    )
}

fn stable_lesson_digest(bytes: &[u8]) -> String {
    let digest = lesson_sha256(b"spoon:teacher-lesson:idempotency:v1", bytes);
    format!("sha256:{}", hex_bytes(&digest))
}

fn deterministic_lesson_uuid(canonical: &[u8], entity_kind: &str, key: &str) -> Uuid {
    let mut identity = Vec::with_capacity(canonical.len() + entity_kind.len() + key.len() + 2);
    identity.extend_from_slice(entity_kind.as_bytes());
    identity.push(0);
    identity.extend_from_slice(key.as_bytes());
    identity.push(0);
    identity.extend_from_slice(canonical);
    let digest = lesson_sha256(b"spoon:teacher-lesson:entity-uuid:v1", &identity);
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
    if input.budget.max_context_items > spoon_reason::MAX_CONTEXT_COLLECTION_ITEMS {
        return Err(EngineError::InvalidInput(format!(
            "max_context_items exceeds hard maximum {}",
            spoon_reason::MAX_CONTEXT_COLLECTION_ITEMS
        )));
    }
    if input.situation.chars().count() > spoon_reason::MAX_CONTEXT_TEXT_CHARS {
        return Err(EngineError::InvalidInput(
            "situation exceeds hard maximum".into(),
        ));
    }
    if input.recall_mode == RecallMode::Session && input.session_id.is_none() {
        return Err(EngineError::InvalidInput(
            "session recall requires session_id".into(),
        ));
    }
    if let Some(session_id) = input.session_id.as_deref() {
        Uuid::parse_str(session_id)
            .map_err(|_| EngineError::InvalidInput("session_id is not a UUID".into()))?;
    }
    if let Some(permission_mode) = input.permission_mode.as_deref()
        && !matches!(
            permission_mode,
            "ask" | "workspace" | "full-access" | "god-mode"
        )
    {
        return Err(EngineError::InvalidInput(
            "permission_mode must be ask, workspace, full-access, or god-mode".into(),
        ));
    }
    if input.assumptions.len() > spoon_reason::MAX_CONTEXT_COLLECTION_ITEMS
        || input.environment.len() > spoon_reason::MAX_CONTEXT_COLLECTION_ITEMS
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
        if assumption.description.chars().count() > spoon_reason::MAX_CONTEXT_TEXT_CHARS
            || assumption.basis.chars().count() > spoon_reason::MAX_CONTEXT_TEXT_CHARS
        {
            return Err(EngineError::InvalidInput(
                "assumption text exceeds hard maximum".into(),
            ));
        }
    }
    for (key, value) in &input.environment {
        if key.chars().count() > spoon_reason::MAX_CONTEXT_TEXT_CHARS {
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
    let mut literals = Vec::new();
    let mut unquoted = String::new();
    let mut characters = situation.chars();

    while let Some(character) = characters.next() {
        if matches!(character, '"' | '\'') {
            let Some(text) = scan_quoted_literal(&mut characters, character) else {
                // Do not partially bind a malformed request. In particular,
                // an unmatched quote must not let preceding scalar text shift
                // parameter positions for a learned procedure.
                return Vec::new();
            };
            literals.extend(extract_scalar_literals(&unquoted));
            unquoted.clear();
            literals.push(Value::Text(text));
        } else {
            unquoted.push(character);
        }
    }
    literals.extend(extract_scalar_literals(&unquoted));
    literals
}

fn extract_scalar_literals(text: &str) -> Vec<Value> {
    text.split(|character: char| {
        character.is_whitespace()
            || matches!(
                character,
                ',' | '?' | '!' | '(' | ')' | '=' | '*' | '/' | '×' | '÷' | '+'
            )
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

/// Read a deliberately narrow JSON-like string literal from a user situation.
///
/// This is an explicit input-binding syntax, not a natural-language parser.
/// The outer cycle already bounds the full situation, while this scanner rejects
/// unterminated and unknown escape sequences before any procedure can run.
fn scan_quoted_literal(characters: &mut std::str::Chars<'_>, quote: char) -> Option<String> {
    let mut text = String::new();
    let mut text_chars = 0usize;
    while let Some(character) = characters.next() {
        match character {
            character if character == quote => return Some(text),
            '\\' => match characters.next()? {
                '"' => text.push('"'),
                '\'' => text.push('\''),
                '\\' => text.push('\\'),
                'n' => text.push('\n'),
                'r' => text.push('\r'),
                't' => text.push('\t'),
                _ => return None,
            },
            character => text.push(character),
        }
        text_chars += 1;
        if text_chars > spoon_reason::MAX_CONTEXT_TEXT_CHARS {
            return None;
        }
    }
    None
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
            "recall: found a verified cached result"
        } else {
            "recall: no verified result in cache, escalating"
        },
        EscalationRung::Recall,
    )];
    if terminal_rung >= EscalationRung::Run {
        steps.push(simple_step(
            if terminal_rung == EscalationRung::Run {
                "run: uniquely matched a known procedure, executing directly"
            } else {
                "run: no unique local match, escalating"
            },
            EscalationRung::Run,
        ));
    }
    if terminal_rung >= EscalationRung::Compose && terminal_rung != EscalationRung::Ask {
        steps.push(simple_step(
            if terminal_rung == EscalationRung::Compose {
                "compose: built a new procedure by chaining known ones"
            } else {
                "compose: could not compose from known procedures, escalating"
            },
            EscalationRung::Compose,
        ));
    }
    if teacher_was_used {
        steps.push(simple_step(
            "ask: escalated to teacher for guidance",
            EscalationRung::Ask,
        ));
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

fn apply_spoonlang_source(content: &mut ProposalContent) -> Result<(), EngineError> {
    let Some(source) = content
        .source
        .as_deref()
        .map(str::trim)
        .filter(|source| !source.is_empty())
    else {
        return Ok(());
    };
    if let Ok(value) = serde_json::from_str::<JsonValue>(source)
        && value.get("primitiveSet").is_some()
        && value.get("procedures").is_some()
    {
        content.proposal_kind = Some(ProposalKind::ReusableLesson);
        content.lesson = Some(value);
        return Ok(());
    }
    let parsed = spoon_core::spoonlang::parse_proposal(source)
        .map_err(|error| EngineError::InvalidInput(error.to_string()))?;
    content.proposal_kind = Some(match parsed.kind {
        spoon_core::spoonlang::SpoonlangKind::ReusableLesson => ProposalKind::ReusableLesson,
        spoon_core::spoonlang::SpoonlangKind::ExternalObservation => {
            ProposalKind::ExternalObservation
        }
        spoon_core::spoonlang::SpoonlangKind::AnswerOnly => ProposalKind::AnswerOnly,
        spoon_core::spoonlang::SpoonlangKind::Abstain => ProposalKind::Abstain,
    });
    content.lesson = parsed.lesson;
    if let Some(answer) = parsed.answer {
        content.answer = serde_json::from_value(answer).ok();
    }
    if parsed.abstain_reason.is_some() {
        content.abstain_reason = parsed.abstain_reason;
    }
    Ok(())
}

pub fn proposal_schema() -> JsonValue {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "source": { "type": "string" },
            "interpretations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "concept": { "type": "string" },
                        "weight": { "type": "number" },
                    },
                    "required": ["concept", "weight"],
                },
            },
        },
        "required": ["source", "interpretations"],
    })
}

#[allow(dead_code)]
fn expr_lesson_schema() -> JsonValue {
    let structured_value =
        json!({ "type": ["null", "boolean", "number", "string", "array", "object"] });
    let expression = expr_program_schema();
    let condition = json!({
        "type": "object", "additionalProperties": false,
        "properties": { "description": { "type": "string" }, "check": expression },
        "required": ["description", "check"]
    });
    let concept_reference = json!({
        "anyOf": [
            { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "new_concept" }, "key": { "type": "string" } }, "required": ["kind", "key"] },
            { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "existing_concept" }, "id": { "type": "string" } }, "required": ["kind", "id"] }
        ]
    });
    let parameter_type = json!({
        "type": "string",
        "enum": ["any", "null", "bool", "number", "text", "list", "map"],
    });
    json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "primitiveSet": { "type": "string", "const": "pure_expr_v2" },
            "concepts": { "type": "array", "minItems": 1, "maxItems": MAX_LESSON_CONCEPTS, "items": { "type": "object", "additionalProperties": false, "properties": { "key": { "type": "string" }, "name": { "type": "string" }, "description": { "type": "string" }, "mutability": { "type": "string", "enum": ["definitional", "defeasible_general", "procedural"] } }, "required": ["key", "name", "description", "mutability"] } },
            "relationships": { "type": "array", "maxItems": MAX_LESSON_RELATIONSHIPS, "items": { "type": "object", "additionalProperties": false, "properties": { "source": concept_reference.clone(), "target": concept_reference, "kind": { "type": "string" }, "strength": { "type": "number", "minimum": 0, "maximum": 1 } }, "required": ["source", "target", "kind", "strength"] } },
            "procedures": { "type": "array", "minItems": 1, "maxItems": MAX_LESSON_PROCEDURES, "items": { "type": "object", "additionalProperties": false, "properties": { "key": { "type": "string" }, "name": { "type": "string" }, "concept": concept_reference, "parameters": { "type": "array", "minItems": 1, "maxItems": MAX_LESSON_PARAMETERS, "items": { "type": "object", "additionalProperties": false, "properties": { "name": { "type": "string" }, "description": { "type": "string" }, "valueType": parameter_type.clone() }, "required": ["name", "description", "valueType"] } }, "body": expression, "contract": { "type": "object", "additionalProperties": false, "properties": { "requires": { "type": "array", "maxItems": MAX_LESSON_CONDITIONS, "items": condition.clone() }, "promises": { "type": "array", "maxItems": MAX_LESSON_CONDITIONS, "items": condition.clone() }, "failsWhen": { "type": "array", "maxItems": MAX_LESSON_CONDITIONS, "items": condition } }, "required": ["requires", "promises", "failsWhen"] } }, "required": ["key", "name", "concept", "parameters", "body", "contract"] } },
            "invocation": { "type": "object", "additionalProperties": false, "properties": { "procedureKey": { "type": "string" }, "inputs": { "type": "array", "maxItems": MAX_LESSON_PARAMETERS, "items": { "type": "object", "additionalProperties": false, "properties": { "name": { "type": "string" }, "value": structured_value }, "required": ["name", "value"] } } }, "required": ["procedureKey", "inputs"] }
        },
        "required": ["primitiveSet", "concepts", "relationships", "procedures", "invocation"]
    })
}

#[allow(dead_code)]
fn expr_program_schema() -> JsonValue {
    json!({ "$ref": "#/$defs/pureExprV2" })
}

#[allow(dead_code)]
fn expr_definition_schema(value: JsonValue) -> JsonValue {
    let expression = json!({ "$ref": "#/$defs/pureExprV2" });
    let leaf = |kind: &str| json!({ "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": kind } }, "required": ["kind"] });
    let named = |kind: &str| json!({ "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": kind }, "name": { "type": "string" } }, "required": ["kind", "name"] });
    let binary_ops = [
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
    ];
    let unary_ops = ["negate", "not"];
    json!({
        "anyOf": [
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "literal" }, "value": value }, "required": ["kind", "value"] },
                    named("parameter"), leaf("result"),
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "binary" }, "op": { "type": "string", "enum": binary_ops }, "left": expression.clone(), "right": expression.clone() }, "required": ["kind", "op", "left", "right"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "unary" }, "op": { "type": "string", "enum": unary_ops }, "operand": expression.clone() }, "required": ["kind", "op", "operand"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "if" }, "condition": expression.clone(), "then": expression.clone(), "else": expression.clone() }, "required": ["kind", "condition", "then", "else"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "let" }, "name": { "type": "string" }, "value": expression.clone(), "body": expression.clone() }, "required": ["kind", "name", "value", "body"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "list" }, "items": { "type": "array", "maxItems": MAX_LESSON_EXPR_CHILDREN, "items": expression.clone() } }, "required": ["kind", "items"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "index" }, "collection": expression.clone(), "index": expression.clone() }, "required": ["kind", "collection", "index"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "field" }, "object": expression.clone(), "field": { "type": "string" } }, "required": ["kind", "object", "field"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "map" }, "collection": expression.clone(), "var": { "type": "string" }, "body": expression.clone() }, "required": ["kind", "collection", "var", "body"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "filter" }, "collection": expression.clone(), "var": { "type": "string" }, "predicate": expression.clone() }, "required": ["kind", "collection", "var", "predicate"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "reduce" }, "collection": expression.clone(), "init": expression.clone(), "acc": { "type": "string" }, "var": { "type": "string" }, "body": expression.clone() }, "required": ["kind", "collection", "init", "acc", "var", "body"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "dependency" }, "alias": { "type": "string" }, "args": { "type": "array", "maxItems": MAX_LESSON_EXPR_CHILDREN, "items": expression.clone() } }, "required": ["kind", "alias", "args"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "capability_call" }, "contentId": { "type": "string", "minLength": 1, "maxLength": MAX_TEACHER_TEXT_CHARS }, "procedureId": { "type": "string", "minLength": 1, "maxLength": MAX_LESSON_KEY_CHARS }, "input": expression.clone() }, "required": ["kind", "contentId", "procedureId", "input"] },
                    { "type": "object", "additionalProperties": false, "properties": { "kind": { "type": "string", "const": "intrinsic" }, "version": { "type": "integer", "const": 1 }, "op": { "type": "string", "enum": [
                        "length", "text_byte_length", "text_scalar_length", "text_grapheme_length", "text_tokenize",
                        "text_split", "text_join", "text_trim", "text_lowercase", "text_uppercase",
                        "text_contains", "text_starts_with", "text_ends_with", "text_replace", "text_url_encode",
                        "text_regex_capture", "text_normalize_nfc", "text_normalize_nfd", "text_normalize_nfkc", "text_normalize_nfkd",
                        "text_trim_start", "text_trim_end", "text_grapheme_substring", "text_index_of", "text_count",
                        "text_repeat", "text_concat_many",
                        "text_pad_start", "text_pad_end", "text_substring", "text_char_at", "text_format",
                        "text_matches_regex", "text_regex_replace_all", "text_base64_encode", "text_base64_decode",
                        "text_url_decode", "text_hex_encode", "text_hex_decode", "text_reverse",
                        "text_char_code", "text_from_char_code", "text_levenshtein",
                        "collection_contains", "collection_find_index", "count_equal",
                        "collection_slice", "collection_reverse", "collection_sort", "collection_unique",
                        "collection_flatten", "collection_zip", "range",
                        "collection_group_by", "collection_sort_by", "collection_min_by", "collection_max_by",
                        "collection_chunk", "collection_enumerate", "collection_any", "collection_all",
                        "collection_take", "collection_drop", "collection_first", "collection_last",
                        "collection_partition", "collection_repeat_value", "collection_window",
                        "map_keys", "map_values", "map_entries", "map_from_entries", "map_set", "map_delete", "map_merge",
                        "map_has_key", "map_get_default", "map_size", "map_filter_keys",
                        "json_parse", "json_stringify", "path_get", "path_get_optional",
                        "json_pointer_get", "json_pointer_get_optional", "json_pointer_set", "json_pointer_delete",
                        "coalesce", "assert", "default_if_null",
                        "numeric_abs", "numeric_sign", "numeric_min", "numeric_max", "numeric_clamp",
                        "numeric_floor", "numeric_ceil", "numeric_round", "numeric_truncate",
                        "numeric_pow_int", "numeric_pow_float", "integer_quotient", "integer_remainder",
                        "math_sqrt", "math_log", "math_log10", "math_log2", "math_exp",
                        "math_sin", "math_cos", "math_tan", "math_asin", "math_acos", "math_atan", "math_atan2",
                        "math_pi", "math_e", "math_is_nan", "math_is_infinite",
                        "math_gcd", "math_lcm", "math_hypot",
                        "random_int", "random_float", "random_choice", "random_shuffle", "random_sample", "random_uuid",
                        "date_now", "date_from_parts", "date_get_part", "date_add", "date_diff", "date_format",
                        "type_name", "is_null", "is_bool", "is_int", "is_float", "is_text", "is_list", "is_map", "is_numeric",
                        "to_int", "to_float", "to_bool", "to_text", "parse_int", "parse_float", "parse_bool",
                        "set_union", "set_intersect", "set_difference", "set_is_subset",
                        "bit_and", "bit_or", "bit_xor", "bit_not", "bit_shift_left", "bit_shift_right",
                        "hash_sha256", "hash_md5",
                        "numeric_to_fixed", "numeric_to_hex", "numeric_from_hex", "numeric_to_binary", "numeric_from_binary"
                    ] }, "args": { "type": "array", "maxItems": MAX_LESSON_EXPR_CHILDREN, "items": expression } }, "required": ["kind", "version", "op", "args"] }
        ]
    })
}

#[allow(dead_code)]
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
        "primitiveSet": "pure_expr_v2",
        "surfaceLanguage": "spoonlang",
        "acceptedPrimitiveSets": ["pure_expr_v2", "pure_rpn_v1"],
        "proposalKinds": ["reusable_lesson", "external_observation", "answer_only", "abstain"],
        "grammar": spoon_core::spoonlang::SPOONLANG_GRAMMAR,
        "teacherProvides": [
            "spoonlang source", "concept name, description, and mutability", "relationship claim",
            "procedure parameters", "expression body", "example invocation and answer"
        ],
        "engineProvides": [
            "ids", "lifecycle", "version", "confidence", "timestamps", "test cases"
        ],
        "constraints": [
            "write spoonlang in source; the engine compiles it to pure_expr_v2",
            "body must use every declared parameter",
            "no generic calls, dependency ids/versions, effects, sensors, clocks, network, files, randomness, or opaque code; dep aliases are engine-resolved and exact-version pinned",
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

fn usable_lifecycle(lifecycle: spoon_core::Lifecycle) -> bool {
    matches!(
        lifecycle,
        spoon_core::Lifecycle::Active
            | spoon_core::Lifecycle::Validated
            | spoon_core::Lifecycle::Provisional
            | spoon_core::Lifecycle::UnderReview
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
    if depth > spoon_reason::MAX_CONTEXT_VALUE_DEPTH {
        return Err(EngineError::InvalidInput(
            "environment value exceeds hard depth maximum".into(),
        ));
    }
    match value {
        Value::Text(text) if text.chars().count() > spoon_reason::MAX_CONTEXT_TEXT_CHARS => Err(
            EngineError::InvalidInput("environment text exceeds hard maximum".into()),
        ),
        Value::List(items) => {
            if items.len() > spoon_reason::MAX_CONTEXT_COLLECTION_ITEMS {
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
            if items.len() > spoon_reason::MAX_CONTEXT_COLLECTION_ITEMS {
                return Err(EngineError::InvalidInput(
                    "environment map exceeds hard maximum".into(),
                ));
            }
            for (key, item) in items {
                if key.chars().count() > spoon_reason::MAX_CONTEXT_TEXT_CHARS {
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
        matches!(
            provider,
            "claude" | "codex" | "cursor" | "openai" | "ollama" | "human"
        )
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
