import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import test from "node:test";

import {
  CliTeacher,
  CodexTeacher,
  lowerCodexSchema,
  type CommandInvocation,
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
  const canonical = {
    type: "object" as const,
    additionalProperties: false,
    properties: {
      proposalKind: { type: "string" as const },
      lesson: { $ref: "#/$defs/pureExprV2" },
      answer: {
        type: ["null", "boolean", "number", "string", "array", "object"],
      },
    },
    required: ["proposalKind", "lesson"],
    $defs: {
      pureExprV2: {
        anyOf: [{ $ref: "#/$defs/pureExprV2" }],
      },
    },
  };

  const lowered = lowerCodexSchema(
    canonical as unknown as TeacherRequest["desiredOutput"],
  );

  assert.equal(lowered.type, "object");
  assert.equal(lowered.additionalProperties, false);
  assert.deepEqual(lowered.required, ["proposalJson"]);
  assert.deepEqual(lowered.properties, { proposalJson: { type: "string" } });
  assert.deepEqual(canonical.properties.lesson, { $ref: "#/$defs/pureExprV2" });
  assert.ok("$defs" in canonical);
});

test("Codex unwraps a complex proposal JSON envelope before local validation", async () => {
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
  const complexRequest = {
    ...request,
    desiredOutput: {
      type: "object" as const,
      properties: { answer: { type: ["number", "string"] } },
      required: ["answer"],
    } as TeacherRequest["desiredOutput"],
  };

  const proposal = await teacher.propose(complexRequest);

  assert.deepEqual(proposal.content, { answer: 14 });
});

test("Codex does not put a recursive canonical schema in the command prompt", async () => {
  let prompt = "";
  const canonical = {
    type: "object" as const,
    additionalProperties: false,
    properties: {
      proposalKind: { type: "string" as const },
      lesson: { $ref: "#/$defs/pureExprV2" },
    },
    required: ["proposalKind", "lesson"],
    $defs: { pureExprV2: { anyOf: [{ $ref: "#/$defs/pureExprV2" }] } },
  };
  const teacher = new CodexTeacher({
    runner: async (received) => {
      prompt = received.args.at(-1) ?? "";
      const outputFlag = received.args.indexOf("--output-last-message");
      await writeFile(
        received.args[outputFlag + 1]!,
        JSON.stringify({
          proposalJson: JSON.stringify({ proposalKind: "abstain" }),
        }),
      );
      return { exitCode: 0, stdout: "", stderr: "" };
    },
  });

  await teacher.propose({
    ...request,
    desiredOutput: canonical as unknown as TeacherRequest["desiredOutput"],
  });

  assert.doesNotMatch(prompt, /\$defs/);
  assert.match(prompt, /lesson/);
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
