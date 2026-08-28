import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import test from "node:test";

import {
  CliTeacher,
  CODEX_FLAT_AUTHORING_SCHEMA,
  CodexTeacher,
  decodeCodexFlatAuthoring,
  lowerCodexSchema,
  validateSchema,
  type CommandInvocation,
  type JsonValue,
  type TeacherRequest,
} from "../src/index.js";

const request: TeacherRequest = {
  situation: "what is double 7?",
  context: {},
  desiredOutput: {
    type: "object",
    additionalProperties: false,
    properties: { answer: { type: "number" } },
    required: ["answer"],
  },
};

const flatAnswer = (answerJson: string) => ({
  format: "spoon_flat_expr_v1",
  proposalKind: "answer_only",
  interpretations: [],
  lesson: null,
  answerJson,
  abstainReason: "",
});

const canonicalTeacherSchema = {
  type: "object" as const,
  additionalProperties: false,
  properties: {
    proposalKind: { type: "string" as const },
    interpretations: { type: "array" as const, items: false },
    lesson: { $ref: "#/$defs/pureExprV2" },
    procedure: { type: "null" as const },
    answer: {
      type: ["null", "boolean", "number", "string", "array", "object"],
    },
    abstainReason: { type: ["string", "null"] },
  },
  required: [
    "proposalKind",
    "interpretations",
    "lesson",
    "procedure",
    "answer",
    "abstainReason",
  ],
  $defs: {
    pureExprV2: { anyOf: [{ $ref: "#/$defs/pureExprV2" }] },
  },
} as unknown as TeacherRequest["desiredOutput"];

test("Codex CLI teacher runs ephemerally in an isolated read-only directory", async () => {
  let invocation: CommandInvocation | undefined;
  const teacher = new CodexTeacher({
    model: "gpt-test",
    idFactory: () => "codex-request",
    now: () => new Date("2026-08-22T00:00:00.000Z"),
    runner: async (received) => {
      invocation = received;
      const outputFlag = received.args.indexOf("--output-last-message");
      await writeFile(received.args[outputFlag + 1]!, '{"answer":14}');
      return { exitCode: 0, stdout: "", stderr: "" };
    },
  });

  const proposal = await teacher.propose(request);

  assert.equal(invocation?.command, "codex");
  assert.deepEqual(invocation?.args.slice(0, 7), [
    "exec",
    "--ephemeral",
    "--ignore-user-config",
    "--ignore-rules",
    "--sandbox",
    "read-only",
    "--skip-git-repo-check",
  ]);
  assert.ok(invocation?.args.includes("--output-schema"));
  assert.ok(invocation?.args.includes("--output-last-message"));
  assert.ok(invocation?.args.includes("gpt-test"));
  assert.ok(invocation?.cwd?.includes("spoon-codex-teacher-"));
  assert.deepEqual(proposal.content, { answer: 14 });
  assert.equal(proposal.source, "codex:gpt-test");
  assert.equal(proposal.provenance.provider, "codex");
  assert.equal(proposal.provenance.requestId, "codex-request");
});

test("Codex CLI failures remain provider failures", async () => {
  const teacher = new CodexTeacher({
    runner: async () => ({
      exitCode: 1,
      stdout: "",
      stderr: "login required",
    }),
  });

  await assert.rejects(
    teacher.propose(request),
    /codex: command exited with status 1: login required/,
  );
});

test("Codex lowers recursive lesson schemas only at the provider boundary", () => {
  const lowered = lowerCodexSchema(canonicalTeacherSchema);

  assert.equal(lowered.type, "object");
  assert.equal(lowered.additionalProperties, false);
  assert.equal(lowered, CODEX_FLAT_AUTHORING_SCHEMA);
  assert.deepEqual(lowered.required, [
    "format",
    "proposalKind",
    "interpretations",
    "lesson",
    "answerJson",
    "abstainReason",
  ]);
  assert.ok("format" in (lowered.properties ?? {}));
  assert.ok(!("proposalJson" in (lowered.properties ?? {})));
  assert.deepEqual(canonicalTeacherSchema.properties?.lesson, {
    $ref: "#/$defs/pureExprV2",
  });
  assert.ok("$defs" in canonicalTeacherSchema);
});

test("Codex decodes a strict flat Spoon proposal before local validation", async () => {
  const teacher = new CodexTeacher({
    runner: async (received) => {
      const outputFlag = received.args.indexOf("--output-last-message");
      await writeFile(
        received.args[outputFlag + 1]!,
        JSON.stringify(flatAnswer("14")),
      );
      return { exitCode: 0, stdout: "", stderr: "" };
    },
  });
  const complexRequest = {
    ...request,
    desiredOutput: canonicalTeacherSchema,
  };

  const proposal = await teacher.propose(complexRequest);

  assert.deepEqual(proposal.content, {
    proposalKind: "answer_only",
    interpretations: [],
    lesson: null,
    procedure: null,
    answer: 14,
    abstainReason: null,
  });
});

test("Codex keeps the generic JSON envelope for non-Spoon recursive schemas", async () => {
  const teacher = new CodexTeacher({
    runner: async (received) => {
      const outputFlag = received.args.indexOf("--output-last-message");
      await writeFile(
        received.args[outputFlag + 1]!,
        JSON.stringify({ proposalJson: JSON.stringify({ answer: 14 }) }),
      );
      return { exitCode: 0, stdout: "", stderr: "" };
    },
  });
  const proposal = await teacher.propose({
    ...request,
    desiredOutput: {
      type: "object" as const,
      properties: { answer: { type: ["number", "string"] } },
      required: ["answer"],
    } as TeacherRequest["desiredOutput"],
  });

  assert.deepEqual(proposal.content, { answer: 14 });
});

test("flat authoring strictly rejects target and expands field/index nodes", () => {
  const flat = {
    format: "spoon_flat_expr_v1",
    proposalKind: "reusable_lesson",
    interpretations: [],
    answerJson: '"foo"',
    abstainReason: "",
    lesson: {
      primitiveSet: "spoon_flat_expr_v1",
      concepts: [
        {
          key: "get_path",
          name: "GET PATH",
          description: "Read a value at a requested path.",
          mutability: "procedural",
        },
      ],
      relationships: [],
      procedures: [
        {
          key: "get_path",
          name: "GET PATH",
          concept: { kind: "new_concept", key: "get_path" },
          parameters: [
            {
              name: "arr",
              description: "Values to inspect.",
              valueType: "list",
            },
          ],
          body: {
            nodes: [
              { id: "arr", kind: "parameter", name: "arr" },
              { id: "zero", kind: "literal", valueJson: "0" },
              {
                id: "first",
                kind: "index",
                collection: "arr",
                index: "zero",
              },
              {
                id: "name",
                kind: "field",
                object: "first",
                field: "name",
              },
            ],
            result: "name",
          },
          contract: { requires: [], promises: [], failsWhen: [] },
        },
      ],
      invocation: {
        procedureKey: "get_path",
        inputs: [
          {
            name: "arr",
            valueJson: '[{"name":"foo"},{"name":"bar"}]',
          },
        ],
      },
    },
  } as unknown as JsonValue;
  assert.deepEqual(validateSchema(flat, CODEX_FLAT_AUTHORING_SCHEMA), []);

  const decoded = decodeCodexFlatAuthoring(flat);
  const proposal = decoded as {
    lesson: { procedures: Array<{ body: unknown }> };
  };
  assert.deepEqual(proposal.lesson.procedures[0]?.body, {
    kind: "field",
    object: {
      kind: "index",
      collection: { kind: "parameter", name: "arr" },
      index: { kind: "literal", value: 0 },
    },
    field: "name",
  });

  const malformed = structuredClone(flat) as unknown as {
    lesson: { procedures: Array<{ body: { nodes: unknown[] } }> };
  };
  const field = malformed.lesson.procedures[0]!.body.nodes[3] as {
    object?: string;
    target?: string;
  };
  field.target = field.object;
  delete field.object;
  assert.ok(
    validateSchema(
      malformed as unknown as JsonValue,
      CODEX_FLAT_AUTHORING_SCHEMA,
    ).length > 0,
  );
});

test("flat authoring expands a capability call into canonical expression IR", () => {
  const flat = {
    format: "spoon_flat_expr_v1",
    proposalKind: "reusable_lesson",
    interpretations: [],
    answerJson: "null",
    abstainReason: "",
    lesson: {
      primitiveSet: "spoon_flat_expr_v1",
      concepts: [
        {
          key: "fetch_page",
          name: "FETCH PAGE",
          description:
            "Fetch a page when a runtime request reaches this procedure.",
          mutability: "procedural",
        },
      ],
      relationships: [],
      procedures: [
        {
          key: "fetch_page",
          name: "FETCH PAGE",
          concept: { kind: "new_concept", key: "fetch_page" },
          parameters: [
            { name: "url", description: "URL to fetch.", valueType: "text" },
          ],
          body: {
            nodes: [
              { id: "url", kind: "parameter", name: "url" },
              {
                id: "fetch",
                kind: "capability_call",
                contentId: "spoon.native",
                procedureId: "web.fetch",
                input: "url",
              },
            ],
            result: "fetch",
          },
          contract: { requires: [], promises: [], failsWhen: [] },
        },
      ],
      invocation: {
        procedureKey: "fetch_page",
        inputs: [{ name: "url", valueJson: '"https://example.com"' }],
      },
    },
  } as unknown as JsonValue;

  assert.deepEqual(validateSchema(flat, CODEX_FLAT_AUTHORING_SCHEMA), []);
  const proposal = decodeCodexFlatAuthoring(flat) as {
    lesson: { procedures: Array<{ body: unknown }> };
  };
  assert.deepEqual(proposal.lesson.procedures[0]?.body, {
    kind: "capability_call",
    contentId: "spoon.native",
    procedureId: "web.fetch",
    input: { kind: "parameter", name: "url" },
  });
});

test("Codex does not put a recursive canonical schema in the command prompt", async () => {
  let prompt = "";
  const teacher = new CodexTeacher({
    runner: async (received) => {
      prompt = received.args.at(-1) ?? "";
      const outputFlag = received.args.indexOf("--output-last-message");
      await writeFile(
        received.args[outputFlag + 1]!,
        JSON.stringify(flatAnswer("null")),
      );
      return { exitCode: 0, stdout: "", stderr: "" };
    },
  });

  await teacher.propose({
    ...request,
    desiredOutput: canonicalTeacherSchema,
  });

  assert.doesNotMatch(prompt, /\$defs/);
  assert.match(prompt, /spoon_flat_expr_v1/);
  assert.match(prompt, /There is no target property/);
  assert.doesNotMatch(prompt, /"nodes": \{/);
});

test("Codex CLI transport accepts a separate structured-output protocol", async () => {
  let prompt = "";
  const teacher = new CodexTeacher({
    systemPrompt: "You are the Judge.",
    promptBuilder: () => "Judge only immutable evidence.",
    runner: async (received) => {
      prompt = received.args.at(-1) ?? "";
      const outputFlag = received.args.indexOf("--output-last-message");
      await writeFile(received.args[outputFlag + 1]!, '{"verdict":"pass"}');
      return { exitCode: 0, stdout: "", stderr: "" };
    },
  });

  const proposal = await teacher.propose({
    ...request,
    desiredOutput: {
      type: "object",
      properties: { verdict: { type: "string" } },
      required: ["verdict"],
    },
  });

  assert.match(prompt, /You are the Judge/);
  assert.match(prompt, /Judge only immutable evidence/);
  assert.doesNotMatch(prompt, /You are a Spoon teacher/);
  assert.deepEqual(proposal.content, { verdict: "pass" });
});

test("generic CLI adapter reuses structured output with a shell-free command", async () => {
  let received: CommandInvocation | undefined;
  const teacher = new CliTeacher({
    command: "local-judge --json",
    systemPrompt: "Judge system",
    promptBuilder: () => "Judge prompt",
    runner: async (invocation) => {
      received = invocation;
      return { exitCode: 0, stdout: '{"verdict":"pass"}', stderr: "" };
    },
  });
  const proposal = await teacher.propose(request);
  assert.deepEqual(received, {
    command: "local-judge",
    args: [
      "--json",
      "Judge system\n\nJudge prompt\n\nReturn only the requested JSON object.",
    ],
  });
  assert.deepEqual(proposal.content, { verdict: "pass" });
  assert.equal(proposal.provenance.provider, "cli");
});
