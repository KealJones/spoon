import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import test from "node:test";

import {
  CodexTeacher,
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
