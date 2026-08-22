import assert from "node:assert/strict";
import test from "node:test";

import {
  createInspectorServer,
  episodeDetail,
  inspectorHtml,
  redactSensitive,
} from "../src/server.js";

test("inspector package exposes a local dashboard entry point", async () => {
  const packageJson = await import("../package.json", {
    with: { type: "json" },
  });
  assert.equal(packageJson.default.scripts.dev, "tsx src/server.ts");
  assert.equal(packageJson.default.scripts.start, "node dist/src/server.js");
});

test("inspector labels Phase 6 slots as bounded evidence, not broad claims", () => {
  const dashboard = inspectorHtml();
  assert.match(dashboard, /Teacher-request episodes/);
  assert.match(dashboard, /Persisted transfer wins/);
  assert.match(dashboard, /Preserved replay verdicts/);
  assert.match(dashboard, /post-promotion success/);
  assert.match(dashboard, /not held-out task-family coverage/);
  assert.match(dashboard, /No domains or comparable time cohorts/);
});

test("episode narrative explains teacher, validation, learning, outcome, and cost", () => {
  const detail = episodeDetail({
    id: "episode-1",
    situation: "double 21",
    action: "learn-procedure",
    prediction: 42,
    observed_result: 42,
    execution_trace: [{ action: "run local procedure" }],
    cost: { rung_reached: "Ask", steps_taken: 4, budget_spent: 0.12 },
    evaluation: {
      tier: "Hard",
      success: true,
      details: "exact match",
      surprise: 0,
    },
    teacher_interaction: {
      source: "openai:teacher",
      provenance: { provider: "openai", model: "gpt-test" },
      content: {
        proposalKind: "reusable_lesson",
        lesson: {
          procedures: [{ name: "DOUBLE" }],
          concepts: [{ name: "DOUBLE" }],
        },
      },
      validation: {
        status: "verified",
        checks: [
          {
            validator: "contract",
            status: "verified",
            reason: "all tests passed",
          },
        ],
      },
    },
  });

  assert.deepEqual(detail.narrative.teacher, {
    used: true,
    provider: "openai",
    model: "gpt-test",
    source: "openai:teacher",
    proposal: {
      proposalKind: "reusable_lesson",
      lesson: {
        procedures: [{ name: "DOUBLE" }],
        concepts: [{ name: "DOUBLE" }],
      },
    },
    proposalSummary: "reusable lesson: DOUBLE",
    validation: {
      status: "verified",
      checks: [
        {
          validator: "contract",
          status: "verified",
          reason: "all tests passed",
        },
      ],
    },
  });
  assert.deepEqual(detail.narrative.learning, {
    action: "learn-procedure",
    summary: "Learned or promoted reusable knowledge.",
    procedures: ["DOUBLE"],
    concepts: ["DOUBLE"],
  });
  assert.deepEqual(detail.narrative.cost, {
    rung: "Ask",
    stepsTaken: 4,
    budgetSpent: 0.12,
  });
  assert.deepEqual(detail.narrative.evaluation, {
    tier: "Hard",
    success: true,
    details: "exact match",
    surprise: 0,
  });
});

test("narrative and raw drill-down redact secret-like values", () => {
  const detail = episodeDetail({
    situation: "private task",
    context: { environment: { DATABASE_URL: "postgres://should-not-leak" } },
    teacher_interaction: {
      authorization: "Bearer very-secret",
      content: { apiKey: "should-not-leak", answer: "ok" },
    },
  });
  const rendered = JSON.stringify(detail);
  assert.match(rendered, /\[REDACTED\]/);
  assert.doesNotMatch(rendered, /should-not-leak|very-secret/);
  assert.deepEqual(redactSensitive({ nested: { cookie: "session=secret" } }), {
    nested: { cookie: "[REDACTED]" },
  });
});

test("episode detail endpoint is read-only and returns the redacted projection", async (t) => {
  const server = createInspectorServer({
    metricsSnapshot: async () => ({}),
    listConcepts: async () => [],
    listProcedures: async () => [],
    listEpisodes: async () => [],
    getEpisode: async (id) => ({
      id,
      situation: "inspect me",
      teacher_interaction: { token: "nope" },
    }),
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(
    () =>
      new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      ),
  );
  const address = server.address();
  assert.ok(address && typeof address !== "string");

  const response = await fetch(
    `http://127.0.0.1:${address.port}/api/episodes/episode-2`,
  );
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), {
    narrative: {
      id: "episode-2",
      request: "inspect me",
      escalation: {},
      teacher: { used: true },
      learning: {
        action: "no reusable procedure",
        summary: "No reusable procedure was learned.",
        procedures: [],
        concepts: [],
      },
      cost: {},
    },
    raw: {
      id: "episode-2",
      situation: "inspect me",
      teacher_interaction: { token: "[REDACTED]" },
    },
  });
});
