import type {
  ConceptInput,
  EpisodeFilter,
  JsonValue,
  RpcTransport,
} from "./types.js";

export class EkgClient {
  constructor(private readonly transport: RpcTransport) {}

  createConcept<T = unknown>(concept: ConceptInput): Promise<T> {
    return this.transport.request<T>("concept.create", concept);
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
    return this.transport.request<T>("concept.update", concept);
  }

  deleteConcept<T = unknown>(conceptId: string): Promise<T> {
    return this.transport.request<T>("concept.delete", { conceptId });
  }

  createRelationship<T = unknown>(
    relationship: Record<string, JsonValue>,
  ): Promise<T> {
    return this.transport.request<T>("relationship.create", relationship);
  }

  getRelationship<T = unknown>(relationshipId: string): Promise<T> {
    return this.transport.request<T>("relationship.get", { relationshipId });
  }

  updateRelationship<T = unknown>(
    relationship: Record<string, JsonValue>,
  ): Promise<T> {
    return this.transport.request<T>("relationship.update", relationship);
  }

  deleteRelationship<T = unknown>(relationshipId: string): Promise<T> {
    return this.transport.request<T>("relationship.delete", { relationshipId });
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
    return this.transport.request<T>("procedure.create", procedure);
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
    return this.transport.request<T>("procedure.update", procedure);
  }

  deleteProcedure<T = unknown>(procedureId: string): Promise<T> {
    return this.transport.request<T>("procedure.delete", { procedureId });
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

  close(): void {
    this.transport.close?.();
  }
}
