import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  SpoonClient,
  StdioTransport,
  type CycleProgress,
  type RpcTransport,
} from "@spoon/sdk";
import {
  CodexTeacher,
  ProposalValidationPipeline,
  fingerprintTeacherRequest,
  type Teacher,
  type TeacherProposal,
  type TeacherRequest,
} from "@spoon/teacher";

import { createConfiguredTeacher, runCycle } from "../src/cycle.js";

class FakeTeacher implements Pick<Teacher, "propose" | "validationPipeline"> {
  calls: TeacherRequest[] = [];
  validationCalls = 0;

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
    this.calls.push(request);
    return {
      content: {
        interpretations: [],
        procedure: null,
        answer: 14,
        abstainReason: null,
      },
      source: "human:fake",
      status: "unverified",
      provenance: {
        provider: "human",
        teacher: "human:fake",
        requestId: "request-1",
        requestHash: fingerprintTeacherRequest(request),
        generatedAt: "2026-08-22T00:00:00.000Z",
        situation: request.situation,
        ...(request.specificQuestion === undefined
          ? {}
          : { specificQuestion: request.specificQuestion }),
      },
    };
  }

  validationPipeline(): ProposalValidationPipeline {
    this.validationCalls += 1;
    return new ProposalValidationPipeline();
  }
}

class LearningCycleTransport implements RpcTransport {
  readonly calls: Array<{ method: string; params: unknown }> = [];
  #learned = false;

  async request<T>(method: string, params: unknown): Promise<T> {
    this.calls.push({ method, params });
    if (method === "cycle.begin" && !this.#learned) {
      return {
        status: "need_teacher",
        cycleId: "cycle-1",
        request: {
          situation: "what is double 7?",
          context: {},
          specificQuestion: "resolve this task",
          desiredOutput: { type: "object" },
        },
      } as T;
    }
    if (method === "cycle.resume") {
      this.#learned = true;
    }
    return completed() as T;
  }
}

class FailingTeacher implements Pick<
  Teacher,
  "propose" | "validationPipeline"
> {
  calls = 0;

  async propose(): Promise<TeacherProposal> {
    this.calls += 1;
    throw new Error("provider authentication failed");
  }

  validationPipeline(): ProposalValidationPipeline {
    return new ProposalValidationPipeline();
  }
}

class AbortCycleTransport implements RpcTransport {
  readonly calls: Array<{ method: string; params: unknown }> = [];

  async request<T>(method: string, params: unknown): Promise<T> {
    this.calls.push({ method, params });
    if (method === "cycle.begin") {
      return {
        status: "need_teacher",
        cycleId: "cycle-failure",
        request: {
          situation: "unknown",
          context: {},
          desiredOutput: { type: "object" },
        },
      } as T;
    }
    return {
      status: "completed",
      cycleId: "cycle-failure",
      disposition: "abstained",
      answer: null,
      episode: { action: "abstain:teacher-provider-failure" },
    } as T;
  }
}

class RetryCycleTransport implements RpcTransport {
  readonly calls: Array<{ method: string; params: unknown }> = [];
  #resumes = 0;

  async request<T>(method: string, params: unknown): Promise<T> {
    this.calls.push({ method, params });
    if (
      method === "cycle.begin" ||
      (method === "cycle.resume" && this.#resumes++ === 0)
    ) {
      return {
        status: "need_teacher",
        cycleId: "cycle-retry",
        request: {
          situation: "what is double 7?",
          context: {},
          specificQuestion:
            method === "cycle.begin"
              ? "return reusable knowledge"
              : "lesson could not be safely compiled; correct it",
          desiredOutput: { type: "object" },
        },
      } as T;
    }
    return completed() as T;
  }
}

function completed(): CycleProgress {
  return {
    status: "completed",
    cycleId: "cycle-1",
    disposition: "verified",
    answer: 14,
    episode: {},
  };
}

test("automatic teacher handoff happens once and a learned repeat stays local", async () => {
  const transport = new LearningCycleTransport();
  const client = new SpoonClient(transport);
  const teacher = new FakeTeacher();

  assert.equal(
    (await runCycle(client, "what is double 7?", teacher)).answer,
    14,
  );
  assert.equal((await runCycle(client, "please double 7", teacher)).answer, 14);

  assert.equal(teacher.calls.length, 1);
  assert.equal(teacher.validationCalls, 1);
  assert.deepEqual(
    transport.calls.map((call) => call.method),
    ["cycle.begin", "cycle.resume", "cycle.begin"],
  );
});

test("provider failures abort the pending cycle instead of abandoning it", async () => {
  const transport = new AbortCycleTransport();
  const teacher = new FailingTeacher();

  const outcome = await runCycle(
    new SpoonClient(transport),
    "unknown",
    teacher,
  );

  assert.equal(outcome.disposition, "abstained");
  assert.equal(teacher.calls, 1);
  assert.deepEqual(
    transport.calls.map((call) => call.method),
    ["cycle.begin", "cycle.abort"],
  );
  assert.deepEqual(transport.calls[1]?.params, {
    cycleId: "cycle-failure",
    reason: "provider authentication failed",
  });
});

test("Teacher-OFF cycles abort cleanly through the public cycle path", async () => {
  const transport = new AbortCycleTransport();
  const outcome = await runCycle(
    new SpoonClient(transport),
    "unknown",
    undefined,
  );

  assert.equal(outcome.disposition, "abstained");
  assert.deepEqual(
    transport.calls.map((call) => call.method),
    ["cycle.begin", "cycle.abort"],
  );
  assert.deepEqual(transport.calls[1]?.params, {
    cycleId: "cycle-failure",
    reason: "teacher is disabled for this run",
  });
});

test("a malformed lesson can consume one bounded retry and then complete", async () => {
  const transport = new RetryCycleTransport();
  const teacher = new FakeTeacher();

  const outcome = await runCycle(
    new SpoonClient(transport),
    "what is double 7?",
    teacher,
    { maxTeacherTurns: 2 },
  );

  assert.equal(outcome.status, "completed");
  assert.equal(teacher.calls.length, 2);
  assert.match(teacher.calls[1]?.specificQuestion ?? "", /safely compiled/);
  assert.deepEqual(
    transport.calls.map((call) => call.method),
    ["cycle.begin", "cycle.resume", "cycle.resume"],
  );
});

test("Codex CLI can be selected without an API key or explicit model", () => {
  const teacher = createConfiguredTeacher({ SPOON_TEACHER: "codex" });
  assert.ok(teacher instanceof CodexTeacher);
});

test(
  "fake teacher teaches a procedure once and the Rust cycle reuses it locally",
  { timeout: 30_000 },
  async () => {
    const directory = await mkdtemp(
      path.join(os.tmpdir(), "spoon-teacher-cycle-"),
    );
    const previousDatabase = process.env.SPOON_DB;
    const previousAdminToken = process.env.SPOON_ADMIN_TOKEN;
    process.env.SPOON_DB = path.join(directory, "spoon.db");
    process.env.SPOON_ADMIN_TOKEN = "cli-integration-admin";
    const client = new SpoonClient(
      StdioTransport.spawn("cargo", ["run", "--quiet", "-p", "spoon-server"]),
      { adminToken: "cli-integration-admin" },
    );

    try {
      const teacher = new ReusableLessonTeacher();

      const taught = await runCycle(client, "what is double 7?", teacher);
      const reused = await runCycle(client, "please double 8", teacher);

      assert.equal(taught.answer, 14);
      assert.equal(taught.disposition, "provisional");
      assert.equal(reused.answer, 16);
      assert.equal(reused.disposition, "provisional");
      assert.equal(teacher.calls.length, 1);
      assert.equal(teacher.validationCalls, 1);
    } finally {
      client.close();
      if (previousDatabase === undefined) delete process.env.SPOON_DB;
      else process.env.SPOON_DB = previousDatabase;
      if (previousAdminToken === undefined)
        delete process.env.SPOON_ADMIN_TOKEN;
      else process.env.SPOON_ADMIN_TOKEN = previousAdminToken;
      await rm(directory, { recursive: true, force: true });
    }
  },
);

class ReusableLessonTeacher implements Pick<
  Teacher,
  "propose" | "validationPipeline"
> {
  readonly calls: TeacherRequest[] = [];
  validationCalls = 0;

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
    this.calls.push(request);
    return {
      content: {
        proposalKind: "reusable_lesson",
        interpretations: [],
        lesson: {
          primitiveSet: "pure_rpn_v1",
          concepts: [
            {
              key: "double",
              name: "DOUBLE",
              description: "Multiply any numeric input by two",
              mutability: "procedural",
            },
          ],
          relationships: [],
          procedures: [
            {
              key: "double-procedure",
              name: "DOUBLE",
              concept: { kind: "new_concept", key: "double" },
              parameters: [{ name: "x", description: "numeric input" }],
              body: {
                instructions: [
                  { op: "load_parameter", name: "x" },
                  { op: "push_literal", value: 2 },
                  { op: "multiply" },
                ],
              },
              contract: {
                requires: [],
                promises: [
                  {
                    description: "result is twice x",
                    check: {
                      instructions: [
                        { op: "load_result" },
                        { op: "load_parameter", name: "x" },
                        { op: "push_literal", value: 2 },
                        { op: "multiply" },
                        { op: "equal" },
                      ],
                    },
                  },
                ],
                failsWhen: [],
              },
            },
          ],
          invocation: {
            procedureKey: "double-procedure",
            inputs: [{ name: "x", value: 7 }],
          },
        },
        procedure: null,
        answer: 14,
        abstainReason: null,
      },
      source: "human:fake-procedure",
      status: "unverified",
      provenance: {
        provider: "human",
        teacher: "human:fake-procedure",
        requestId: "procedure-request-1",
        requestHash: fingerprintTeacherRequest(request),
        generatedAt: "2026-08-22T00:00:00.000Z",
        situation: request.situation,
        ...(request.specificQuestion === undefined
          ? {}
          : { specificQuestion: request.specificQuestion }),
      },
    };
  }

  validationPipeline(): ProposalValidationPipeline {
    this.validationCalls += 1;
    return new ProposalValidationPipeline();
  }
}
