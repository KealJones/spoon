import assert from "node:assert/strict";
import test from "node:test";

import {
  CursorLanguageInterpreter,
  type EngineRequest,
  type InterpretationProposal,
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

test("Cursor interpreter uses print-mode ask and returns unverified intent", async () => {
  let command = "";
  let args: string[] = [];
  const interpreter = new CursorLanguageInterpreter({
    model: "cursor-grok-4.6-high",
    runner: async (invocation) => {
      command = invocation.command;
      args = invocation.args;
      return {
        exitCode: 0,
        stdout: JSON.stringify({
          type: "result",
          subtype: "success",
          is_error: false,
          result: JSON.stringify(proposal),
        }),
        stderr: "",
      };
    },
    now: () => new Date("2026-08-23T00:00:00.000Z"),
    idFactory: () => "cursor-intent-1",
  });

  const result = await interpreter.interpret(request);

  assert.equal(command, "agent");
  assert.deepEqual(args.slice(0, 6), [
    "-p",
    "--mode",
    "ask",
    "--output-format",
    "json",
    "--trust",
  ]);
  assert.deepEqual(args.slice(6, 8), ["--model", "cursor-grok-4.6-high"]);
  assert.match(args.at(-1) ?? "", /bounded language interpreter/);
  assert.deepEqual(result.content, proposal);
  assert.equal(result.source, "cursor:cursor-grok-4.6-high");
  assert.equal(result.provenance.provider, "cursor");
  assert.equal(result.status, "unverified");
});

test("Cursor interpreter turns command failures into bounded errors", async () => {
  const interpreter = new CursorLanguageInterpreter({
    runner: async () => ({
      exitCode: 1,
      stdout: "",
      stderr: "not logged in",
    }),
  });
  await assert.rejects(
    interpreter.interpret(request),
    /cursor: command exited with status 1: not logged in/,
  );
});
