import assert from "node:assert/strict";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  redactedConfig,
  resolveConfig,
  validateConfig,
} from "../src/config.js";

test("resolves home and nested project config with source precedence", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "spoon-config-"));
  const home = path.join(root, "home");
  const project = path.join(root, "projects", "demo", "nested");
  await mkdir(path.join(home, ".spoon"), { recursive: true });
  await mkdir(path.join(root, "projects", ".spoon"), { recursive: true });
  await mkdir(path.join(root, "projects", "demo", ".spoon"), {
    recursive: true,
  });
  await mkdir(path.join(project, ".spoon"), { recursive: true });
  await writeFile(
    path.join(home, ".spoon", "config.json"),
    JSON.stringify({
      version: 1,
      teacher: { provider: "codex" },
      recall: { maxEpisodes: 12 },
    }),
  );
  await writeFile(
    path.join(root, "projects", ".spoon", "config.json"),
    JSON.stringify({ version: 1, recall: { mode: "session" } }),
  );
  await writeFile(
    path.join(root, "projects", "demo", ".spoon", "config.json"),
    JSON.stringify({ version: 1, teacher: { model: "fast" } }),
  );
  await writeFile(
    path.join(project, ".spoon", "config.local.json"),
    JSON.stringify({ version: 1, output: { mode: "quiet" } }),
  );

  const resolved = await resolveConfig({
    cwd: project,
    homeDir: home,
    env: {},
  });
  assert.equal(resolved.config.teacher.provider, "codex");
  assert.equal(resolved.config.teacher.model, "fast");
  assert.equal(resolved.config.recall.mode, "session");
  assert.equal(resolved.config.recall.maxEpisodes, 12);
  assert.equal(resolved.config.output.mode, "quiet");
  assert.equal(resolved.config.database.path, path.join(project, "spoon.db"));
  assert.equal(resolved.sources["teacher.model"]?.kind, "file");
  assert.ok(resolved.shadowed["teacher.provider"]?.length === 1);
});

test("environment overrides files and redacted output hides sensitive values", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "spoon-config-env-"));
  const home = path.join(root, "home");
  const cwd = path.join(root, "project");
  await mkdir(path.join(home, ".spoon"), { recursive: true });
  await mkdir(cwd, { recursive: true });
  await writeFile(
    path.join(home, ".spoon", "config.json"),
    JSON.stringify({ version: 1, capabilities: { permissionMode: "ask" } }),
  );
  const resolved = await resolveConfig({
    cwd,
    homeDir: home,
    env: {
      SPOON_PERMISSION_MODE: "full-access",
      SPOON_ADMIN_TOKEN: "do-not-print",
    },
  });
  assert.equal(resolved.config.capabilities.permissionMode, "full-access");
  assert.equal(
    JSON.stringify(redactedConfig(resolved)).includes("do-not-print"),
    false,
  );
});

test("rejects unknown keys and invalid permission modes", () => {
  assert.throws(
    () => validateConfig({ version: 1, mystery: true }),
    /unknown key 'mystery'/,
  );
  assert.throws(
    () =>
      validateConfig({ version: 1, capabilities: { permissionMode: "root" } }),
    /permissionMode must be ask, workspace, or full-access/,
  );
});
