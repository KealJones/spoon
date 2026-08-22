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
  await client.recordFeedback({
    episodeId: "episode-1",
    observedResult: "flat pancakes",
    idempotencyKey: "flat-feedback-1",
  });
  const analysis = {
    episodeId: "episode-1",
    selectedFeedbackId: "feedback-1",
    candidates: [
      {
        suspect: { procedure: "procedure-1", version: 1, traceStep: 0 },
        priorScore: 0.8,
        change: {
          description: "replace multiplier",
          replacement: { kind: "replace_body" },
        },
        mode: "deterministic" as const,
      },
    ],
    budget: { topK: 1, maxReplays: 1, maxReplaySteps: 100 },
  };
  await client.analyzeFailure(analysis);

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
    {
      method: "feedback.record",
      params: {
        episodeId: "episode-1",
        observedResult: "flat pancakes",
        idempotencyKey: "flat-feedback-1",
      },
    },
    { method: "credit.analyze", params: analysis },
  ]);
});

test("client maps durable credit reads and explicit retry keys", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport);
  const analysis = {
    episodeId: "episode-1",
    candidates: [],
    budget: { topK: 0, maxReplays: 0, maxReplaySteps: 0 },
    idempotencyKey: "analysis-retry-1",
  };

  await client.analyzeFailure(analysis);
  await client.getFailureAnalysis("analysis-1");
  await client.getFailureAnalysisByKey("analysis-retry-1");

  assert.deepEqual(transport.calls, [
    { method: "credit.analyze", params: analysis },
    { method: "credit.get", params: { analysisId: "analysis-1" } },
    {
      method: "credit.getByKey",
      params: { idempotencyKey: "analysis-retry-1" },
    },
  ]);
});

test("client maps capability discovery, quarantine, validation, and local grants", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport, { adminToken: "admin" });
  const description = {
    source: "weather-api",
    fingerprint: "weather-v1",
    operations: [
      {
        name: "forecast",
        inputSchema: { type: "object" },
        outputSchema: { type: "object" },
        host: "api.example.test",
        method: "GET",
        responseFixture: { temperature: 72 },
      },
    ],
  };
  const bundle = await client.discoverCapability(description);
  await client.importCapability(bundle);
  await client.revalidateCapability("cap-1", {
    passed: true,
    validationEpisodes: ["episode-1"],
    environmentDigest: "local",
  });
  await client.grantCapability("cap-1", {
    kind: "network_host",
    host: "api.example.test",
  });
  await client.revokeCapability("cap-1", {
    kind: "network_host",
    host: "api.example.test",
  });

  assert.deepEqual(transport.calls, [
    { method: "capability.discover", params: description },
    { method: "capability.import", params: { bundle } },
    {
      method: "capability.revalidate",
      params: {
        contentId: "cap-1",
        validation: {
          passed: true,
          validationEpisodes: ["episode-1"],
          environmentDigest: "local",
        },
      },
    },
    {
      method: "capability.grant",
      params: {
        contentId: "cap-1",
        permission: { kind: "network_host", host: "api.example.test" },
        adminToken: "admin",
      },
    },
    {
      method: "capability.revoke",
      params: {
        contentId: "cap-1",
        permission: { kind: "network_host", host: "api.example.test" },
        adminToken: "admin",
      },
    },
  ]);
});

test("client maps native primitive observation", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport);
  await client.observePrimitive("clock");
  assert.deepEqual(transport.calls, [
    { method: "primitive.observe", params: { target: "clock" } },
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

test("client maps adaptation plan, get, apply, and receipt RPCs with exact params", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport);
  const planInput = {
    idempotencyKey: "flat-pancake-plan-1",
    analysis: {
      episodeId: "episode-1",
      candidates: [],
      budget: { topK: 1, maxReplays: 0, maxReplaySteps: 0 },
    },
    attribution: {
      suspect: { procedure: "procedure-1", version: 1, traceStep: 0 },
      mechanism: "contract_violation" as const,
    },
    evidence: [{ episodeId: "episode-1" }],
    target: {
      kind: "procedure_scope" as const,
      procedureId: "procedure-1",
      expectedVersion: 1,
      condition: {
        description: "batter contains active leavening",
        check: { Var: "has_active_leavening" },
      },
      learnedFrom: "episode-1",
    },
    createdAt: 1_800_000_000,
  };

  await client.planAdaptation(planInput);
  await client.getAdaptation("plan-1");
  await client.applyAdaptation({
    planId: "plan-1",
    idempotencyKey: "flat-pancake-apply-1",
    appliedAt: 1_800_000_001,
  });

  assert.deepEqual(transport.calls, [
    { method: "adaptation.plan", params: planInput },
    { method: "adaptation.get", params: { planId: "plan-1" } },
    {
      method: "adaptation.apply",
      params: {
        planId: "plan-1",
        idempotencyKey: "flat-pancake-apply-1",
        appliedAt: 1_800_000_001,
      },
    },
  ]);
});

test("client maps contradiction RPCs with exact params", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport);

  await client.listContradictions();
  await client.getContradiction(7);
  const record = {
    left: {
      id: "left",
      statement: "pancakes rise",
      implication: { predicate: "pancakes-rise", value: true },
      supportingEpisodes: ["episode-1"],
      scope: [],
    },
    right: {
      id: "right",
      statement: "pancakes do not rise",
      implication: { predicate: "pancakes-rise", value: false },
      supportingEpisodes: ["episode-2"],
      scope: [],
    },
    createdAt: 10,
  };
  const refinement = {
    contradictionId: 7,
    discriminator: {
      feature: "ovenType",
      leftValue: "convection",
      leftEpisode: "episode-1",
      rightValue: "conventional",
      rightEpisode: "episode-2",
    },
    updatedAt: 11,
  };
  await client.recordContradiction(record);
  await client.refineContradiction(refinement);
  await client.getClaimUncertainty("recipe-plan");

  assert.deepEqual(transport.calls, [
    { method: "contradiction.list", params: {} },
    { method: "contradiction.get", params: { contradictionId: 7 } },
    { method: "contradiction.record", params: record },
    { method: "contradiction.refine", params: refinement },
    {
      method: "contradiction.uncertainty",
      params: { claimId: "recipe-plan" },
    },
  ]);
});

test("admin-enabled client injects auth only into privileged mutation calls", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport, { adminToken: "bootstrap-secret" });

  await client.createConcept({ name: "ADMIN_ONLY" });
  await client.listConcepts();
  await client.applyAdaptationOffline({
    planId: "plan-1",
    idempotencyKey: "offline-apply-1",
    appliedAt: 1,
  });
  await client.recordContradiction({
    left: {
      id: "left",
      statement: "left",
      implication: { predicate: "p", value: true },
      supportingEpisodes: ["episode-1"],
      scope: [],
    },
    right: {
      id: "right",
      statement: "right",
      implication: { predicate: "p", value: false },
      supportingEpisodes: ["episode-2"],
      scope: [],
    },
    createdAt: 2,
  });
  await client.refineContradiction({
    contradictionId: 1,
    discriminator: {
      feature: "scope",
      leftValue: "a",
      leftEpisode: "episode-1",
      rightValue: "b",
      rightEpisode: "episode-2",
    },
    updatedAt: 3,
  });

  assert.deepEqual(transport.calls, [
    {
      method: "concept.create",
      params: { name: "ADMIN_ONLY", adminToken: "bootstrap-secret" },
    },
    { method: "concept.list", params: {} },
    {
      method: "adaptation.applyOffline",
      params: {
        planId: "plan-1",
        idempotencyKey: "offline-apply-1",
        appliedAt: 1,
        adminToken: "bootstrap-secret",
      },
    },
    {
      method: "contradiction.record",
      params: {
        left: {
          id: "left",
          statement: "left",
          implication: { predicate: "p", value: true },
          supportingEpisodes: ["episode-1"],
          scope: [],
        },
        right: {
          id: "right",
          statement: "right",
          implication: { predicate: "p", value: false },
          supportingEpisodes: ["episode-2"],
          scope: [],
        },
        createdAt: 2,
        adminToken: "bootstrap-secret",
      },
    },
    {
      method: "contradiction.refine",
      params: {
        contradictionId: 1,
        discriminator: {
          feature: "scope",
          leftValue: "a",
          leftEpisode: "episode-1",
          rightValue: "b",
          rightEpisode: "episode-2",
        },
        updatedAt: 3,
        adminToken: "bootstrap-secret",
      },
    },
  ]);
});

test("feedback sends raw observation without caller-selected trust fields", async () => {
  const transport = new RecordingTransport();
  const client = new EkgClient(transport);

  await client.recordFeedback({
    episodeId: "episode-1",
    observedResult: "flat pancakes",
    idempotencyKey: "raw-feedback-1",
  });

  assert.deepEqual(transport.calls, [
    {
      method: "feedback.record",
      params: {
        episodeId: "episode-1",
        observedResult: "flat pancakes",
        idempotencyKey: "raw-feedback-1",
      },
    },
  ]);
});
