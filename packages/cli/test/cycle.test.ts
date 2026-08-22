import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  EkgClient,
  StdioTransport,
  type CycleProgress,
  type RpcTransport,
} from "@ekg/sdk";
import {
  ProposalValidationPipeline,
  fingerprintTeacherRequest,
  type Teacher,
  type TeacherProposal,
  type TeacherRequest,
} from "@ekg/teacher";

import { runCycle } from "../src/cycle.js";

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
  const client = new EkgClient(transport);
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

  const outcome = await runCycle(new EkgClient(transport), "unknown", teacher);

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

test(
  "fake teacher teaches a procedure once and the Rust cycle reuses it locally",
  { timeout: 30_000 },
  async () => {
    const directory = await mkdtemp(
      path.join(os.tmpdir(), "ekg-teacher-cycle-"),
    );
    const previousDatabase = process.env.EKG_DB;
    process.env.EKG_DB = path.join(directory, "ekg.db");
    const client = new EkgClient(
      StdioTransport.spawn("cargo", ["run", "--quiet", "-p", "ekg-server"]),
    );

    try {
      const concept = await client.createConcept<{ id: string }>({
        name: "DOUBLE",
        mutability: "Definitional",
      });
      const teacher = new ProcedureTeacher(concept.id);

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
      if (previousDatabase === undefined) delete process.env.EKG_DB;
      else process.env.EKG_DB = previousDatabase;
      await rm(directory, { recursive: true, force: true });
    }
  },
);

class ProcedureTeacher implements Pick<
  Teacher,
  "propose" | "validationPipeline"
> {
  readonly calls: TeacherRequest[] = [];
  validationCalls = 0;

  constructor(private readonly conceptId: string) {}

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
    this.calls.push(request);
    const now = Math.floor(Date.now() / 1_000);
    return {
      content: {
        interpretations: [
          {
            concept: { id: this.conceptId },
            weight: 1,
            inputs: [{ name: "x", value: 7 }],
          },
        ],
        procedure: JSON.stringify({
          id: randomUUID(),
          name: "DOUBLE",
          params: [{ name: "x", description: null }],
          body: {
            BinOp: {
              op: "Mul",
              left: { Var: "x" },
              right: { Literal: 2 },
            },
          },
          contract: {
            requires: [],
            promises: [],
            fails_when: [],
            costs: { operations: 1, description: "one multiplication" },
            confidence: {
              support_count: 0,
              contradiction_count: 0,
              scope: [],
              sources: [],
              last_tested: null,
            },
          },
          test_cases: [],
          concept: this.conceptId,
          version: 1,
          lifecycle: "Provisional",
          created_at: now,
          updated_at: now,
        }),
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
