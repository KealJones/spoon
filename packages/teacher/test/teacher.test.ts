import assert from "node:assert/strict";
import test from "node:test";

import {
  ClaudeTeacher,
  CursorTeacher,
  HumanTeacher,
  normalizeClaudeSchema,
  OllamaTeacher,
  OpenAITeacher,
  ProposalValidationPipeline,
  REUSABLE_LESSON_PROTOCOL,
  SourceReliabilityTracker,
  TEACHER_SYSTEM_PROMPT,
  TeacherError,
  fingerprintTeacherRequest,
  validateSchema,
  buildTeacherPrompt,
  type CommandInvocation,
  type ProposalSchema,
  type TeacherProposal,
  type TeacherRequest,
} from "../src/index.js";

test("teacher protocol teaches complete bounded reusable lessons", () => {
  assert.equal(REUSABLE_LESSON_PROTOCOL.primitiveSet, "pure_expr_v2");
  assert.ok(REUSABLE_LESSON_PROTOCOL.expressionKinds.includes("binary"));
  assert.ok(
    !(REUSABLE_LESSON_PROTOCOL.expressionKinds as readonly string[]).includes(
      "call",
    ),
  );

  const prompt = `${TEACHER_SYSTEM_PROMPT}\n${buildTeacherPrompt(request)}`;
  assert.match(prompt, /prefer a reusable lesson/i);
  assert.match(prompt, /pure_expr_v2/);
  assert.match(prompt, /procedureKey/);
  assert.match(prompt, /trusted sensor primitive/i);
  assert.match(prompt, /never invent ids, timestamps, lifecycle/i);
  assert.ok(
    REUSABLE_LESSON_PROTOCOL.teachingFacets.includes(
      "language and terminology",
    ),
  );
  assert.ok(
    REUSABLE_LESSON_PROTOCOL.teachingFacets.includes(
      "user intent and requested outcome",
    ),
  );
  assert.match(prompt, /Teaching checklist/);
  assert.match(prompt, /Language: identify the introduced terms/i);
  assert.match(prompt, /Meaning: state the definition/i);
  assert.match(prompt, /Intent: identify what the user wants/i);
  assert.match(prompt, /most complete safe structured lesson/i);
  assert.match(prompt, /defeasible_general/);
  assert.match(prompt, /lesson:<procedure-key>/);
  assert.match(prompt, /one to four focused procedures/i);
  assert.match(prompt, /programmatic requests over user-supplied values/i);
  assert.match(
    prompt,
    /arr\[0\]\.name should teach the reusable path operation/i,
  );
  assert.match(prompt, /path_get_optional/i);
  assert.match(prompt, /obj\.field/i);
  assert.match(prompt, /arr\[i\]/);
});

const schema: ProposalSchema = {
  type: "object",
  properties: {
    lesson: { type: "string", minLength: 1 },
    confidence: { type: "number", minimum: 0, maximum: 1 },
    tags: { type: "array", items: { type: "string" } },
  },
  required: ["lesson", "confidence"],
  additionalProperties: false,
};

const request: TeacherRequest = {
  situation: "A user asks to double 7",
  context: {
    concepts: [{ id: "double", name: "DOUBLE" }],
    relationships: [],
    procedures: [],
  },
  specificQuestion: "What reusable lesson should Spoon learn?",
  desiredOutput: schema,
};

function proposal(content: TeacherProposal["content"]): TeacherProposal {
  return {
    content,
    source: "claude:test",
    status: "unverified",
    provenance: {
      provider: "claude",
      teacher: "claude:test",
      model: "test",
      requestId: "request-1",
      generatedAt: "2026-08-22T00:00:00.000Z",
      requestHash: fingerprintTeacherRequest(request),
      situation: request.situation,
      specificQuestion: request.specificQuestion,
    },
  };
}

test("validation rejects schema-invalid output and records provenance", async () => {
  const tracker = new SourceReliabilityTracker();
  const pipeline = new ProposalValidationPipeline({
    reliabilityTracker: tracker,
  });
  const result = await pipeline.validate(
    proposal({ lesson: "Double by multiplying by two", confidence: 2 }),
    request,
  );

  assert.equal(result.status, "rejected");
  assert.equal(result.validation.checks[0]?.validator, "proposal-schema");
  assert.match(result.validation.checks[0]?.reason ?? "", /maximum/);
  assert.equal(result.provenance.teacher, "claude:test");
  assert.deepEqual(tracker.get("claude:test"), {
    source: "claude:test",
    total: 1,
    verified: 0,
    rejected: 1,
    provisional: 0,
    score: 1 / 3,
  });
});

test("validation stays provisional without independent verification", async () => {
  const pipeline = new ProposalValidationPipeline();
  const result = await pipeline.validate(
    proposal({
      lesson: "Double by multiplying by two",
      confidence: 0.9,
      tags: ["math"],
    }),
    request,
  );

  assert.equal(result.status, "provisional");
  assert.deepEqual(result.validation.checks, [
    {
      validator: "proposal-schema",
      status: "verified",
      reason: "Proposal content matches the requested schema",
    },
  ]);
});

test("validators can verify or reject a schema-valid proposal", async () => {
  const verifiedPipeline = new ProposalValidationPipeline({
    validators: [
      {
        name: "known-arithmetic",
        validate: () => ({
          status: "verified",
          reason: "Matches executable evidence",
        }),
      },
    ],
  });
  const rejectedPipeline = new ProposalValidationPipeline({
    validators: [
      {
        name: "known-arithmetic",
        validate: () => ({
          status: "rejected",
          reason: "Contradicts executable evidence",
        }),
      },
    ],
  });
  const candidate = proposal({ lesson: "Multiply by two", confidence: 0.9 });

  assert.equal(
    (await verifiedPipeline.validate(candidate, request)).status,
    "verified",
  );
  assert.equal(
    (await rejectedPipeline.validate(candidate, request)).status,
    "rejected",
  );
  assert.equal(candidate.status, "unverified");
});

test("reliability is tracked independently per source with a conservative prior", () => {
  const tracker = new SourceReliabilityTracker();
  tracker.record("claude:sonnet", "verified");
  tracker.record("claude:sonnet", "provisional");
  tracker.record("ollama:qwen", "rejected");

  assert.deepEqual(tracker.get("claude:sonnet"), {
    source: "claude:sonnet",
    total: 2,
    verified: 1,
    rejected: 0,
    provisional: 1,
    score: 0.625,
  });
  assert.deepEqual(tracker.get("ollama:qwen"), {
    source: "ollama:qwen",
    total: 1,
    verified: 0,
    rejected: 1,
    provisional: 0,
    score: 1 / 3,
  });
  assert.equal(tracker.get("human:cli").score, 0.5);
});

test("Claude adapter uses print-mode structured output and returns unverified content", async () => {
  let invocation: CommandInvocation | undefined;
  const tracker = new SourceReliabilityTracker();
  const teacher = new ClaudeTeacher({
    model: "sonnet",
    reliabilityTracker: tracker,
    runner: async (nextInvocation) => {
      invocation = nextInvocation;
      return {
        exitCode: 0,
        stdout: JSON.stringify({
          type: "result",
          subtype: "success",
          structured_output: { lesson: "Multiply by two", confidence: 0.91 },
        }),
        stderr: "",
      };
    },
    now: () => new Date("2026-08-22T00:00:00.000Z"),
    idFactory: () => "claude-request",
  });

  const result = await teacher.propose(request);

  assert.equal(invocation?.command, "claude");
  assert.deepEqual(invocation?.args.slice(0, 7), [
    "-p",
    "--output-format",
    "json",
    "--json-schema",
    JSON.stringify(schema),
    "--tools",
    "",
  ]);
  const systemPromptIndex = invocation?.args.indexOf("--system-prompt") ?? -1;
  assert.notEqual(systemPromptIndex, -1);
  assert.match(
    invocation?.args[systemPromptIndex + 1] ?? "",
    /never automatically accepted/i,
  );
  assert.match(
    invocation?.args.at(-1) ?? "",
    /Teaching checklist.*Language: identify/s,
  );
  assert.deepEqual(result.content, {
    lesson: "Multiply by two",
    confidence: 0.91,
  });
  assert.equal(result.status, "unverified");
  assert.equal(result.source, "claude:sonnet");
  assert.equal(result.provenance.requestId, "claude-request");
  assert.equal(teacher.reliability().score, 0.5);
});

test("Cursor adapter uses print-mode ask and returns unverified content", async () => {
  let invocation: CommandInvocation | undefined;
  const teacher = new CursorTeacher({
    model: "composer-2",
    runner: async (nextInvocation) => {
      invocation = nextInvocation;
      return {
        exitCode: 0,
        stdout: JSON.stringify({
          type: "result",
          subtype: "success",
          is_error: false,
          result: JSON.stringify({
            lesson: "Multiply by two",
            confidence: 0.91,
          }),
        }),
        stderr: "",
      };
    },
    now: () => new Date("2026-08-22T00:00:00.000Z"),
    idFactory: () => "cursor-request",
  });

  const result = await teacher.propose(request);

  assert.equal(invocation?.command, "agent");
  assert.deepEqual(invocation?.args.slice(0, 6), [
    "-p",
    "--mode",
    "ask",
    "--output-format",
    "json",
    "--trust",
  ]);
  assert.deepEqual(invocation?.args.slice(6, 8), ["--model", "composer-2"]);
  assert.match(invocation?.args.at(-1) ?? "", /never automatically accepted/i);
  assert.match(invocation?.args.at(-1) ?? "", /Return only the requested JSON/);
  assert.deepEqual(result.content, {
    lesson: "Multiply by two",
    confidence: 0.91,
  });
  assert.equal(result.status, "unverified");
  assert.equal(result.source, "cursor:composer-2");
  assert.equal(result.provenance.provider, "cursor");
  assert.equal(result.provenance.requestId, "cursor-request");
});

test("Claude schema normalization lowers union types for strict AJV", () => {
  assert.deepEqual(
    normalizeClaudeSchema({
      type: "object",
      properties: {
        answer: { type: ["null", "number"] },
      },
    }),
    {
      type: "object",
      properties: {
        answer: {
          anyOf: [{ type: "null" }, { type: "number" }],
        },
      },
    },
  );
});

test("OpenAI adapter sends Responses API strict structured-output request", async () => {
  let url = "";
  let init: RequestInit | undefined;
  const teacher = new OpenAITeacher({
    apiKey: "test-key",
    model: "gpt-test",
    fetch: async (nextUrl, nextInit) => {
      url = String(nextUrl);
      init = nextInit;
      return new Response(
        JSON.stringify({
          id: "resp_123",
          output: [
            {
              type: "message",
              content: [
                {
                  type: "output_text",
                  text: JSON.stringify({
                    lesson: "Multiply by two",
                    confidence: 0.88,
                  }),
                },
              ],
            },
          ],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      );
    },
    now: () => new Date("2026-08-22T00:00:00.000Z"),
    idFactory: () => "openai-request",
  });

  const result = await teacher.propose(request);
  const body = JSON.parse(String(init?.body)) as Record<string, unknown>;

  assert.equal(url, "https://api.openai.com/v1/responses");
  assert.equal(
    new Headers(init?.headers).get("authorization"),
    "Bearer test-key",
  );
  assert.equal(body.model, "gpt-test");
  assert.deepEqual(body.text, {
    format: {
      type: "json_schema",
      name: "spoon_teacher_proposal",
      strict: true,
      schema,
    },
  });
  assert.equal(result.status, "unverified");
  assert.equal(result.source, "openai:gpt-test");
  assert.equal(result.provenance.providerRequestId, "resp_123");
});

test("Ollama adapter streams generate with JSON schema", async () => {
  let url = "";
  let init: RequestInit | undefined;
  const teacher = new OllamaTeacher({
    model: "qwen3:8b",
    baseUrl: "http://ollama.test/",
    fetch: async (nextUrl, nextInit) => {
      url = String(nextUrl);
      init = nextInit;
      return new Response(
        [
          JSON.stringify({ response: '{"lesson":"', done: false }),
          JSON.stringify({
            response: 'Multiply by two","confidence":0.8}',
            done: true,
          }),
          "",
        ].join("\n"),
        { status: 200 },
      );
    },
  });

  const result = await teacher.propose(request);
  const body = JSON.parse(String(init?.body)) as Record<string, unknown>;

  assert.equal(url, "http://ollama.test/api/generate");
  assert.equal(body.model, "qwen3:8b");
  assert.equal(body.stream, true);
  assert.equal(body.think, false);
  assert.deepEqual(body.format, schema);
  assert.equal(result.status, "unverified");
  assert.equal(result.source, "ollama:qwen3:8b");
  assert.deepEqual(result.content, {
    lesson: "Multiply by two",
    confidence: 0.8,
  });
});

test("Ollama adapter reads JSON from thinking when response is empty", async () => {
  const teacher = new OllamaTeacher({
    model: "qwen3.8:27b",
    fetch: async () =>
      new Response(
        [
          JSON.stringify({
            thinking: '{\n  "lesson":"',
            response: "",
            done: false,
          }),
          JSON.stringify({
            thinking: 'Multiply by two","confidence":0.8}',
            response: "",
            done: true,
          }),
          "",
        ].join("\n"),
        { status: 200 },
      ),
  });

  const result = await teacher.propose(request);
  assert.deepEqual(result.content, {
    lesson: "Multiply by two",
    confidence: 0.8,
  });
});

test("Ollama teacher defaults to the language interpreter Qwen model", async () => {
  let url = "";
  let init: RequestInit | undefined;
  const teacher = new OllamaTeacher({
    baseUrl: "http://ollama.test/",
    fetch: async (nextUrl, nextInit) => {
      url = String(nextUrl);
      init = nextInit;
      return new Response(
        JSON.stringify({
          model: "qwen2.5:1.5b",
          created_at: "2026-08-22T00:00:00Z",
          response: JSON.stringify({
            lesson: "Multiply by two",
            confidence: 0.8,
          }),
          done: true,
        }),
        { status: 200 },
      );
    },
  });

  const result = await teacher.propose(request);
  const body = JSON.parse(String(init?.body)) as Record<string, unknown>;

  assert.equal(url, "http://ollama.test/api/generate");
  assert.equal(body.model, "qwen2.5:1.5b");
  assert.equal(body.stream, true);
  assert.deepEqual(body.format, schema);
  assert.equal(result.status, "unverified");
  assert.equal(result.source, "ollama:qwen2.5:1.5b");
  assert.equal(result.provenance.provider, "ollama");
});

test("human adapter accepts injected structured input and never self-verifies", async () => {
  let displayedPrompt = "";
  const teacher = new HumanTeacher({
    name: "reviewer",
    prompt: async (message) => {
      displayedPrompt = message;
      return { lesson: "Multiply by two", confidence: 1 };
    },
  });

  const result = await teacher.propose(request);

  assert.match(displayedPrompt, /A user asks to double 7/);
  assert.match(displayedPrompt, /What reusable lesson/);
  assert.equal(result.status, "unverified");
  assert.equal(result.source, "human:reviewer");
});

test("provider failures and malformed outputs are explicit", async () => {
  const failedClaude = new ClaudeTeacher({
    runner: async () => ({
      exitCode: 2,
      stdout: "",
      stderr: "not authenticated",
    }),
  });
  const malformedOllama = new OllamaTeacher({
    fetch: async () =>
      new Response(JSON.stringify({ response: "not json" }), { status: 200 }),
  });

  await assert.rejects(
    () => failedClaude.propose(request),
    /not authenticated/,
  );
  await assert.rejects(() => malformedOllama.propose(request), /valid JSON/);
});

test("validation binds the proposal envelope and provenance to the request", async () => {
  const customValidatorCalls: string[] = [];
  const pipeline = new ProposalValidationPipeline({
    validators: [
      {
        name: "must-not-run",
        validate: () => {
          customValidatorCalls.push("called");
          return { status: "verified", reason: "unexpected" };
        },
      },
    ],
  });
  const cases: Array<[string, (candidate: TeacherProposal) => void]> = [
    [
      "runtime status",
      (candidate) => {
        (candidate as { status: string }).status = "verified";
      },
    ],
    [
      "empty source",
      (candidate) => {
        candidate.source = " ";
        candidate.provenance.teacher = " ";
      },
    ],
    [
      "source without an identity",
      (candidate) => {
        candidate.source = "claude:";
        candidate.provenance.teacher = "claude:";
      },
    ],
    [
      "teacher/source mismatch",
      (candidate) => {
        candidate.provenance.teacher = "claude:other";
      },
    ],
    [
      "provider/source mismatch",
      (candidate) => {
        candidate.provenance.provider = "openai";
      },
    ],
    [
      "situation mismatch",
      (candidate) => {
        candidate.provenance.situation = "A different situation";
      },
    ],
    [
      "question mismatch",
      (candidate) => {
        candidate.provenance.specificQuestion = "A different question";
      },
    ],
    [
      "empty request id",
      (candidate) => {
        candidate.provenance.requestId = "";
      },
    ],
    [
      "invalid timestamp",
      (candidate) => {
        candidate.provenance.generatedAt = "not-a-date";
      },
    ],
    [
      "non-canonical timestamp",
      (candidate) => {
        candidate.provenance.generatedAt = "2026-08-22";
      },
    ],
    [
      "model/source mismatch",
      (candidate) => {
        candidate.provenance.model = "other";
      },
    ],
    [
      "empty provider request id",
      (candidate) => {
        candidate.provenance.providerRequestId = " ";
      },
    ],
    [
      "request fingerprint mismatch",
      (candidate) => {
        candidate.provenance.requestHash = "sha256:not-the-request";
      },
    ],
  ];

  for (const [name, mutate] of cases) {
    const candidate = proposal({ lesson: "Multiply by two", confidence: 1 });
    mutate(candidate);
    const result = await pipeline.validate(candidate, request);
    assert.equal(result.status, "rejected", name);
    assert.equal(result.validation.checks[0]?.validator, "proposal-envelope");
    assert.match(result.validation.checks[0]?.reason ?? "", /proposal/i);
  }
  assert.deepEqual(customValidatorCalls, []);
});

test("request fingerprints bind context and desired schema semantically", async () => {
  const candidate = proposal({ lesson: "Multiply by two", confidence: 1 });
  const changedContext: TeacherRequest = {
    ...request,
    context: { ...request.context, concepts: [{ id: "triple" }] },
  };
  const changedSchema: TeacherRequest = {
    ...request,
    desiredOutput: {
      ...request.desiredOutput,
      properties: {
        ...request.desiredOutput.properties,
        extra: { type: "string" },
      },
    },
  };

  assert.notEqual(
    fingerprintTeacherRequest(request),
    fingerprintTeacherRequest(changedContext),
  );
  assert.notEqual(
    fingerprintTeacherRequest(request),
    fingerprintTeacherRequest(changedSchema),
  );
  for (const changedRequest of [changedContext, changedSchema]) {
    const result = await new ProposalValidationPipeline().validate(
      candidate,
      changedRequest,
    );
    assert.equal(result.status, "rejected");
    assert.match(result.validation.checks[0]?.reason ?? "", /fingerprint/i);
  }

  const reordered: TeacherRequest = {
    situation: request.situation,
    context: {
      procedures: [],
      relationships: [],
      concepts: [{ name: "DOUBLE", id: "double" }],
    },
    specificQuestion: request.specificQuestion,
    desiredOutput: {
      additionalProperties: false,
      required: ["lesson", "confidence"],
      properties: {
        tags: { items: { type: "string" }, type: "array" },
        confidence: { maximum: 1, minimum: 0, type: "number" },
        lesson: { minLength: 1, type: "string" },
      },
      type: "object",
    },
  };
  assert.equal(
    fingerprintTeacherRequest(reordered),
    fingerprintTeacherRequest(request),
  );
});

test("validation requires exact optional-question provenance binding", async () => {
  const candidate = proposal({ lesson: "Multiply by two", confidence: 1 });
  const requestWithoutQuestion = { ...request, specificQuestion: undefined };
  const result = await new ProposalValidationPipeline().validate(
    candidate,
    requestWithoutQuestion,
  );

  assert.equal(result.status, "rejected");
  assert.match(result.validation.checks[0]?.reason ?? "", /question/i);
});

test("teacher-created validation pipelines share the teacher reliability state", async () => {
  const teacher = new HumanTeacher({
    name: "connected",
    prompt: async () => ({ lesson: "Multiply by two", confidence: 1 }),
    now: () => new Date("2026-08-22T00:00:00.000Z"),
    idFactory: () => "connected-request",
  });
  const candidate = await teacher.propose(request);
  const pipeline = teacher.validationPipeline({
    validators: [
      {
        name: "executable-check",
        validate: () => ({
          status: "verified",
          reason: "Matches executable evidence",
        }),
      },
    ],
  });

  const result = await pipeline.validate(candidate, request);

  assert.equal(result.status, "verified");
  assert.deepEqual(teacher.reliability(), {
    source: "human:connected",
    total: 1,
    verified: 1,
    rejected: 0,
    provisional: 0,
    score: 2 / 3,
  });
});

test("JSON schema equality is structural and object key order independent", () => {
  assert.deepEqual(
    validateSchema(
      { right: [2, { nested: true }], left: 1 },
      { const: { left: 1, right: [2, { nested: true }] } },
    ),
    [],
  );
  assert.deepEqual(
    validateSchema(
      { second: 2, first: 1 },
      { enum: [{ first: 1, second: 2 }] },
    ),
    [],
  );
  assert.match(
    validateSchema(
      [
        { first: 1, second: 2 },
        { second: 2, first: 1 },
      ],
      { type: "array", uniqueItems: true },
    ).join("; "),
    /unique/,
  );
});

test("required schema properties must be own properties", () => {
  const inherited = Object.create({ lesson: "inherited" }) as {
    lesson: string;
    confidence: number;
  };
  inherited.confidence = 1;

  assert.match(
    validateSchema(inherited, schema).join("; "),
    /\$\.lesson is required/,
  );
});

test("transport and prompt rejections retain provider attribution", async () => {
  const boundaryFailure = new Error("boundary unavailable");
  const teachers = [
    new ClaudeTeacher({
      runner: async () => {
        throw boundaryFailure;
      },
    }),
    new CursorTeacher({
      runner: async () => {
        throw boundaryFailure;
      },
    }),
    new OpenAITeacher({
      apiKey: "test-key",
      model: "gpt-test",
      fetch: async () => {
        throw boundaryFailure;
      },
    }),
    new OllamaTeacher({
      fetch: async () => {
        throw boundaryFailure;
      },
    }),
    new HumanTeacher({
      prompt: async () => {
        throw boundaryFailure;
      },
    }),
  ];
  const providers = ["claude", "cursor", "openai", "ollama", "human"];

  for (const [index, teacher] of teachers.entries()) {
    await assert.rejects(
      () => teacher.propose(request),
      (error: unknown) => {
        assert.ok(error instanceof TeacherError);
        assert.equal(error.provider, providers[index]);
        assert.equal(error.cause, boundaryFailure);
        return true;
      },
    );
  }
});
