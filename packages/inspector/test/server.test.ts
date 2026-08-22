import assert from "node:assert/strict";
import test from "node:test";

test("inspector package exposes a local dashboard entry point", async () => {
  const packageJson = await import("../package.json", { with: { type: "json" } });
  assert.equal(packageJson.default.scripts.dev, "tsx src/server.ts");
  assert.equal(packageJson.default.scripts.start, "node dist/src/server.js");
});
