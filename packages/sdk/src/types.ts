export type JsonValue =
  null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

export interface ConceptInput {
  name: string;
  description?: string;
  [key: string]: JsonValue | undefined;
}

export interface EpisodeFilter {
  since?: number;
  until?: number;
  limit?: number;
  outcome?: "success" | "failure" | "any";
  rung?: string;
  conceptId?: string;
}

export interface FeedbackInput {
  episodeId: string;
  observedResult: JsonValue;
  idempotencyKey: string;
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

export interface EkgClientOptions {
  adminToken?: string;
}

export type CapabilityPermission =
  | { kind: "network_host"; host: string }
  | { kind: "file_read_prefix"; path_prefix: string }
  | { kind: "file_write_prefix"; path_prefix: string }
  | { kind: "observe_target"; target: string }
  | { kind: "sandbox_profile"; profile: string };

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
  rungDistribution: Array<[string, number]>;
  intuition: {
    indexedDocuments: number;
    invertedTermRows: number;
    retrievalQueries: number;
    candidatesExamined: number;
    rankingExamples: number;
    supervisionTasks: number;
    groundedTasks: number;
  };
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

export interface EpisodeCompressionPlan {
  retainFull: string[];
  summarize: string[];
  forgottenAsKnownGap: string[];
}

export interface CapabilityBundle {
  formatVersion: number;
  name: string;
  version: string;
  contentId: string;
  procedures: JsonValue[];
  dependencies: JsonValue[];
  provenance: JsonValue;
  reconstruction: JsonValue;
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
  environment: Record<string, JsonValue>;
  assumptions: CycleAssumption[];
  budget: CycleBudget;
  teacherAllowed: boolean;
}

export interface TeacherRequestWire {
  situation: string;
  context: Record<string, JsonValue>;
  specificQuestion?: string;
  desiredOutput: Record<string, JsonValue>;
}

export interface ProposalProvenanceWire {
  provider: "claude" | "codex" | "openai" | "ollama" | "human";
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
