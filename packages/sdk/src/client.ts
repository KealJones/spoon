import type {
  AdaptationPlan,
  AdaptationPlanInput,
  AdaptationReceipt,
  AdaptationRecord,
  ApplyAdaptationInput,
  ConceptInput,
  ClaimUncertainty,
  Contradiction,
  ContradictionRefinement,
  CycleInput,
  CycleProgress,
  EpisodeFilter,
  EpisodeFeedback,
  EkgClientOptions,
  FeedbackInput,
  FailureAnalysisInput,
  JsonValue,
  RecordContradictionInput,
  RefineContradictionInput,
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
