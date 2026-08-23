import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { tryHandleAdminRequest } from "../src/admin.js";
import { resolveConfig } from "../src/config.js";

test("deterministic admin requests update user config without teacher access", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "spoon-admin-"));
  const home = path.join(root, "home");
  const cwd = path.join(root, "project");
  await mkdir(cwd, { recursive: true });
  const previous = process.env.SPOON_TEACHER_ENABLED;
  delete process.env.SPOON_TEACHER_ENABLED;
  try {
    const resolved = await resolveConfig({ cwd, homeDir: home, env: {} });
    const message = await tryHandleAdminRequest("turn off teacher", resolved);
    assert.equal(
      message,
      "Teacher disabled. Effective setting applies next cycle.",
    );
    const saved = JSON.parse(
      await readFile(path.join(home, ".spoon", "config.json"), "utf8"),
    ) as { teacher?: { enabled?: boolean } };
    assert.equal(saved.teacher?.enabled, false);
    assert.match(
      await readFile(path.join(home, ".spoon", "admin-receipts.jsonl"), "utf8"),
      /requestDigest/,
    );
  } finally {
    if (previous === undefined) delete process.env.SPOON_TEACHER_ENABLED;
    else process.env.SPOON_TEACHER_ENABLED = previous;
  }
});
