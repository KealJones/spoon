import assert from "node:assert/strict";
import test from "node:test";

import { EkgClient, type RpcTransport } from "../src/index.js";

class RecordingTransport implements RpcTransport {
  calls: Array<{ method: string; params: unknown }> = [];

  async request<T>(method: string, params: unknown): Promise<T> {
    this.calls.push({ method, params });
    return { ok: true } as T;
  }
}

test("client maps every concept RPC method with exact params", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport);
  const updatedConcept = {
    id: "concept-1",
    name: "DOUBLE",
    description: "Multiply a number by two",
    mutability: "Definitional",
    version: 2,
  };

  await client.createConcept({
    name: "DOUBLE",
    description: "Multiply a number by two",
    mutability: "Definitional",
  });
  await client.getConcept("concept-1");
  await client.getConceptByName("DOUBLE");
  await client.listConcepts();
  await client.updateConcept(updatedConcept);
  await client.deleteConcept("concept-1");

  assert.deepEqual(transport.calls, [
    {
      method: "concept.create",
      params: {
        name: "DOUBLE",
        description: "Multiply a number by two",
        mutability: "Definitional",
      },
    },
    { method: "concept.get", params: { conceptId: "concept-1" } },
    { method: "concept.findByName", params: { name: "DOUBLE" } },
    { method: "concept.list", params: {} },
    { method: "concept.update", params: updatedConcept },
    { method: "concept.delete", params: { conceptId: "concept-1" } },
  ]);
});

test("client maps every relationship and traversal RPC method with exact params", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport);
  const createdRelationship = {
    source: "concept-1",
    target: "concept-2",
    kind: "implemented-by",
    strength: 0.9,
  };
  const updatedRelationship = {
    id: "relationship-1",
    ...createdRelationship,
    version: 2,
  };

  await client.createRelationship(createdRelationship);
  await client.getRelationship("relationship-1");
  await client.updateRelationship(updatedRelationship);
  await client.deleteRelationship("relationship-1");
  await client.traverse("concept-1", "implemented-by", 2);

  assert.deepEqual(transport.calls, [
    { method: "relationship.create", params: createdRelationship },
    {
      method: "relationship.get",
      params: { relationshipId: "relationship-1" },
    },
    { method: "relationship.update", params: updatedRelationship },
    {
      method: "relationship.delete",
      params: { relationshipId: "relationship-1" },
    },
    {
      method: "graph.traverse",
      params: { conceptId: "concept-1", kind: "implemented-by", maxHops: 2 },
    },
  ]);
});

test("client maps every procedure RPC method with exact params", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport);
  const createdProcedure = {
    name: "DOUBLE",
    params: [{ name: "x", description: null }],
    body: { Var: "x" },
    conceptId: "concept-1",
  };
  const updatedProcedure = {
    id: "procedure-1",
    ...createdProcedure,
    version: 2,
  };

  await client.createProcedure(createdProcedure);
  await client.getProcedure("procedure-1");
  await client.getProcedureByName("DOUBLE");
  await client.listProcedures();
  await client.updateProcedure(updatedProcedure);
  await client.deleteProcedure("procedure-1");
  await client.executeProcedure("procedure-1", { x: 7 }, 14);
  await client.executeProcedure("procedure-1", { x: 8 });

  assert.deepEqual(transport.calls, [
    { method: "procedure.create", params: createdProcedure },
    { method: "procedure.get", params: { procedureId: "procedure-1" } },
    { method: "procedure.findByName", params: { name: "DOUBLE" } },
    { method: "procedure.list", params: {} },
    { method: "procedure.update", params: updatedProcedure },
    { method: "procedure.delete", params: { procedureId: "procedure-1" } },
    {
      method: "procedure.execute",
      params: { procedureId: "procedure-1", inputs: { x: 7 }, prediction: 14 },
    },
    {
      method: "procedure.execute",
      params: { procedureId: "procedure-1", inputs: { x: 8 } },
    },
  ]);
});

test("client maps every episode RPC method with exact params", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport);

  await client.getEpisode("episode-1");
  await client.listEpisodes({
    since: 1_700_000_000,
    until: 1_800_000_000,
    outcome: "success",
    rung: "Act",
    conceptId: "concept-1",
    limit: 10,
  });
  await client.replayEpisode("episode-1", { x: 9 });

  assert.deepEqual(transport.calls, [
    { method: "episode.get", params: { episodeId: "episode-1" } },
    {
      method: "episode.list",
      params: {
        since: 1_700_000_000,
        until: 1_800_000_000,
        outcome: "success",
        rung: "Act",
        conceptId: "concept-1",
        limit: 10,
      },
    },
    {
      method: "episode.replay",
      params: { episodeId: "episode-1", substitutions: { x: 9 } },
    },
  ]);
});

test("client maps cycle begin, resume, and abort with exact camelCase params", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport);
  const proposal = {
    content: { interpretations: [], answer: 42 },
    source: "human:test",
    status: "unverified" as const,
    provenance: {
      provider: "human" as const,
      teacher: "human:test",
      requestId: "request-1",
      generatedAt: "2026-08-22T00:00:00.000Z",
      situation: "what is the answer?",
    },
  };

  await client.beginCycle({
    situation: "what is the answer?",
    environment: {},
    assumptions: [],
    budget: {
      maxExecSteps: 1_000,
      maxContextItems: 32,
      maxTeacherTurns: 1,
    },
    teacherAllowed: true,
  });
  await client.resumeCycle("cycle-1", proposal);
  await client.abortCycle("cycle-2", "provider unavailable");

  assert.deepEqual(transport.calls.slice(-3), [
    {
      method: "cycle.begin",
      params: {
        situation: "what is the answer?",
        environment: {},
        assumptions: [],
        budget: {
          maxExecSteps: 1_000,
          maxContextItems: 32,
          maxTeacherTurns: 1,
        },
        teacherAllowed: true,
      },
    },
    {
      method: "cycle.resume",
      params: { cycleId: "cycle-1", proposal },
    },
    {
      method: "cycle.abort",
      params: { cycleId: "cycle-2", reason: "provider unavailable" },
    },
  ]);
});
