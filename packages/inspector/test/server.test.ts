import assert from "node:assert/strict";
import test from "node:test";

import {
  createInspectorServer,
  knowledgeGraph,
  episodeDetail,
  inspectorHtml,
  procedureDetail,
  redactSensitive,
} from "../src/server.js";

test("inspector package exposes a local dashboard entry point", async () => {
  const packageJson = await import("../package.json", {
    with: { type: "json" },
  });
  assert.equal(packageJson.default.scripts.dev, "tsx watch src/server.ts");
  assert.equal(packageJson.default.scripts.start, "node dist/src/server.js");
});

test("episode detail binds replay by element id, not getElementById of a CSS selector", () => {
  const dashboard = inspectorHtml();
  assert.match(dashboard, /id="replay-button"/);
  assert.match(dashboard, /\$\('replay-button'\)\?\.addEventListener/);
  assert.doesNotMatch(dashboard, /\$\('\[data-replay\]'\)\.addEventListener/);
  assert.match(dashboard, /detail\.narrative\|\|/);
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
    proposalKind: "reusable_lesson",
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

test("episode narrative projects interpreter frames, provenance, and request context", () => {
  const detail = episodeDetail({
    id: "episode-interp",
    situation: "please make 7 twice as large",
    action: "reuse-procedure",
    teacher_interaction: {
      languageInterpreter: {
        request: {
          situation: "please make 7 twice as large",
          context: {
            candidates: [{ alias: "candidate_0" }],
            priorTurns: [{ role: "user" }],
          },
        },
        source: "ollama:qwen2.5:0.5b",
        status: "unverified",
        provenance: { provider: "ollama", model: "qwen2.5:0.5b" },
        frames: {
          disposition: "execute",
          selected: 0,
          candidates: [
            {
              name: "candidate_0",
              confidence: 0.98,
              ambiguities: [],
              slots: [{ name: "x", value: 7, confidence: 0.99 }],
            },
          ],
        },
      },
    },
  });

  assert.deepEqual(detail.narrative.teacher, { used: false });
  assert.deepEqual(detail.narrative.interpreter, {
    used: true,
    source: "ollama:qwen2.5:0.5b",
    status: "unverified",
    provider: "ollama",
    model: "qwen2.5:0.5b",
    disposition: "execute",
    selected: 0,
    candidateCount: 1,
    priorTurnCount: 1,
    candidates: [
      {
        name: "candidate_0",
        confidence: 0.98,
        selected: true,
        ambiguities: [],
        slots: [{ name: "x", value: 7, confidence: 0.99 }],
      },
    ],
  });
});

test("episode narrative finds interpreter under priorFailure after teacher fallback", () => {
  const detail = episodeDetail({
    id: "episode-fallback",
    situation: "make this bigger somehow",
    teacher_interaction: {
      request: { situation: "make this bigger somehow" },
      proposal: {
        provenance: { provider: "openai", model: "gpt-test" },
        content: { proposalKind: "reusable_lesson" },
      },
      priorFailure: {
        languageInterpreter: {
          request: {
            context: { candidates: [{}, {}], priorTurns: [] },
          },
          source: "ollama:qwen2.5:0.5b",
          status: "unverified",
          provenance: { provider: "ollama", model: "qwen2.5:0.5b" },
          frames: { disposition: "abstain", candidates: [], selected: null },
        },
      },
    },
  });

  assert.equal(
    (detail.narrative.interpreter as { used: boolean; disposition: string })
      .used,
    true,
  );
  assert.equal(
    (detail.narrative.interpreter as { disposition: string }).disposition,
    "abstain",
  );
  assert.equal(
    (detail.narrative.interpreter as { candidateCount: number }).candidateCount,
    2,
  );
  assert.equal((detail.narrative.teacher as { used: boolean }).used, true);
});

test("episode narrative projects interpreter rejection and rejected proposal", () => {
  const detail = episodeDetail({
    id: "episode-rejected",
    situation: "Spell strawberry",
    teacher_interaction: {
      languageInterpreter: {
        request: { context: { candidates: [{}] } },
        source: "ollama:test",
        status: "unverified",
        provenance: { provider: "ollama", model: "test" },
        rejection:
          "interpreter selected a procedure without enough language support",
        providerError: undefined,
        rejectedProposal: {
          content: { disposition: "execute", selected: 0 },
          rawContent: { modelOutput: "count strawberry" },
        },
      },
    },
  });

  const interpreter = detail.narrative.interpreter as {
    rejection: string;
    rejectedProposal: { disposition: string; modelOutput: string };
  };
  assert.equal(
    interpreter.rejection,
    "interpreter selected a procedure without enough language support",
  );
  assert.deepEqual(interpreter.rejectedProposal, {
    disposition: "execute",
    selected: 0,
    modelOutput: "count strawberry",
  });
});

test("inspector html renders interpreter data on episode detail", () => {
  const dashboard = inspectorHtml();
  assert.match(dashboard, /<h3>Interpreter<\/h3>/);
  assert.match(dashboard, /n\.interpreter/);
  assert.match(dashboard, /rejectedProposal/);
});

test("episode narrative unwraps nested teacher observations", () => {
  const detail = episodeDetail({
    id: "episode-observation",
    situation: "first line of README",
    action: "teacher-observation:provisional",
    prediction: "# Spoon",
    observed_result: null,
    teacher_interaction: {
      request: { situation: "first line of README" },
      proposal: {
        provenance: {
          provider: "codex",
          model: "gpt-5.6-sol",
          teacher: "codex:gpt-5.6-sol",
        },
        content: {
          proposalKind: "external_observation",
          answer: "# Spoon",
        },
        validation: { status: "provisional" },
      },
    },
  });

  assert.deepEqual(detail.narrative.teacher, {
    used: true,
    provider: "codex",
    model: "gpt-5.6-sol",
    source: "codex:gpt-5.6-sol",
    proposal: {
      proposalKind: "external_observation",
      answer: "# Spoon",
    },
    proposalKind: "external_observation",
    proposalSummary: "external observation: # Spoon",
    validation: { status: "provisional" },
  });
  assert.deepEqual(detail.narrative.learning, {
    action: "teacher-observation:provisional",
    summary:
      "Teacher supplied a provisional external answer; no reusable lesson or procedure was proposed or admitted.",
    answerSource: "teacher-provided external observation (unverified)",
    reusableKnowledge: false,
    procedures: [],
    concepts: [],
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
      teacher: { used: false },
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

test("knowledge projection keeps bounded nodes and edges while preserving relationships", () => {
  const graph = knowledgeGraph(
    [
      { id: "c1", name: "addition", lifecycle: "active" },
      { id: "c2", name: "numbers", lifecycle: "validated" },
    ],
    [{ id: "p1", name: "ADD", version: 2, concept: "c1" }],
    [{ id: "r1", source: "c1", target: "c2", kind: "supports", strength: 0.9 }],
    2,
    1,
  );

  assert.equal(graph.nodes.length, 2);
  assert.equal(graph.edges.length, 1);
  assert.equal(graph.edges[0]?.kind, "supports");
  assert.equal(graph.bounded, true);
});

test("inspector exposes graph, procedure history, contradictions, filtered episodes, and replay", async (t) => {
  const server = createInspectorServer({
    metricsSnapshot: async () => ({}),
    listConcepts: async () => [{ id: "c1", name: "addition" }],
    listProcedures: async () => [{ id: "p1", name: "ADD", version: 2 }],
    listRelationships: async () => [
      {
        id: "r1",
        source: "c1",
        target: "p1",
        kind: "implements",
        token: "hide-me",
      },
    ],
    listProcedureVersions: async () => [
      { id: "p1", name: "ADD", version: 1 },
      { id: "p1", name: "ADD", version: 2 },
    ],
    getProcedure: async () => ({
      id: "p1",
      name: "ADD",
      contract: { preconditions: [] },
      test_cases: [{ expected_output: 3 }],
    }),
    listContradictions: async () => [
      {
        id: 4,
        status: "Held",
        left: { statement: "a", supportingEpisodes: ["e1"] },
        right: { statement: "not a", supportingEpisodes: ["e2"] },
      },
    ],
    getContradiction: async () => ({
      id: 4,
      left: { statement: "a", apiKey: "do-not-show" },
      right: { statement: "not a" },
    }),
    listEpisodes: async () => [
      { id: "e1", situation: "double 7", action: "answer-only" },
      { id: "e2", situation: "time", action: "teacher" },
    ],
    getEpisode: async (id) => ({ id, situation: "double 7" }),
    replayEpisode: async (_id, substitutions) => ({
      observed: 14,
      substitutions,
      token: "should-not-show",
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
  const base = `http://127.0.0.1:${address.port}`;

  const graph = await (await fetch(`${base}/api/knowledge`)).json();
  assert.equal(graph.edges[0].kind, "implements");
  assert.equal(graph.edges[0].token, undefined);

  const procedure = await (await fetch(`${base}/api/procedures/p1`)).json();
  assert.equal(procedure.versions.length, 2);
  assert.equal(procedure.procedure.contract.preconditions.length, 0);

  const contradiction = await (
    await fetch(`${base}/api/contradictions/4`)
  ).json();
  assert.equal(contradiction.left.apiKey, "[REDACTED]");

  const episodes = await (await fetch(`${base}/api/episodes?q=double`)).json();
  assert.deepEqual(
    episodes.map((episode: { id: string }) => episode.id),
    ["e1"],
  );

  const replay = await (
    await fetch(
      `${base}/api/episodes/e1/replay?substitutions=${encodeURIComponent(JSON.stringify({ token: "secret" }))}`,
    )
  ).json();
  assert.equal(replay.readOnly, true);
  assert.equal(replay.result.token, "[REDACTED]");
  assert.equal(replay.result.substitutions.token, "[REDACTED]");
});

test("procedure detail redacts contract and test secrets", () => {
  const detail = procedureDetail({ id: "p1", contract: { apiKey: "secret" } }, [
    { version: 1, tests: [{ token: "secret" }] },
  ]);
  assert.equal(
    (detail.procedure as { contract: { apiKey: string } }).contract.apiKey,
    "[REDACTED]",
  );
  assert.equal(
    (detail.versions[0] as { tests: Array<{ token: string }> }).tests[0]?.token,
    "[REDACTED]",
  );
});
