import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { SpoonClient, StdioTransport } from "../src/index.js";

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
