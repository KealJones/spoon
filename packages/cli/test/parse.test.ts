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
