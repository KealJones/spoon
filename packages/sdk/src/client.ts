import type {
  AdaptationPlan,
  AdaptationPlanInput,
  AdaptationReceipt,
  AdaptationRecord,
  ApplyAdaptationInput,
  AuthenticatedObservationInput,
  ConceptInput,
  ClaimUncertainty,
  Contradiction,
  ContradictionRefinement,
  CycleInput,
  CycleProgress,
  CapabilityBundle,
  CapabilityProcedure,
  CapabilityPermission,
  CuriosityGap,
  EpisodeCompressionPlan,
  EpisodeCompressionRecord,
  EpisodeCompressionResult,
  VerifiedAnswerRecord,
  DiscoveredOperation,
  EpisodeFilter,
  EpisodeFeedback,
  EkgClientOptions,
  FeedbackInput,
  FailureAnalysisInput,
  JsonValue,
  ImportedCapability,
  InterfaceDescription,
  LocalValidation,
  MetricsSnapshot,
  ManagedSkill,
  PrimitiveExecution,
  PromotionReplay,
  Goal,
  GoalKind,
  GoalLearningRecord,
  GoalDerivationRecord,
  SkillCandidate,
  SkillShadowWinInput,
  RecordContradictionInput,
  RefineContradictionInput,
  RankingEvaluation,
  RepresentationModel,
  ReconstructedCapability,
  SemanticRecallEvaluation,
  RpcTransport,
  TeacherProposalWire,
} from "./types.js";

export class EkgClient {
  constructor(
    private readonly transport: RpcTransport,
    private readonly options: EkgClientOptions = {},
  ) {}

  createConcept<T = unknown>(concept: ConceptInput): Promise<T> {
    return this.transport.request<T>(
      "concept.create",
      this.withAdminToken(concept),
    );
  }

  getConcept<T = unknown>(conceptId: string): Promise<T> {
    return this.transport.request<T>("concept.get", { conceptId });
  }

  getConceptByName<T = unknown>(name: string): Promise<T> {
    return this.transport.request<T>("concept.findByName", { name });
  }

  listConcepts<T = unknown[]>(): Promise<T> {
    return this.transport.request<T>("concept.list", {});
  }

  updateConcept<T = unknown>(concept: Record<string, JsonValue>): Promise<T> {
    return this.transport.request<T>(
      "concept.update",
      this.withAdminToken(concept),
    );
  }

  deleteConcept<T = unknown>(conceptId: string): Promise<T> {
    return this.transport.request<T>(
      "concept.delete",
      this.withAdminToken({ conceptId }),
    );
  }

  createRelationship<T = unknown>(
    relationship: Record<string, JsonValue>,
  ): Promise<T> {
    return this.transport.request<T>(
      "relationship.create",
      this.withAdminToken(relationship),
    );
  }

  getRelationship<T = unknown>(relationshipId: string): Promise<T> {
    return this.transport.request<T>("relationship.get", { relationshipId });
  }

  updateRelationship<T = unknown>(
    relationship: Record<string, JsonValue>,
  ): Promise<T> {
    return this.transport.request<T>(
      "relationship.update",
      this.withAdminToken(relationship),
    );
  }

  deleteRelationship<T = unknown>(relationshipId: string): Promise<T> {
    return this.transport.request<T>(
      "relationship.delete",
      this.withAdminToken({ relationshipId }),
    );
  }

  traverse<T = unknown[]>(
    conceptId: string,
    kind: string,
    maxHops = 1,
  ): Promise<T> {
    return this.transport.request<T>("graph.traverse", {
      conceptId,
      kind,
      maxHops,
    });
  }

  createProcedure<T = unknown>(
    procedure: Record<string, JsonValue>,
  ): Promise<T> {
    return this.transport.request<T>(
      "procedure.create",
      this.withAdminToken(procedure),
    );
  }

  getProcedure<T = unknown>(procedureId: string): Promise<T> {
    return this.transport.request<T>("procedure.get", { procedureId });
  }

  getProcedureByName<T = unknown>(name: string): Promise<T> {
    return this.transport.request<T>("procedure.findByName", { name });
  }

  listProcedures<T = unknown[]>(): Promise<T> {
    return this.transport.request<T>("procedure.list", {});
  }

  updateProcedure<T = unknown>(
    procedure: Record<string, JsonValue>,
  ): Promise<T> {
    return this.transport.request<T>(
      "procedure.update",
      this.withAdminToken(procedure),
    );
  }

  deleteProcedure<T = unknown>(procedureId: string): Promise<T> {
    return this.transport.request<T>(
      "procedure.delete",
      this.withAdminToken({ procedureId }),
    );
  }

  executeProcedure<T = unknown>(
    procedureId: string,
    inputs: Record<string, JsonValue>,
    prediction?: JsonValue,
  ): Promise<T> {
    const params: {
      procedureId: string;
      inputs: Record<string, JsonValue>;
      prediction?: JsonValue;
    } = {
      procedureId,
      inputs,
    };
    if (prediction !== undefined) params.prediction = prediction;
    return this.transport.request<T>("procedure.execute", params);
  }

  listEpisodes<T = unknown[]>(filter: EpisodeFilter = {}): Promise<T> {
    return this.transport.request<T>("episode.list", filter);
  }

  getEpisode<T = unknown>(episodeId: string): Promise<T> {
    return this.transport.request<T>("episode.get", { episodeId });
  }

  replayEpisode<T = unknown>(
    episodeId: string,
    substitutions: Record<string, JsonValue>,
  ): Promise<T> {
    return this.transport.request<T>("episode.replay", {
      episodeId,
      substitutions,
    });
  }

  recordFeedback(input: FeedbackInput): Promise<EpisodeFeedback> {
    return this.transport.request<EpisodeFeedback>("feedback.record", input);
  }

  recordAuthenticatedObservation<T = JsonValue>(
    input: AuthenticatedObservationInput,
  ): Promise<T> {
    return this.transport.request<T>(
      "observation.recordAuthenticated",
      this.withAdminToken(input),
    );
  }

  analyzeFailure<T = Record<string, JsonValue>>(
    input: FailureAnalysisInput,
  ): Promise<T> {
    return this.transport.request<T>("credit.analyze", input);
  }

  getFailureAnalysis<T = Record<string, JsonValue>>(
    analysisId: string,
  ): Promise<T | null> {
    return this.transport.request<T | null>("credit.get", { analysisId });
  }

  getFailureAnalysisByKey<T = Record<string, JsonValue>>(
    idempotencyKey: string,
  ): Promise<T | null> {
    return this.transport.request<T | null>("credit.getByKey", {
      idempotencyKey,
    });
  }

  planAdaptation(input: AdaptationPlanInput): Promise<AdaptationPlan> {
    return this.transport.request<AdaptationPlan>("adaptation.plan", input);
  }

  getAdaptation(planId: string): Promise<AdaptationRecord | null> {
    return this.transport.request<AdaptationRecord | null>("adaptation.get", {
      planId,
    });
  }

  applyAdaptation(input: ApplyAdaptationInput): Promise<AdaptationReceipt> {
    return this.transport.request<AdaptationReceipt>("adaptation.apply", input);
  }

  applyAdaptationOffline(
    input: ApplyAdaptationInput,
  ): Promise<AdaptationReceipt> {
    return this.transport.request<AdaptationReceipt>(
      "adaptation.applyOffline",
      this.withAdminToken(input),
    );
  }

  listContradictions(): Promise<Contradiction[]> {
    return this.transport.request<Contradiction[]>("contradiction.list", {});
  }

  getContradiction(contradictionId: number): Promise<Contradiction | null> {
    return this.transport.request<Contradiction | null>("contradiction.get", {
      contradictionId,
    });
  }

  recordContradiction(input: RecordContradictionInput): Promise<Contradiction> {
    return this.transport.request<Contradiction>(
      "contradiction.record",
      this.withAdminToken(input),
    );
  }

  refineContradiction(
    input: RefineContradictionInput,
  ): Promise<ContradictionRefinement> {
    return this.transport.request<ContradictionRefinement>(
      "contradiction.refine",
      this.withAdminToken(input),
    );
  }

  getClaimUncertainty(claimId: string): Promise<ClaimUncertainty> {
    return this.transport.request<ClaimUncertainty>(
      "contradiction.uncertainty",
      { claimId },
    );
  }

  discoverCapability(description: InterfaceDescription): Promise<CapabilityBundle> {
    return this.transport.request<CapabilityBundle>(
      "capability.discover",
      description,
    );
  }

  importCapability(bundle: CapabilityBundle): Promise<ImportedCapability> {
    return this.transport.request<ImportedCapability>("capability.import", {
      bundle,
    });
  }

  importAndRevalidateCapability(
    bundle: CapabilityBundle,
    validation: LocalValidation,
  ): Promise<ImportedCapability> {
    return this.transport.request<ImportedCapability>(
      "capability.importAndRevalidate",
      { bundle, validation },
    );
  }

  reconstructCapability(contentId: string): Promise<ReconstructedCapability> {
    return this.transport.request<ReconstructedCapability>(
      "capability.reconstruct",
      { contentId },
    );
  }

  exportCapability(contentId: string): Promise<{ bundle: CapabilityBundle }> {
    return this.transport.request<{ bundle: CapabilityBundle }>(
      "capability.export",
      { contentId },
    );
  }

  revalidateCapability(
    contentId: string,
    validation: LocalValidation,
  ): Promise<ImportedCapability> {
    return this.transport.request<ImportedCapability>("capability.revalidate", {
      contentId,
      validation,
    });
  }

  grantCapability(
    contentId: string,
    permission: CapabilityPermission,
  ): Promise<{ granted: boolean }> {
    return this.transport.request<{ granted: boolean }>("capability.grant", {
      contentId,
      permission,
      adminToken: this.options.adminToken,
    });
  }

  revokeCapability(
    contentId: string,
    permission: CapabilityPermission,
  ): Promise<{ revoked: boolean }> {
    return this.transport.request<{ revoked: boolean }>("capability.revoke", {
      contentId,
      permission,
      adminToken: this.options.adminToken,
    });
  }

  authorizeCapabilityProcedure(
    contentId: string,
    procedureId: string,
  ): Promise<CapabilityProcedure> {
    return this.transport.request<CapabilityProcedure>(
      "capability.authorizeProcedure",
      { contentId, procedureId },
    );
  }

  metricsSnapshot(): Promise<MetricsSnapshot> {
    return this.transport.request<MetricsSnapshot>("metrics.snapshot", {});
  }

  evaluateRecallRanking(
    query: string,
    candidateLimit: number,
    holdoutExamples: number,
  ): Promise<RankingEvaluation> {
    return this.transport.request<RankingEvaluation>(
      "intuition.evaluateRanking",
      { query, candidateLimit, holdoutExamples },
    );
  }

  trainRepresentationModel(holdoutTasks: number): Promise<RepresentationModel> {
    return this.transport.request<RepresentationModel>(
      "intuition.trainRepresentation",
      { holdoutTasks },
    );
  }

  latestRepresentationModel(): Promise<RepresentationModel | null> {
    return this.transport.request<RepresentationModel | null>(
      "intuition.latestRepresentation",
      {},
    );
  }

  activateRepresentationModel(modelId: number): Promise<RepresentationModel> {
    return this.transport.request<RepresentationModel>(
      "intuition.activateRepresentation",
      this.withAdminToken({ modelId }),
    );
  }

  evaluateSemanticRecall(
    candidateLimit: number,
    holdoutQueries: number,
  ): Promise<SemanticRecallEvaluation> {
    return this.transport.request<SemanticRecallEvaluation>(
      "intuition.evaluateSemanticRecall",
      { candidateLimit, holdoutQueries },
    );
  }

  createGoal(
    kind: GoalKind,
    statement: string,
    parentId?: string,
  ): Promise<Goal> {
    return this.transport.request<Goal>("goal.create", {
      kind,
      statement,
      ...(parentId === undefined ? {} : { parentId }),
    });
  }

  listGoals(): Promise<Goal[]> {
    return this.transport.request<Goal[]>("goal.list", {});
  }

  createLearningGoal(
    statement: string,
    standingGoalId: string,
    sourceGapId: string,
    derivationReason: string,
  ): Promise<Goal> {
    return this.transport.request<Goal>("goal.createLearning", {
      statement,
      standingGoalId,
      sourceGapId,
      derivationReason,
    });
  }

  listLearningGoalRecords(): Promise<GoalLearningRecord[]> {
    return this.transport.request<GoalLearningRecord[]>("goal.learningRecords", {});
  }

  createInstrumentalGoal(
    statement: string,
    parentGoalId: string,
    derivationReason: string,
  ): Promise<Goal> {
    return this.transport.request<Goal>("goal.createInstrumental", {
      statement,
      parentGoalId,
      derivationReason,
    });
  }

  listGoalDerivationRecords(): Promise<GoalDerivationRecord[]> {
    return this.transport.request<GoalDerivationRecord[]>("goal.derivationRecords", {});
  }

  recordCuriosityGap(gap: CuriosityGap): Promise<{ recorded: boolean }> {
    return this.transport.request<{ recorded: boolean }>("curiosity.record", gap);
  }

  rankCuriosityGaps(limit = 32): Promise<CuriosityGap[]> {
    return this.transport.request<CuriosityGap[]>("curiosity.rank", { limit });
  }

  discoverSkillCandidates(limit = 128): Promise<SkillCandidate[]> {
    return this.transport.request<SkillCandidate[]>("consolidation.discover", {
      limit,
    });
  }

  episodeCompressionPlan(limit = 128): Promise<EpisodeCompressionPlan> {
    return this.transport.request<EpisodeCompressionPlan>(
      "consolidation.compressionPlan",
      { limit },
    );
  }

  compressEpisodeHistory(limit = 128): Promise<EpisodeCompressionResult> {
    return this.transport.request<EpisodeCompressionResult>(
      "consolidation.compress",
      this.withAdminToken({ limit }),
    );
  }

  listEpisodeCompressionRecords(limit = 128): Promise<EpisodeCompressionRecord[]> {
    return this.transport.request<EpisodeCompressionRecord[]>(
      "consolidation.compressedList",
      { limit },
    );
  }

  listVerifiedAnswers(limit = 128): Promise<VerifiedAnswerRecord[]> {
    return this.transport.request<VerifiedAnswerRecord[]>("regression.list", {
      limit,
    });
  }

  registerSkillCandidate(candidate: SkillCandidate): Promise<ManagedSkill> {
    return this.transport.request<ManagedSkill>(
      "consolidation.register",
      this.withAdminToken(candidate),
    );
  }

  listManagedSkills(limit = 128): Promise<ManagedSkill[]> {
    return this.transport.request<ManagedSkill[]>("consolidation.list", {
      limit,
    });
  }

  listActiveManagedSkills(limit = 128): Promise<ManagedSkill[]> {
    return this.transport.request<ManagedSkill[]>("consolidation.listActive", {
      limit,
    });
  }

  rankActiveManagedSkills(query: string, limit = 128): Promise<ManagedSkill[]> {
    return this.transport.request<ManagedSkill[]>("consolidation.rankActive", { query, limit });
  }

  executeBestManagedSkill<T = unknown>(
    query: string,
    inputs: Record<string, JsonValue> = {},
    prediction?: JsonValue,
  ): Promise<T> {
    return this.transport.request<T>("consolidation.executeBest", {
      query, inputs, ...(prediction === undefined ? {} : { prediction }),
    });
  }

  executeManagedSkill<T = unknown>(
    skillId: string,
    inputs: Record<string, JsonValue> = {},
    prediction?: JsonValue,
  ): Promise<T> {
    return this.transport.request<T>("consolidation.execute", {
      skillId,
      inputs,
      ...(prediction === undefined ? {} : { prediction }),
    });
  }

  registerSingleSuccessSkill(episodeId: string): Promise<ManagedSkill> {
    return this.transport.request<ManagedSkill>(
      "consolidation.registerSingle",
      this.withAdminToken({ episodeId }),
    );
  }

  registerFailureCriticSkill(episodeId: string): Promise<ManagedSkill> {
    return this.transport.request<ManagedSkill>(
      "consolidation.registerFailureCritic",
      this.withAdminToken({ episodeId }),
    );
  }

  evaluateSkillForShadow(
    skillId: string,
    replays: PromotionReplay[],
  ): Promise<ManagedSkill> {
    return this.transport.request<ManagedSkill>(
      "consolidation.evaluateShadow",
      this.withAdminToken({ skillId, replays }),
    );
  }

  promoteSkillFromLiveWin(input: SkillShadowWinInput): Promise<ManagedSkill> {
    return this.transport.request<ManagedSkill>(
      "consolidation.promoteLive",
      this.withAdminToken(input),
    );
  }

  retireManagedSkill(
    skillId: string,
    successorSkill: string,
    reason: string,
  ): Promise<ManagedSkill> {
    return this.transport.request<ManagedSkill>(
      "consolidation.retire",
      this.withAdminToken({ skillId, successorSkill, reason }),
    );
  }

  observePrimitive(target: string): Promise<PrimitiveExecution> {
    return this.transport.request<PrimitiveExecution>("primitive.observe", {
      target,
    });
  }

  beginCycle(input: CycleInput): Promise<CycleProgress> {
    return this.transport.request<CycleProgress>("cycle.begin", input);
  }

  resumeCycle(
    cycleId: string,
    proposal: TeacherProposalWire,
  ): Promise<CycleProgress> {
    return this.transport.request<CycleProgress>("cycle.resume", {
      cycleId,
      proposal,
    });
  }

  abortCycle(cycleId: string, reason: string): Promise<CycleProgress> {
    return this.transport.request<CycleProgress>("cycle.abort", {
      cycleId,
      reason,
    });
  }

  close(): void {
    this.transport.close?.();
  }

  private withAdminToken<T extends object>(
    params: T,
  ): T & {
    adminToken?: string;
  } {
    const adminToken = this.options.adminToken?.trim();
    return adminToken ? { ...params, adminToken } : params;
  }
}
