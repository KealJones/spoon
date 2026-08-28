import assert from "node:assert/strict";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  applyConfigEnvironment,
  redactedConfig,
  resolveConfig,
  validateConfig,
} from "../src/config.js";
import { createConfiguredInterpreter } from "../src/cycle.js";

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
      language: { interpreter: { provider: "ollama", model: "qwen2.5:1.5b" } },
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
  assert.equal(resolved.config.language.interpreter.provider, "ollama");
  assert.equal(resolved.config.language.interpreter.model, "qwen2.5:1.5b");
  assert.equal(resolved.config.output.mode, "quiet");
  assert.equal(resolved.config.database.path, path.join(project, "spoon.db"));
  assert.equal(resolved.sources["teacher.model"]?.kind, "file");
  assert.ok(resolved.shadowed["teacher.provider"]?.length === 1);

  const interpreterEnvironment: NodeJS.ProcessEnv = {};
  applyConfigEnvironment(resolved, interpreterEnvironment);
  assert.equal(interpreterEnvironment.SPOON_INTERPRETER, "ollama");
  assert.equal(interpreterEnvironment.SPOON_INTERPRETER_MODEL, "qwen2.5:1.5b");
  assert.equal(
    createConfiguredInterpreter(interpreterEnvironment)?.constructor.name,
    "OllamaLanguageInterpreter",
  );
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
      SPOON_INTERPRETER: "ollama",
      SPOON_INTERPRETER_MODEL: "qwen2.5:0.5b",
      SPOON_ADMIN_TOKEN: "do-not-print",
    },
  });
  assert.equal(resolved.config.capabilities.permissionMode, "full-access");
  assert.equal(resolved.config.language.interpreter.provider, "ollama");
  assert.equal(resolved.config.language.interpreter.model, "qwen2.5:0.5b");
  const existingInterpreterEnvironment: NodeJS.ProcessEnv = {
    SPOON_INTERPRETER: "off",
  };
  applyConfigEnvironment(resolved, existingInterpreterEnvironment);
  assert.equal(existingInterpreterEnvironment.SPOON_INTERPRETER, "off");
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
    /permissionMode must be ask, workspace, full-access, or god-mode/,
  );
  assert.doesNotThrow(() =>
    validateConfig({
      $schema: "../packages/cli/src/config.schema.json",
      version: 1,
      capabilities: { permissionMode: "god-mode" },
    }),
  );
  assert.throws(
    () =>
      validateConfig({
        version: 1,
        language: { interpreter: { provider: "fake" } },
      }),
    /language\.interpreter\.provider must be off, ollama, or cursor/,
  );
});

test("ollama teacher inherits the language interpreter model", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "spoon-config-ollama-"));
  const home = path.join(root, "home");
  const cwd = path.join(root, "project");
  await mkdir(path.join(home, ".spoon"), { recursive: true });
  await mkdir(cwd, { recursive: true });
  await writeFile(
    path.join(home, ".spoon", "config.json"),
    JSON.stringify({
      version: 1,
      teacher: { provider: "ollama" },
      language: {
        interpreter: { provider: "ollama", model: "qwen2.5:1.5b" },
      },
    }),
  );

  const inherited = await resolveConfig({ cwd, homeDir: home, env: {} });
  const inheritedEnv: NodeJS.ProcessEnv = {};
  applyConfigEnvironment(inherited, inheritedEnv);
  assert.equal(inheritedEnv.SPOON_TEACHER, "ollama");
  assert.equal(inheritedEnv.SPOON_TEACHER_MODEL, "qwen2.5:1.5b");
  assert.equal(inheritedEnv.SPOON_INTERPRETER_MODEL, "qwen2.5:1.5b");

  const overridden = await resolveConfig({
    cwd,
    homeDir: home,
    env: { SPOON_TEACHER_MODEL: "qwen3:8b" },
  });
  const overriddenEnv: NodeJS.ProcessEnv = {};
  applyConfigEnvironment(overridden, overriddenEnv);
  assert.equal(overriddenEnv.SPOON_TEACHER_MODEL, "qwen3:8b");
});
