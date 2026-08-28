import assert from "node:assert/strict";
import test from "node:test";

import { formatExplanation } from "../src/main.js";

test("explain output includes the exact executed procedure IR", () => {
  const output = formatExplanation({
    disposition: "verified",
    answer: 14,
    procedureIr: {
      name: "DOUBLE",
      params: [{ name: "x", value_type: "number" }],
      body: { kind: "bin_op", op: "mul" },
    },
    episode: {
      situation: "double 7",
      id: "episode-1",
      context: {},
      action: "procedure:double@1",
    },
  });

  assert.match(output, /Procedure IR:/);
  assert.match(output, /"name":\s*"DOUBLE"/);
  assert.match(output, /"op":\s*"mul"/);
});
