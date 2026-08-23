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
    quiet: false,
    explain: false,
  });
});

test("parses quiet natural-language cycle requests", () => {
  assert.deepEqual(
    parseCommand(["ask", "--quiet", "what", "is", "double", "7?"]),
    {
      kind: "cycle.run",
      situation: "what is double 7?",
      quiet: true,
      explain: false,
    },
  );
  assert.deepEqual(parseCommand(["ask", "-q", "what", "time", "is", "it?"]), {
    kind: "cycle.run",
    situation: "what time is it?",
    quiet: true,
    explain: false,
  });
  assert.deepEqual(
    parseCommand(["ask", "what", "is", "double", "7?", "--quiet"]),
    {
      kind: "cycle.run",
      situation: "what is double 7?",
      quiet: true,
      explain: false,
    },
  );
  assert.deepEqual(
    parseCommand(["ask", "--explain", "what", "is", "double", "7?"]),
    {
      kind: "cycle.run",
      situation: "what is double 7?",
      quiet: false,
      explain: true,
    },
  );
  assert.deepEqual(
    parseCommand(["ask", "--teacher", "off", "what", "is", "double", "7?"]),
    {
      kind: "cycle.run",
      situation: "what is double 7?",
      quiet: false,
      explain: false,
      teacher: "off",
    },
  );
});

test("parses benchmark run and report commands", () => {
  assert.deepEqual(
    parseCommand(["benchmark", "run", "bench.json", "report.json"]),
    {
      kind: "benchmark.run",
      fixturePath: "bench.json",
      reportPath: "report.json",
    },
  );
  assert.deepEqual(parseCommand(["benchmark", "report", "report.json"]), {
    kind: "benchmark.report",
    reportPath: "report.json",
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

test("parses a native primitive observation", () => {
  assert.deepEqual(parseCommand(["primitive", "observe", "clock"]), {
    kind: "primitive.observe",
    target: "clock",
  });
});

test("parses config diagnostics and session-aware ask options", () => {
  assert.deepEqual(parseCommand(["config", "path"]), { kind: "config.path" });
  assert.deepEqual(parseCommand(["config", "show", "--sources"]), {
    kind: "config.show",
    withSources: true,
  });
  assert.deepEqual(
    parseCommand(["config", "set", "recall.mode", "none", "--layer", "user"]),
    { kind: "config.set", key: "recall.mode", value: "none", layer: "user" },
  );
  assert.deepEqual(
    parseCommand([
      "ask",
      "--session",
      "chat-1",
      "--recall",
      "session",
      "--permission-mode",
      "workspace",
      "hello",
    ]),
    {
      kind: "cycle.run",
      situation: "hello",
      quiet: false,
      explain: false,
      session: "chat-1",
      recall: "session",
      permissionMode: "workspace",
    },
  );
  assert.deepEqual(parseCommand(["chat", "--isolated"]), {
    kind: "chat.run",
    isolated: true,
  });
});
