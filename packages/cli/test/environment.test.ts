import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  adminTokenFromEnvironment,
  loadProjectEnvironment,
} from "../src/environment.js";

test("loads a local env file without overwriting the shell", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "ekg-env-"));
  const file = path.join(directory, ".env");
  const key = "EKG_ENV_LOAD_TEST";
  const previous = process.env[key];
  try {
    await writeFile(file, `${key}=from-file\n`);
    process.env[key] = "from-shell";
    loadProjectEnvironment(file);
    assert.equal(process.env[key], "from-shell");
  } finally {
    if (previous === undefined) delete process.env[key];
    else process.env[key] = previous;
    await rm(directory, { recursive: true, force: true });
  }
});

test("a missing local env file is optional", () => {
  assert.doesNotThrow(() =>
    loadProjectEnvironment("/tmp/ekg-missing-environment-file"),
  );
});

test("reads a non-empty admin token from the environment", () => {
  const previous = process.env.EKG_ADMIN_TOKEN;
  try {
    delete process.env.EKG_ADMIN_TOKEN;
    assert.equal(adminTokenFromEnvironment(), undefined);
    process.env.EKG_ADMIN_TOKEN = "  ";
    assert.equal(adminTokenFromEnvironment(), undefined);
    process.env.EKG_ADMIN_TOKEN = "bootstrap-secret";
    assert.equal(adminTokenFromEnvironment(), "bootstrap-secret");
  } finally {
    if (previous === undefined) delete process.env.EKG_ADMIN_TOKEN;
    else process.env.EKG_ADMIN_TOKEN = previous;
  }
});
