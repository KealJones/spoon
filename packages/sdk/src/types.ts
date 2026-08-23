export type JsonValue =
  null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

/**
 * Typed input/output for Spoon's bounded response-plan renderer. This is not
 * a general text-generation API: every rendered sentence is caller-supplied
 * claim text paired with an evidence reference, and the server reports that
 * those references are not independently verified by this endpoint.
 */
export type DialogueAct =
  | "Inform"
  | "Ask"
  | "Clarify"
  | "Confirm"
  | "Correct"
  | "Acknowledge"
  | "Refuse"
  | "Abstain";

export interface DialogueMove {
  act: DialogueAct;
  relatesToTurn: string | null;
}

export type EvidenceSourceKind =
  "Taught" | "SelfVerified" | "Inferred" | "Observed";

export interface ResponseEvidenceReference {
  id: string;
  sourceKind: EvidenceSourceKind;
  linkedEpisode: string | null;
}

export interface GroundedResponseClaim {
  id: string;
  text: string;
  evidence: ResponseEvidenceReference[];
  /** Retained server-side for validation only; never returned by render output. */
  provenance: string[];
}

export type PlannedResponseClaim =
  | { Grounded: GroundedResponseClaim }
  | { Unsupported: { id: string; reason: string } };

export type UncertaintyLevel = "Certain" | "Qualified" | "Unknown";

export interface ResponseUncertainty {
  level: UncertaintyLevel;
  disclosure: string | null;
}

export type ResponseTone = "Neutral" | "Direct" | "Warm" | "Formal";
export type ResponseRenderVariant = "Plain" | "Bulleted";

export interface ResponsePlan {
  dialogueMove: DialogueMove;
  claims: PlannedResponseClaim[];
  uncertainty: ResponseUncertainty;
  tone: ResponseTone;
  variant: ResponseRenderVariant;
}

/** Content-free overrides; tone remains metadata in the current renderer. */
export interface ResponseRenderOptions {
  tone?: ResponseTone;
  variant?: ResponseRenderVariant;
}

export interface RenderedResponsePlan {
  text: string;
  includedClaimIds: string[];
  omittedClaimIds: string[];
  uncertainty: ResponseUncertainty;
  tone: ResponseTone;
  dialogueMove: DialogueMove;
  audit: {
    renderer: "bounded_response_plan_v1";
    claimsSubmitted: number;
    evidenceStatus: "caller_supplied_unverified";
    provenanceRedacted: true;
    redacted: true;
  };
}

export interface ConceptInput {
  name: string;
  description?: string;
  [key: string]: JsonValue | undefined;
}

/** Wire shape returned by the bounded, read-only relationship collection. */
export interface RelationshipRecord {
  id: string;
  source: string;
  target: string;
  kind: string;
  strength: number;
  scope: JsonValue[];
  evidence: string[];
  lifecycle: string;
  created_at: number;
}

export interface EpisodeFilter {
  since?: number;
  until?: number;
  limit?: number;
  outcome?: "success" | "failure" | "any";
  rung?: string;
  conceptId?: string;
  sessionId?: string;
  sessionVisibility?: SessionVisibility;
  includeIsolated?: boolean;
}

export interface FeedbackInput {
  episodeId: string;
  observedResult: JsonValue;
  idempotencyKey: string;
}

export interface AuthenticatedObservationInput {
  predicate: string;
  value: JsonValue;
  scope?: Record<string, JsonValue>;
  evaluation: {
    tier: "Hard" | "Consensus";
    success: boolean;
    details: string;
    surprise?: number | null;
  };
  verifierIdentity: string;
}

export interface EpisodeFeedback extends FeedbackInput {
  id: string;
  evaluation: {
    tier: "Hard" | "Consensus" | "Deferred";
    success: boolean;
    details: string;
    surprise?: number | null;
  };
  source: {
    kind: string;
    actor?: string | null;
  };
  createdAt: number;
}

export interface SpoonClientOptions {
  adminToken?: string;
}

export type CapabilityPermission =
  | { kind: "network_host"; host: string }
  | { kind: "file_read_prefix"; path_prefix: string }
  | { kind: "file_write_prefix"; path_prefix: string }
  | { kind: "observe_target"; target: string }
  | { kind: "sandbox_profile"; profile: string };

export type CapabilityEffect =
  | "network"
  | "file_read"
  | "file_write"
  | "observation"
  | "sandboxed_execution";

export type ProvenanceIdentityKind = "author" | "discoverer";

export interface ProvenanceIdentity {
  kind: ProvenanceIdentityKind;
  scheme: string;
  identifier: string;
}

export type PortableEvidenceKind = "validation_episode" | "evidence";

export interface PortableEvidenceReference {
  kind: PortableEvidenceKind;
  identifier: string;
  digest: string;
}

export interface CapabilityProvenance {
  source: string;
  discoveredAt: number;
  interfaceFingerprint: string;
  identities: ProvenanceIdentity[];
  validationEpisodes: string[];
  evidenceReferences: PortableEvidenceReference[];
}

export interface CapabilityDependencyReference {
  name: string;
  version: string;
  contentHash: string;
}

export interface CapabilityDependency extends CapabilityDependencyReference {
  dependencies: CapabilityDependencyReference[];
}

export type NeutralProcedureKind = "native_primitive";

export interface NeutralProcedureMetadata {
  kind: NeutralProcedureKind;
  irVersion: number;
  fixtureFormat: string;
}

export interface ReconstructionStep {
  sequence: number;
  operation: string;
  artifactDigest?: string | null;
}

export interface ReconstructionRecipe {
  kind: string;
  recipeVersion: number;
  compatibility: string[];
  steps: ReconstructionStep[];
}

export interface CapabilityTest {
  name: string;
  input: JsonValue;
  expectedOutput: JsonValue;
  fixtureOutput: JsonValue;
}

export interface DiscoveredOperation {
  name: string;
  inputSchema: JsonValue;
  outputSchema: JsonValue;
  host: string;
  method: string;
  responseFixture: JsonValue;
}

export interface InterfaceDescription {
  source: string;
  fingerprint: string;
  operations: DiscoveredOperation[];
}

export interface LocalValidation {
  passed: boolean;
  validationEpisodes: string[];
  environmentDigest: string;
}

export interface ImportedCapability {
  contentId: string;
  name: string;
  status: "quarantined" | "provisional" | "active" | "rejected";
  locallyValidated: boolean;
}

export interface MetricsSnapshot {
  episodeCount: number;
  verifiedAnswerCount: number;
  rungDistribution: Array<[string, number]>;
  phase6: {
    /** Aggregate teacher-request evidence; not a per-domain weaning metric. */
    teacherInteractionEpisodes: number;
    teacherAssistedSuccesses: number;
    teacherFreeSuccesses: number;
    /** Bounded to the durable records inspected by the snapshot. */
    managedSkillRecordsExamined: number;
    /** Persisted replay verdicts where the challenger preserved correctness. */
    replayPreservedSkillVerdicts: number;
    replayRegressions: number;
    transferEligibleSkillVerdicts: number;
    currentlyPromotedSkills: number;
    postPromotionSkillUses: number;
    postPromotionSkillSuccesses: number;
  };
  /** Immutable benchmark/probe evidence. Metrics with too little evidence say so explicitly. */
  section38: Section38TelemetrySnapshot;
  intuition: {
    indexedDocuments: number;
    invertedTermRows: number;
    retrievalQueries: number;
    candidatesExamined: number;
    rankingExamples: number;
    rankingEvaluations: number;
    rankingSearchWins: number;
    semanticRecallEvaluations: number;
    semanticRecallWins: number;
    supervisionTasks: number;
    groundedTasks: number;
    groundingRatio: number;
  };
}

export type ProbeCohort = "training" | "heldOut";
export type TeacherMode = "on" | "off" | "optional";
export type GroundingTier = "none" | "teacher" | "soft" | "strong";
export type MetricEvidenceStatus = "measured" | "insufficientEvidence";

export interface FalsificationRunInput {
  label: string;
  benchmark: string;
  notes?: string | null;
}

export interface FalsificationRun extends FalsificationRunInput {
  id: string;
  createdAt: number;
}

export interface FalsificationMeasurementInput {
  domain: string;
  family: string;
  cohort: ProbeCohort;
  probeId: string;
  noveltyIdentity: string;
  repeatOf?: string | null;
  teacherMode: TeacherMode;
  teacherUsed: boolean;
  teacherCalls: number;
  rung: string;
  steps: number;
  candidates: number;
  traceSteps: number;
  cost: number;
  abstained: boolean;
  clarified: boolean;
  confidence?: number | null;
  groundingTier: GroundingTier;
  usedSkillId?: string | null;
  createdSkillId?: string | null;
  correct?: boolean | null;
  failureReason?: string | null;
  baselineTraceSteps?: number | null;
  regressionProbe?: boolean;
  attributionCorrect?: boolean | null;
  attributionCost?: number | null;
}

export interface FalsificationMeasurement extends FalsificationMeasurementInput {
  id: string;
  runId: string;
  recordedAt: number;
}

export interface Section38Metric {
  slot: number;
  name: string;
  status: MetricEvidenceStatus;
  sampleSize: number;
  value?: number | null;
  detail: string;
}

export interface Section38TelemetrySnapshot {
  runs: number;
  measurements: number;
  failures: number;
  abstentions: number;
  clarifications: number;
  teacherOffViolationsRejected: number;
  duplicateMeasurementsRejected: number;
  cohortLeakageRejected: number;
  metrics: Section38Metric[];
}

export interface RankingEvaluation {
  id: number;
  query: string;
  candidateLimit: number;
  trainingExamples: number;
  heldOutExamples: number;
  heldOutSuccesses: number;
  scoredSuccesses: number;
  baselineMeanRank?: number | null;
  learnedMeanRank?: number | null;
  baselineMeanReciprocalRank?: number | null;
  learnedMeanReciprocalRank?: number | null;
  learnedImprovesSearch: boolean;
  createdAt: number;
}

export interface RepresentationModel {
  id: number;
  modelVersion: string;
  trainingTasks: number;
  heldOutTasks: number;
  heldOutCoverage: number;
  activated: boolean;
  termWeights: Record<string, number>;
  createdAt: number;
}

export interface RepresentationRegressionEvaluation {
  id: number;
  modelId: number;
  heldOutQueries: number;
  heldOutSuccesses: number;
  baselineScoredSuccesses: number;
  candidateScoredSuccesses: number;
  baselineMeanRank?: number | null;
  candidateMeanRank?: number | null;
  preservesSearch: boolean;
  createdAt: number;
}

export interface SemanticRecallEvaluation {
  id: number;
  candidateLimit: number;
  trainingQueries: number;
  heldOutQueries: number;
  heldOutSuccesses: number;
  lexicalScoredSuccesses: number;
  semanticScoredSuccesses: number;
  semanticImprovesRecall: boolean;
  createdAt: number;
}

export type GoalKind = "task" | "standing" | "instrumental" | "learning";

export interface Goal {
  id: string;
  kind: GoalKind;
  statement: string;
  parentId?: string | null;
  immutable: boolean;
  createdAt: number;
}

export interface GoalLearningRecord {
  learningGoalId: string;
  standingGoalId: string;
  sourceGapId: string;
  derivationReason: string;
  createdAt: number;
}

export interface GoalDerivationRecord {
  goalId: string;
  parentGoalId: string;
  derivationReason: string;
  createdAt: number;
}

export type GapKind =
  | "structural"
  | "functional"
  | "repeated_impass"
  | "contradiction"
  | "failed_prediction"
  | "ungrounded";

export interface CuriosityGap {
  id: string;
  kind: GapKind;
  statement: string;
  blastRadius: number;
  goalRelevance: number;
  learningProgress: number;
  costToClose: number;
  valueScore: number;
  sourceEpisode?: string | null;
  resolved: boolean;
  createdAt: number;
}

export interface SkillCandidate {
  name: string;
  sourceEpisodeIds: string[];
  supportCount: number;
  rationale: string;
  failureCritic: boolean;
}

export type SkillLifecycle = "candidate" | "shadow" | "promoted" | "retired";

export interface ManagedSkill {
  id: string;
  candidate: SkillCandidate;
  lifecycle: SkillLifecycle;
  promotionVerdict?: JsonValue | null;
  shadowLiveWins: number;
  retirement?: JsonValue | null;
  createdAt: number;
  updatedAt: number;
}

export interface PromotionReplay {
  episode_id: string;
  incumbent_correct: boolean;
  challenger_correct: boolean;
  incumbent_trace_steps?: number | null;
  challenger_trace_steps?: number | null;
  incumbent_candidates_explored?: number | null;
  challenger_candidates_explored?: number | null;
  transfer: boolean;
}

export interface SkillShadowWinInput {
  skillId: string;
  observedResult: JsonValue;
  scope?: Record<string, JsonValue>;
  evaluation: {
    tier: "Hard" | "Consensus";
    success: true;
    details: string;
    surprise?: number | null;
  };
  verifierIdentity: string;
}

export interface EpisodeCompressionPlan {
  retainFull: string[];
  summarize: string[];
  forgottenAsKnownGap: string[];
}

export interface EpisodeCompressionResult {
  plan: EpisodeCompressionPlan;
  archivedEpisodeIds: string[];
}

export interface EpisodeCompressionRecord {
  episodeId: string;
  summary: JsonValue;
  archivedEpisode: JsonValue;
  createdAt: number;
}

export interface VerifiedAnswerRecord {
  episodeId: string;
  situation: string;
  environment: Record<string, JsonValue>;
  observedResult: JsonValue;
  tier: "Hard" | "Consensus";
  rung: string;
  createdAt: number;
}

export interface PrimitiveExecution {
  receipt: {
    primitive: string;
    effect: string;
    target: string;
    payloadDigest: string;
    bounds: { maxBytes: number; maxSteps: number; maxMillis: number };
    redacted: boolean;
    replayable: boolean;
  };
  output: JsonValue;
}

export interface CapabilityBundle {
  formatVersion: number;
  name: string;
  version: string;
  contentId: string;
  procedures: CapabilityProcedure[];
  dependencies: CapabilityDependency[];
  provenance: CapabilityProvenance;
  reconstruction: ReconstructionRecipe;
}

export interface ReconstructedCapability {
  contentId: string;
  name: string;
  dependencyOrder: CapabilityDependency[];
  procedures: CapabilityProcedure[];
  reconstruction: ReconstructionRecipe;
}

export interface CapabilityProcedure {
  id: string;
  name: string;
  version: number;
  primitive: string;
  inputSchema: JsonValue;
  outputSchema: JsonValue;
  contract: JsonValue;
  neutralMetadata: NeutralProcedureMetadata;
  permissions: CapabilityPermission[];
  effects: CapabilityEffect[];
  bounds: { maxBytes: number; maxSteps: number; maxMillis: number };
  dependencies: CapabilityDependencyReference[];
  tests: CapabilityTest[];
  provenance: CapabilityProvenance;
}

/** Public result for a server-configured capability effect. Host roots,
 * permission scopes, and raw request targets are deliberately absent. */
export interface CapabilityInvocationResult {
  contentId: string;
  procedureId: string;
  output: JsonValue;
  outputDigest: string;
  receipt: {
    primitive: string;
    effect: CapabilityEffect;
    payloadDigest: string;
    bounds: { maxBytes: number; maxSteps: number; maxMillis: number };
    redacted: true;
    replayable: boolean;
  };
  usage: { bytes: number; steps: number; millis: number };
  episodeId: string;
  redacted: true;
}

export interface FailureAnalysisInput {
  idempotencyKey?: string;
  episodeId: string;
  selectedFeedbackId?: string;
  candidates: Array<{
    suspect: { procedure: string; version: number; traceStep: number };
    priorScore: number;
    change: { description: string; replacement: JsonValue };
    mode: "deterministic" | "simulated";
  }>;
  budget: {
    topK: number;
    maxReplays: number;
    maxReplaySteps: number;
  };
}

export type AttributionMechanism =
  "contract_violation" | "statistical_suspicion" | "counterfactual_replay";

export interface AttributionSelector {
  suspect: {
    procedure: string;
    version: number;
    traceStep: number;
  };
  mechanism: AttributionMechanism;
}

export interface AdaptationEvidenceRef {
  episodeId: string;
  selectedFeedbackId?: string | null;
}

export type AdaptationTarget =
  | { kind: "unusual_input"; reason: string }
  | { kind: "assumption"; key: string; replacement: JsonValue }
  | {
      kind: "procedure_scope";
      procedureId: string;
      expectedVersion: number;
      condition: JsonValue;
      learnedFrom: string;
    }
  | {
      kind: "procedure_replacement";
      incumbentId: string;
      incumbentVersion: number;
      challenger: JsonValue;
    }
  | {
      kind: "concept_revision";
      conceptId: string;
      expectedVersion: number;
      revisedDescription: string;
    };

export interface AdaptationPlanInput {
  idempotencyKey: string;
  analysis: Omit<FailureAnalysisInput, "idempotencyKey">;
  attribution: AttributionSelector;
  evidence: AdaptationEvidenceRef[];
  target: AdaptationTarget;
  createdAt: number;
}

export interface ApplyAdaptationInput {
  planId: string;
  idempotencyKey: string;
  appliedAt: number;
}

export type AttributionConfidence =
  "inconclusive" | "low" | "medium" | "high" | "certain";

export interface CreditAttribution {
  suspect: AttributionSelector["suspect"];
  mechanism: AttributionMechanism;
  confidence: AttributionConfidence;
  score: number;
  decisive: boolean;
  evidence: JsonValue[];
  limitations: JsonValue[];
  provenance: {
    episodeIds: string[];
    details: string[];
  };
  attributionCost: number;
  totalCost: number;
  attributionCostRatio: number;
}

export interface AdaptationEvidenceGate {
  verifiedEpisodes: number;
  distinctSources: number;
  strongestTier?: "Hard" | "Consensus" | "Deferred" | null;
  challengerBeatsIncumbent: boolean;
  corroborated: boolean;
  offline: boolean;
}

export type AdaptationAction =
  | { kind: "record_only"; reason: string }
  | { kind: "fix_assumption"; key: string; replacement: JsonValue }
  | {
      kind: "narrow_scope";
      procedureId: string;
      expectedVersion: number;
      condition: JsonValue;
      learnedFrom: string;
    }
  | {
      kind: "replace_procedure";
      incumbentId: string;
      incumbentVersion: number;
      challenger: JsonValue;
    }
  | {
      kind: "revise_concept_offline";
      conceptId: string;
      expectedVersion: number;
      revisedDescription: string;
      supportingEpisodes: number;
    }
  | { kind: "schedule_test"; reason: string };

export type AdaptationKnowledgeRef =
  | { kind: "concept"; id: string }
  | { kind: "procedure"; id: string }
  | { kind: "relationship"; id: string };

export interface AdaptationReconciliationEntry {
  knowledge: AdaptationKnowledgeRef;
  depth: number;
  expectedVersion: number;
  previousLifecycle: string;
  nextLifecycle: string;
  outcome:
    "preserved_by_alternative_support" | "mark_stale" | "mark_under_review";
}

export interface AdaptationReconciliationPlan {
  changed: AdaptationKnowledgeRef;
  entries: AdaptationReconciliationEntry[];
}

export interface AdaptationPlan {
  id: string;
  idempotencyKey: string;
  analysisEpisodeId: string;
  attribution: CreditAttribution;
  evidence: AdaptationEvidenceRef[];
  evidenceGate: AdaptationEvidenceGate;
  target: AdaptationTarget;
  action: AdaptationAction;
  rationale: string;
  mutationScope: "online_narrow" | "offline_broad" | "no_graph_change";
  reconciliation?: AdaptationReconciliationPlan | null;
  createdAt: number;
}

export type AdaptationOutcome =
  | { kind: "no_graph_change" }
  | {
      kind: "procedure_updated";
      procedureId: string;
      previousVersion: number;
      currentVersion: number;
    }
  | { kind: "concept_updated"; conceptId: string };

export interface AdaptationReconciliationReceipt {
  updated: AdaptationKnowledgeRef[];
  preserved: AdaptationKnowledgeRef[];
}

export interface AdaptationReceipt {
  planId: string;
  idempotencyKey: string;
  outcome: AdaptationOutcome;
  reconciliation?: AdaptationReconciliationReceipt | null;
  evidence: AdaptationEvidenceRef[];
  appliedAt: number;
}

export interface AdaptationRecord {
  plan: AdaptationPlan;
  receipt?: AdaptationReceipt | null;
}

export interface ClaimImplication {
  predicate: string;
  value: JsonValue;
}

export interface ClaimScopeAssignment {
  feature: string;
  value: JsonValue;
  learnedFrom: string;
}

export interface ContradictionClaim {
  id: string;
  statement: string;
  implication: ClaimImplication;
  supportingEpisodes: string[];
  scope: ClaimScopeAssignment[];
}

export interface DemonstratedFeature {
  feature: string;
  leftValue: JsonValue;
  leftEpisode: string;
  rightValue: JsonValue;
  rightEpisode: string;
}

export interface ContradictionRefinement {
  left: ContradictionClaim;
  right: ContradictionClaim;
  discriminator: DemonstratedFeature;
}

export interface Contradiction {
  id: number;
  left: ContradictionClaim;
  right: ContradictionClaim;
  status: "Held" | "Refined";
  refinement?: ContradictionRefinement | null;
  createdAt: number;
  updatedAt: number;
}

export interface RecordContradictionInput {
  left: ContradictionClaim;
  right: ContradictionClaim;
  createdAt: number;
}

export interface RefineContradictionInput {
  contradictionId: number;
  discriminator: DemonstratedFeature;
  updatedAt: number;
}

export type ClaimUncertainty =
  | { status: "certain" }
  | { status: "held_contradictions"; contradictionIds: number[] };

export interface RpcTransport {
  request<T>(method: string, params: unknown): Promise<T>;
  close?(): void;
}

export interface CycleAssumption {
  description: string;
  basis: string;
  concept?: string | null;
}

export interface CycleBudget {
  maxExecSteps: number;
  maxContextItems: number;
  maxTeacherTurns: number;
}

export interface CycleInput {
  situation: string;
  workingDirectory?: string;
  environment: Record<string, JsonValue>;
  assumptions: CycleAssumption[];
  budget: CycleBudget;
  teacherAllowed: boolean;
  sessionId?: string;
  recallMode?: "global" | "session" | "none";
  permissionMode?: "ask" | "workspace" | "full-access";
}

export type SessionVisibility = "global" | "isolated";
export type SessionState = "active" | "ended";
export type RecallMode = "global" | "session" | "none";
export type PermissionMode = "ask" | "workspace" | "full-access";

export interface Session {
  id: string;
  name?: string | null;
  visibility: SessionVisibility;
  state: SessionState;
  createdAt: number;
  endedAt?: number | null;
}

export interface TeacherRequestWire {
  situation: string;
  context: Record<string, JsonValue>;
  specificQuestion?: string;
  desiredOutput: Record<string, JsonValue>;
}

export interface ProposalProvenanceWire {
  provider: "claude" | "codex" | "cli" | "openai" | "ollama" | "human";
  teacher: string;
  model?: string;
  requestId: string;
  requestHash?: string;
  providerRequestId?: string;
  generatedAt: string;
  situation: string;
  specificQuestion?: string;
}

export interface TeacherProposalWire {
  content: JsonValue;
  source: string;
  status: "unverified";
  provenance: ProposalProvenanceWire;
  validation?: {
    status: "verified" | "rejected" | "provisional";
    validatedAt: string;
    checks: Array<{
      validator: string;
      status: "verified" | "rejected" | "provisional";
      reason: string;
      evidence?: JsonValue;
    }>;
  };
}

export interface NeedTeacherProgress {
  status: "need_teacher";
  cycleId: string;
  request: TeacherRequestWire;
}

export interface CompletedCycleProgress {
  status: "completed";
  cycleId: string;
  disposition: "verified" | "provisional" | "abstained";
  answer: JsonValue;
  episode: Record<string, JsonValue>;
}

export type CycleProgress = NeedTeacherProgress | CompletedCycleProgress;
