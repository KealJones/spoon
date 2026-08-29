import assert from "node:assert/strict";
import test from "node:test";

import {
  fingerprintIntentRequest,
  OllamaLanguageInterpreter,
  type EngineRequest,
  type InterpretationProposal,
  type JsonValue,
} from "../src/index.js";

const schema = {
  type: "object",
  additionalProperties: false,
  properties: {
    candidates: { type: "array" },
    selected: { type: ["integer", "null"] },
    disposition: { enum: ["execute", "clarify", "abstain"] },
  },
  required: ["candidates", "selected", "disposition"],
};

const request: EngineRequest = {
  situation: "please double 7",
  tokenStream: {
    document: { text: "please double 7", normalization: "nfkc" },
    tokens: [
      { kind: "word", span: { start_byte: 0, end_byte: 6 } },
      { kind: "word", span: { start_byte: 7, end_byte: 13 } },
      { kind: "number", span: { start_byte: 14, end_byte: 15 } },
    ],
  },
  context: {
    candidates: [
      {
        alias: "candidate_0",
        procedure: { name: "double", slots: [{ name: "x" }] },
      },
    ],
  },
  desiredOutput: schema,
};

const proposal: InterpretationProposal = {
  candidates: [
    {
      name: "candidate_0",
      confidence: 0.98,
      scope: "CurrentTurn",
      sourceTokens: [{ startToken: 0, endToken: 3 }],
      slots: [
        {
          name: "x",
          confidence: 0.99,
          sourceTokens: [{ startToken: 2, endToken: 3 }],
        },
      ],
      ambiguities: [],
    },
  ],
  selected: 0,
  disposition: "execute",
};

test("Ollama interpreter sends the Engine request with strict JSON output", async () => {
  let url = "";
  let init: RequestInit | undefined;
  const interpreter = new OllamaLanguageInterpreter({
    model: "qwen-test",
    baseUrl: "http://ollama.test/",
    idFactory: () => "request-1",
    now: () => new Date("2026-08-23T00:00:00.000Z"),
    fetch: async (nextUrl, nextInit) => {
      url = String(nextUrl);
      init = nextInit;
      return new Response(
        JSON.stringify({ response: JSON.stringify(proposal) }),
        { status: 200 },
      );
    },
  });

  const result = await interpreter.interpret(request);
  const body = JSON.parse(String(init?.body)) as Record<string, unknown>;

  assert.equal(url, "http://ollama.test/api/generate");
  assert.equal(init?.method, "POST");
  assert.deepEqual(body.model, "qwen-test");
  assert.equal(body.stream, true);
  assert.deepEqual(body.format, schema);
  assert.match(String(body.prompt), /please double 7/);
  assert.match(String(body.prompt), /candidate_0: double; required slots: x/);
  assert.deepEqual(result.content, proposal);
  assert.deepEqual(result.rawContent, proposal);
  assert.equal(result.source, "ollama:qwen-test");
  assert.equal(result.status, "unverified");
  assert.deepEqual(result.provenance, {
    provider: "ollama",
    model: "qwen-test",
    requestId: "request-1",
    generatedAt: "2026-08-23T00:00:00.000Z",
    requestHash: fingerprintIntentRequest(request),
  });
});

test("Ollama interpreter canonicalizes an unambiguous selected index", async () => {
  const interpreter = new OllamaLanguageInterpreter({
    fetch: async () =>
      new Response(
        JSON.stringify({
          response: JSON.stringify({ ...proposal, selected: null }),
        }),
        { status: 200 },
      ),
  });

  const result = await interpreter.interpret(request);
  assert.equal(result.content.selected, 0);
});

test("reconsideration repairs the last procedure without asking the model", async () => {
  const reconsiderationRequest: EngineRequest = {
    situation: "Are you sure?",
    tokenStream: {
      document: {
        text: "Are you sure?",
        normalization: "nfkc",
      },
      tokens: Array.from({ length: 4 }, (_, index) => ({
        kind: "word",
        span: { start_byte: index, end_byte: index + 1 },
      })),
    },
    context: {
      priorTurns: [
        {
          alias: "turn_0",
          situation: 'How many "r"s are in Strawberry?',
          succeeded: true,
          answer: 3,
          actionKind: "procedure",
        },
      ],
      reconsideration: {
        candidateProcedure: "procedure-count",
        previousSituation: 'How many "r"s are in Strawberry?',
        previousAnswer: 3,
        previousInputs: ["Strawberry", "r"],
      },
      candidates: [
        {
          alias: "candidate_0",
          procedure: {
            name: "count exact occurrences",
            slots: [{ name: "text" }, { name: "target" }],
          },
        },
      ],
      literalCandidates: [],
    },
    desiredOutput: schema,
  };

  const interpreter = new OllamaLanguageInterpreter({
    fetch: async () => {
      throw new Error(
        "the model should not be called for a deterministic repair",
      );
    },
    idFactory: () => "reconsideration-1",
    now: () => new Date("2026-08-23T00:00:00.000Z"),
  });

  const result = await interpreter.interpret(reconsiderationRequest);
  assert.equal(result.source, "spoon:reconsideration");
  assert.equal(result.provenance.provider, "spoon");
  assert.equal(result.content.disposition, "execute");
  assert.equal(result.content.selected, 0);
  assert.deepEqual(result.content.candidates[0]?.slots, [
    {
      name: "text",
      confidence: 1,
      sourceTokens: [],
      inferredValue: "Strawberry",
    },
    {
      name: "target",
      confidence: 1,
      sourceTokens: [],
      inferredValue: "r",
    },
  ]);
});

test("bare incorrectness is not converted into a deterministic procedure replay", async () => {
  const requestContext = request.context as Record<string, JsonValue>;
  let modelCalled = false;
  const interpreter = new OllamaLanguageInterpreter({
    fetch: async () => {
      modelCalled = true;
      return new Response(
        JSON.stringify({ response: JSON.stringify(proposal) }),
        {
          status: 200,
        },
      );
    },
  });
  const result = await interpreter.interpret({
    ...request,
    situation: "incorrect",
    context: {
      ...requestContext,
      priorTurns: [
        {
          alias: "turn_0",
          situation: "please double 7",
          succeeded: true,
          answer: 14,
          actionKind: "procedure",
        },
      ],
      reconsideration: {
        candidateProcedure: "procedure-double",
        previousInputs: [7],
      },
      literalCandidates: [],
    },
  });
  assert.equal(modelCalled, true);
  assert.equal(result.source, "ollama:qwen3:30b-a3b");
});

test("Ollama interpreter drops contradictory inferred values from grounded slots", async () => {
  const interpreter = new OllamaLanguageInterpreter({
    fetch: async () =>
      new Response(
        JSON.stringify({
          response: JSON.stringify({
            ...proposal,
            candidates: [
              {
                ...proposal.candidates[0],
                slots: [
                  {
                    ...proposal.candidates[0]!.slots[0]!,
                    inferredValue: false,
                  },
                ],
              },
            ],
          }),
        }),
        { status: 200 },
      ),
  });

  const result = await interpreter.interpret(request);
  assert.equal(
    "inferredValue" in result.content.candidates[0]!.slots[0]!,
    false,
  );
});

test("Ollama interpreter converts execute-plus-ambiguity into clarification", async () => {
  const interpreter = new OllamaLanguageInterpreter({
    fetch: async () =>
      new Response(
        JSON.stringify({
          response: JSON.stringify({
            ...proposal,
            candidates: [
              {
                ...proposal.candidates[0],
                ambiguities: ["the target is not certain"],
              },
            ],
          }),
        }),
        { status: 200 },
      ),
  });

  const result = await interpreter.interpret(request);
  assert.equal(result.content.disposition, "clarify");
  assert.equal(result.content.selected, null);
  assert.deepEqual(result.content.candidates[0]?.ambiguities, [
    "the target is not certain",
  ]);
});

test("Ollama interpreter repairs constrained-output filler without inventing intent", async () => {
  const filler = {
    ...proposal,
    disposition: "clarify" as const,
    selected: null,
    candidates: [
      {
        ...proposal.candidates[0]!,
        slots: [
          proposal.candidates[0]!.slots[0]!,
          {
            ...proposal.candidates[0]!.slots[0]!,
            sourceTokens: [{ startToken: 0, endToken: 1 }],
          },
        ],
        ambiguities: [request.situation],
      },
    ],
  };
  const interpreter = new OllamaLanguageInterpreter({
    fetch: async () =>
      new Response(JSON.stringify({ response: JSON.stringify(filler) }), {
        status: 200,
      }),
  });

  const result = await interpreter.interpret(request);
  assert.equal(result.content.disposition, "execute");
  assert.equal(result.content.selected, 0);
  assert.deepEqual(result.content.candidates[0]?.slots, [
    proposal.candidates[0]!.slots[0]!,
  ]);
  assert.deepEqual(result.content.candidates[0]?.ambiguities, []);
  assert.deepEqual(result.rawContent, filler);
});

test("intent request fingerprints are stable across object key order", () => {
  const reordered = {
    ...request,
    context: {
      candidates: [
        {
          procedure: { slots: [{ name: "x" }], name: "double" },
          alias: "candidate_0",
        },
      ],
    },
  } satisfies EngineRequest;

  assert.equal(
    fingerprintIntentRequest(request),
    fingerprintIntentRequest(reordered),
  );
});

test("Ollama interpreter turns transport and provider failures into bounded errors", async () => {
  const transportFailure = new OllamaLanguageInterpreter({
    fetch: async () => {
      throw new Error("connection refused");
    },
  });
  await assert.rejects(
    transportFailure.interpret(request),
    /ollama: generate API request failed/,
  );

  const providerFailure = new OllamaLanguageInterpreter({
    fetch: async () =>
      new Response(JSON.stringify({ error: "model not found" }), {
        status: 404,
        statusText: "Not Found",
      }),
  });
  await assert.rejects(
    providerFailure.interpret(request),
    /ollama: generate API returned 404: model not found/,
  );
});

test("Ollama interpreter rejects malformed structured output", async () => {
  const interpreter = new OllamaLanguageInterpreter({
    fetch: async () =>
      new Response(JSON.stringify({ response: "not an interpretation" }), {
        status: 200,
      }),
  });

  await assert.rejects(
    interpreter.interpret(request),
    /ollama: provider response was not valid JSON/,
  );
});
