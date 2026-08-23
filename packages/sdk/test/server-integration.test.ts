import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  SpoonClient,
  StdioTransport,
  type CycleInput,
  type TeacherProposalWire,
} from "../src/index.js";

const defaultCycleInput = (
  situation: string,
  teacherAllowed: boolean,
): CycleInput => ({
  situation,
  environment: {},
  assumptions: [],
  budget: {
    maxExecSteps: 1_000,
    maxContextItems: 32,
    maxTeacherTurns: 1,
  },
  teacherAllowed,
});

const quotedLetterCountLesson = (situation: string): TeacherProposalWire => ({
  content: {
    proposalKind: "reusable_lesson",
    interpretations: [],
    lesson: {
      primitiveSet: "pure_expr_v2",
      concepts: [
        {
          key: "count-letter-in-text",
          name: "COUNT LETTER IN TEXT",
          description:
            "Count case-insensitive occurrences of one letter in quoted text",
        },
      ],
      relationships: [],
      procedures: [
        {
          key: "count-letter-in-text-procedure",
          name: "COUNT LETTER IN TEXT",
          concept: { kind: "new_concept", key: "count-letter-in-text" },
          parameters: [
            { name: "letter", description: "letter to count" },
            { name: "text", description: "text to inspect" },
          ],
          body: {
            kind: "intrinsic",
            version: 1,
            op: "length",
            args: [
              {
                kind: "filter",
                collection: {
                  kind: "intrinsic",
                  version: 1,
                  op: "text_split",
                  args: [
                    {
                      kind: "intrinsic",
                      version: 1,
                      op: "text_lowercase",
                      args: [{ kind: "parameter", name: "text" }],
                    },
                    { kind: "literal", value: "" },
                  ],
                },
                var: "character",
                predicate: {
                  kind: "binary",
                  op: "equal",
                  left: { kind: "parameter", name: "character" },
                  right: {
                    kind: "intrinsic",
                    version: 1,
                    op: "text_lowercase",
                    args: [{ kind: "parameter", name: "letter" }],
                  },
                },
              },
            ],
          },
          contract: { requires: [], promises: [], failsWhen: [] },
        },
      ],
      invocation: {
        procedureKey: "count-letter-in-text-procedure",
        inputs: [
          { name: "letter", value: "r" },
          { name: "text", value: "strawberry" },
        ],
      },
    },
    procedure: null,
    answer: 3,
    abstainReason: null,
  },
  source: "human:test-quoted-letter-count",
  status: "unverified",
  provenance: {
    provider: "human",
    teacher: "human:test-quoted-letter-count",
    requestId: "quoted-letter-count-1",
    generatedAt: "2026-08-23T00:00:00.000Z",
    situation,
  },
});

const tokenizedWordCountLesson = (
  situation: string,
  text: string,
): TeacherProposalWire => ({
  content: {
    proposalKind: "reusable_lesson",
    interpretations: [],
    lesson: {
      primitiveSet: "pure_expr_v2",
      concepts: [
        {
          key: "count-tokenized-words",
          name: "COUNT TOKENIZED WORDS",
          description:
            "Count word tokens in quoted text using the bounded tokenizer",
        },
      ],
      relationships: [],
      procedures: [
        {
          key: "count-tokenized-words-procedure",
          name: "COUNT TOKENIZED WORDS",
          concept: {
            kind: "new_concept",
            key: "count-tokenized-words",
          },
          parameters: [{ name: "text", description: "text to tokenize" }],
          body: {
            kind: "intrinsic",
            version: 1,
            op: "length",
            args: [
              {
                kind: "filter",
                collection: {
                  kind: "intrinsic",
                  version: 1,
                  op: "text_tokenize",
                  args: [{ kind: "parameter", name: "text" }],
                },
                var: "token",
                predicate: {
                  kind: "binary",
                  op: "equal",
                  left: {
                    kind: "field",
                    object: { kind: "parameter", name: "token" },
                    field: "kind",
                  },
                  right: { kind: "literal", value: "word" },
                },
              },
            ],
          },
          contract: { requires: [], promises: [], failsWhen: [] },
        },
      ],
      invocation: {
        procedureKey: "count-tokenized-words-procedure",
        inputs: [{ name: "text", value: text }],
      },
    },
    procedure: null,
    answer: 3,
    abstainReason: null,
  },
  source: "human:test-tokenized-word-count",
  status: "unverified",
  provenance: {
    provider: "human",
    teacher: "human:test-tokenized-word-count",
    requestId: "tokenized-word-count-1",
    generatedAt: "2026-08-23T00:00:00.000Z",
    situation,
  },
});

const jsonPointerDefaultLesson = (
  situation: string,
  document: string,
): TeacherProposalWire => ({
  content: {
    proposalKind: "reusable_lesson",
    interpretations: [],
    lesson: {
      primitiveSet: "pure_expr_v2",
      concepts: [
        {
          key: "json-pointer-default",
          name: "JSON POINTER DEFAULT",
          description:
            "Read an optional JSON Pointer and return a bounded default when the field is absent",
        },
      ],
      relationships: [],
      procedures: [
        {
          key: "json-pointer-default-procedure",
          name: "JSON POINTER DEFAULT",
          concept: {
            kind: "new_concept",
            key: "json-pointer-default",
          },
          parameters: [
            {
              name: "document",
              description: "JSON document text to inspect",
            },
          ],
          body: {
            kind: "intrinsic",
            version: 1,
            op: "coalesce",
            args: [
              {
                kind: "intrinsic",
                version: 1,
                op: "json_pointer_get_optional",
                args: [
                  {
                    kind: "intrinsic",
                    version: 1,
                    op: "json_parse",
                    args: [{ kind: "parameter", name: "document" }],
                  },
                  { kind: "literal", value: "/answer" },
                ],
              },
              { kind: "literal", value: "unknown" },
            ],
          },
          contract: { requires: [], promises: [], failsWhen: [] },
        },
      ],
      invocation: {
        procedureKey: "json-pointer-default-procedure",
        inputs: [{ name: "document", value: document }],
      },
    },
    procedure: null,
    answer: "unknown",
    abstainReason: null,
  },
  source: "human:test-json-pointer-default",
  status: "unverified",
  provenance: {
    provider: "human",
    teacher: "human:test-json-pointer-default",
    requestId: "json-pointer-default-1",
    generatedAt: "2026-08-23T00:00:00.000Z",
    situation,
  },
});

test(
  "SDK completes the DOUBLE kitchen cycle through the Rust stdio server",
  { timeout: 30_000 },
  async () => {
    const directory = await mkdtemp(path.join(os.tmpdir(), "spoon-kitchen-"));
    const previousDatabase = process.env.SPOON_DB;
    const previousAdminToken = process.env.SPOON_ADMIN_TOKEN;
    process.env.SPOON_DB = path.join(directory, "spoon.db");
    process.env.SPOON_ADMIN_TOKEN = "sdk-integration-admin";
    const transport = StdioTransport.spawn("cargo", [
      "run",
      "--quiet",
      "-p",
      "spoon-server",
    ]);
    const client = new SpoonClient(transport, {
      adminToken: "sdk-integration-admin",
    });

    try {
      const procedure = await client.createProcedure<{ id: string }>({
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
          requires: [
            {
              description: "x is non-negative",
              check: {
                BinOp: {
                  op: "Ge",
                  left: { Var: "x" },
                  right: { Literal: 0 },
                },
              },
            },
          ],
          promises: [
            {
              description: "result is double x",
              check: {
                BinOp: {
                  op: "Eq",
                  left: { Var: "result" },
                  right: {
                    BinOp: {
                      op: "Mul",
                      left: { Var: "x" },
                      right: { Literal: 2 },
                    },
                  },
                },
              },
            },
          ],
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
      });
      const executed = await client.executeProcedure<{
        value: number;
        episode: { id: string; evaluation: { success: boolean } };
        trace: {
          steps: Array<{
            procedure_version: number;
            contract_checks: {
              requires: Array<{ status: string }>;
              promises: Array<{ status: string }>;
            };
          }>;
        };
      }>(procedure.id, { x: 7 }, 14);
      const replayed = await client.replayEpisode<{ value: number }>(
        executed.episode.id,
        { x: 9 },
      );
      const stored = await client.getEpisode<{
        observed_result: number;
        execution_trace: { steps: unknown[] };
      }>(executed.episode.id);

      assert.equal(executed.value, 14);
      assert.equal(executed.episode.evaluation.success, true);
      assert.equal(executed.trace.steps[0]?.procedure_version, 1);
      assert.equal(
        executed.trace.steps[0]?.contract_checks.requires[0]?.status,
        "Passed",
      );
      assert.equal(
        executed.trace.steps[0]?.contract_checks.promises[0]?.status,
        "Passed",
      );
      assert.equal(stored.observed_result, 14);
      assert.equal(stored.execution_trace.steps.length, 1);
      assert.equal(replayed.value, 18);
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

test(
  "SDK renders a bounded response plan through the Rust stdio server",
  { timeout: 30_000 },
  async () => {
    const directory = await mkdtemp(path.join(os.tmpdir(), "spoon-language-"));
    const transport = StdioTransport.spawn(
      "cargo",
      ["run", "--quiet", "-p", "spoon-server"],
      { env: { ...process.env, SPOON_DB: path.join(directory, "spoon.db") } },
    );
    const client = new SpoonClient(transport);

    try {
      const rendered = await client.renderResponsePlan(
        {
          dialogueMove: { act: "Inform", relatesToTurn: null },
          claims: [
            {
              Grounded: {
                id: "answer",
                text: "There are 3 r characters in strawberry.",
                evidence: [
                  {
                    id: "episode:letter-count",
                    sourceKind: "SelfVerified",
                    linkedEpisode: null,
                  },
                ],
                provenance: ["procedure:private-letter-count"],
              },
            },
            {
              Unsupported: {
                id: "unsupported",
                reason: "No observed evidence was supplied.",
              },
            },
          ],
          uncertainty: { level: "Certain", disclosure: null },
          tone: "Neutral",
          variant: "Plain",
        },
        { variant: "Bulleted", tone: "Warm" },
      );

      assert.equal(rendered.text, "- There are 3 r characters in strawberry.");
      assert.deepEqual(rendered.includedClaimIds, ["answer"]);
      assert.deepEqual(rendered.omittedClaimIds, ["unsupported"]);
      assert.equal(rendered.audit.evidenceStatus, "caller_supplied_unverified");
      assert.equal(rendered.audit.provenanceRedacted, true);
      assert.equal(
        JSON.stringify(rendered).includes("private-letter-count"),
        false,
      );
    } finally {
      client.close();
      await rm(directory, { recursive: true, force: true });
    }
  },
);

test(
  "SDK adopts a quoted-text letter-count lesson and reuses it with Teacher-OFF",
  { timeout: 30_000 },
  async () => {
    const directory = await mkdtemp(
      path.join(os.tmpdir(), "spoon-quoted-letter-count-"),
    );
    const acquisition = 'how many "r" letters are in "strawberry"?';
    const transport = StdioTransport.spawn(
      "cargo",
      ["run", "--quiet", "-p", "spoon-server"],
      { env: { ...process.env, SPOON_DB: path.join(directory, "spoon.db") } },
    );
    const client = new SpoonClient(transport);

    try {
      const started = await client.beginCycle(
        defaultCycleInput(acquisition, true),
      );
      assert.equal(started.status, "need_teacher");

      if (started.status !== "need_teacher") {
        assert.fail("acquisition must request a Teacher proposal");
      }

      const adopted = await client.resumeCycle(
        started.cycleId,
        quotedLetterCountLesson(acquisition),
      );
      assert.equal(adopted.status, "completed");
      assert.equal(adopted.disposition, "provisional");
      assert.equal(adopted.answer, 3);

      const retained = await client.beginCycle(
        defaultCycleInput('count "r" in "raspberry"', false),
      );
      assert.equal(retained.status, "completed");
      assert.equal(retained.disposition, "provisional");
      assert.equal(retained.answer, 3);
    } finally {
      client.close();
      await rm(directory, { recursive: true, force: true });
    }
  },
);

test(
  "SDK adopts and reuses a tokenized word-count lesson through Rust stdio",
  { timeout: 30_000 },
  async () => {
    const directory = await mkdtemp(
      path.join(os.tmpdir(), "spoon-tokenized-word-count-"),
    );
    const acquisition = 'how many words are in "Alpha beta, gamma!"?';
    const transport = StdioTransport.spawn(
      "cargo",
      ["run", "--quiet", "-p", "spoon-server"],
      { env: { ...process.env, SPOON_DB: path.join(directory, "spoon.db") } },
    );
    const client = new SpoonClient(transport);

    try {
      const started = await client.beginCycle(
        defaultCycleInput(acquisition, true),
      );
      assert.equal(started.status, "need_teacher");

      if (started.status !== "need_teacher") {
        assert.fail("acquisition must request a Teacher proposal");
      }

      const adopted = await client.resumeCycle(
        started.cycleId,
        tokenizedWordCountLesson(acquisition, "Alpha beta, gamma!"),
      );
      assert.equal(adopted.status, "completed");
      assert.equal(adopted.disposition, "provisional");
      assert.equal(adopted.answer, 3);

      const retained = await client.beginCycle(
        defaultCycleInput('how many words are in "solo   two\tTHREE."?', false),
      );
      assert.equal(retained.status, "completed");
      assert.equal(retained.disposition, "provisional");
      assert.equal(retained.answer, 3);
    } finally {
      client.close();
      await rm(directory, { recursive: true, force: true });
    }
  },
);

test(
  "SDK adopts a JSON Pointer default lesson and reuses it with Teacher-OFF",
  { timeout: 30_000 },
  async () => {
    const directory = await mkdtemp(
      path.join(os.tmpdir(), "spoon-json-pointer-default-"),
    );
    const acquisition =
      'read the answer field from JSON \'{"name":"Spoon"}\', defaulting to unknown if absent';
    const transport = StdioTransport.spawn(
      "cargo",
      ["run", "--quiet", "-p", "spoon-server"],
      { env: { ...process.env, SPOON_DB: path.join(directory, "spoon.db") } },
    );
    const client = new SpoonClient(transport);

    try {
      const started = await client.beginCycle(
        defaultCycleInput(acquisition, true),
      );
      assert.equal(started.status, "need_teacher");

      if (started.status !== "need_teacher") {
        assert.fail("acquisition must request a Teacher proposal");
      }

      const adopted = await client.resumeCycle(
        started.cycleId,
        jsonPointerDefaultLesson(acquisition, '{"name":"Spoon"}'),
      );
      assert.equal(adopted.status, "completed");
      assert.equal(adopted.disposition, "provisional");
      assert.equal(adopted.answer, "unknown");

      const retained = await client.beginCycle(
        defaultCycleInput(
          'read the answer field from JSON \'{"name":"EKG"}\', defaulting to unknown if absent',
          false,
        ),
      );
      assert.equal(retained.status, "completed");
      assert.equal(retained.disposition, "provisional");
      assert.equal(retained.answer, "unknown");
    } finally {
      client.close();
      await rm(directory, { recursive: true, force: true });
    }
  },
);
