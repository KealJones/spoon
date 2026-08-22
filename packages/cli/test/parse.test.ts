import assert from "node:assert/strict";
import test from "node:test";

import { parseCommand } from "../src/parse.js";

test("parses concept creation", () => {
  assert.deepEqual(parseCommand(["concept", "add", "double"]), {
    kind: "concept.add",
    name: "double",
  });
});

test("parses procedure execution bindings as JSON values", () => {
  assert.deepEqual(
    parseCommand(["procedure", "run", "double", "x=7", 'label="trial"']),
    {
      kind: "procedure.run",
      procedure: "double",
      inputs: { x: 7, label: "trial" },
    },
  );
});

test("rejects unknown commands with usage text", () => {
  assert.throws(() => parseCommand(["wat"]), /Usage:/);
});

test("parses graph inspection and procedure listing", () => {
  assert.deepEqual(
    parseCommand(["graph", "traverse", "concept-1", "implemented-by", "3"]),
    {
      kind: "graph.traverse",
      conceptId: "concept-1",
      relationship: "implemented-by",
      maxHops: 3,
    },
  );
  assert.deepEqual(parseCommand(["procedure", "list"]), {
    kind: "procedure.list",
  });
});

test("parses a natural-language cycle request", () => {
  assert.deepEqual(parseCommand(["ask", "what", "is", "double", "7?"]), {
    kind: "cycle.run",
    situation: "what is double 7?",
  });
});

test("parses the explicit failure adaptation workflow", () => {
  const request = { episodeId: "episode-1", candidates: [] };
  assert.deepEqual(
    parseCommand(["failure", "analyze", JSON.stringify(request)]),
    { kind: "failure.analyze", request },
  );
  assert.deepEqual(parseCommand(["failure", "plan", JSON.stringify(request)]), {
    kind: "failure.plan",
    request,
  });
  assert.deepEqual(parseCommand(["failure", "apply", "plan-1"]), {
    kind: "failure.apply",
    planId: "plan-1",
  });
  assert.deepEqual(parseCommand(["failure", "apply-offline", "plan-1"]), {
    kind: "failure.apply-offline",
    planId: "plan-1",
  });
});

test("parses adaptation and contradiction inspection", () => {
  assert.deepEqual(parseCommand(["adaptation", "show", "plan-1"]), {
    kind: "adaptation.show",
    planId: "plan-1",
  });
  assert.deepEqual(parseCommand(["contradiction", "list"]), {
    kind: "contradiction.list",
  });
  assert.deepEqual(parseCommand(["contradiction", "get", "42"]), {
    kind: "contradiction.get",
    contradictionId: 42,
  });
  const record = { left: { id: "left" }, right: { id: "right" } };
  assert.deepEqual(
    parseCommand(["contradiction", "record", JSON.stringify(record)]),
    { kind: "contradiction.record", request: record },
  );
  const refinement = { contradictionId: 42, discriminator: { feature: "x" } };
  assert.deepEqual(
    parseCommand(["contradiction", "refine", JSON.stringify(refinement)]),
    { kind: "contradiction.refine", request: refinement },
  );
  assert.deepEqual(
    parseCommand(["contradiction", "uncertainty", "recipe-plan"]),
    { kind: "contradiction.uncertainty", claimId: "recipe-plan" },
  );
});
