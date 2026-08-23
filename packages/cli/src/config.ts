import { access, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

export type PermissionMode = "ask" | "workspace" | "full-access";
export type RecallMode = "global" | "session" | "none";

export interface SpoonConfig {
  version: 1;
  database: { path: string };
  teacher: {
    enabled: boolean;
    provider: string;
    model?: string | null;
    command?: string | null;
  };
  capabilities: { permissionMode: PermissionMode };
  recall: {
    mode: RecallMode;
    lookback?: string | null;
    maxEpisodes: number;
  };
  output: { mode: "quiet" | "normal" | "explain" };
}

export interface ConfigSource {
  source: string;
  path?: string;
  kind: "default" | "file" | "environment" | "flag" | "authority";
}

export interface ResolvedConfig {
  config: SpoonConfig;
  sources: Record<string, ConfigSource>;
  shadowed: Record<string, ConfigSource[]>;
  files: string[];
  cwd: string;
  homeDir: string;
}

export interface ResolveConfigOptions {
  cwd?: string;
  homeDir?: string;
  env?: NodeJS.ProcessEnv;
  includeEnvironment?: boolean;
}

const DEFAULT_CONFIG: SpoonConfig = {
  version: 1,
  database: { path: "spoon.db" },
  teacher: { enabled: true, provider: "claude", model: null },
  capabilities: { permissionMode: "ask" },
  recall: { mode: "global", lookback: "90d", maxEpisodes: 64 },
  output: { mode: "normal" },
};

const ALLOWED_KEYS: Record<string, string[]> = {
  "": ["version", "database", "teacher", "capabilities", "recall", "output"],
  database: ["path"],
  teacher: ["enabled", "provider", "model", "command"],
  capabilities: ["permissionMode"],
  recall: ["mode", "lookback", "maxEpisodes"],
  output: ["mode"],
};

export function defaultConfig(): SpoonConfig {
  return structuredClone(DEFAULT_CONFIG);
}

export async function resolveConfig(
  options: ResolveConfigOptions = {},
): Promise<ResolvedConfig> {
  const cwd = path.resolve(options.cwd ?? process.cwd());
  const homeDir = path.resolve(options.homeDir ?? os.homedir());
  const env = options.env ?? process.env;
  let config = defaultConfig();
  const sources: Record<string, ConfigSource> = {};
  const shadowed: Record<string, ConfigSource[]> = {};
  const files: string[] = [];

  addSourceTree(
    config,
    "",
    {
      kind: "default",
      source: "built-in defaults",
    },
    sources,
  );

  const filePaths = configFilePaths(cwd, homeDir);
  for (const filePath of filePaths) {
    const layer = await readOptionalConfig(filePath);
    if (layer === undefined) continue;
    validateConfig(layer, filePath);
    const normalized = normalizeLayer(layer, path.dirname(filePath));
    config = mergeConfig(config, normalized, filePath, sources, shadowed);
    files.push(filePath);
  }

  if (options.includeEnvironment !== false) {
    const environment = environmentLayer(env);
    config = mergeConfig(config, environment, "environment", sources, shadowed);
  }

  if (!path.isAbsolute(config.database.path))
    config.database.path = path.resolve(cwd, config.database.path);
  enforceResolvedConfig(config, cwd, homeDir, env);
  return { config, sources, shadowed, files, cwd, homeDir };
}

export function validateConfig(value: unknown, source = "config"): void {
  if (!isRecord(value)) fail(source, "configuration must be a JSON object");
  validateKeys(value, "", source);
  if (value.version !== 1) fail(source, "version must be 1");
  const database = objectAt(value, "database", source);
  if (
    database.path !== undefined &&
    (typeof database.path !== "string" || database.path.trim() === "")
  )
    fail(source, "database.path must be a non-empty string");
  const teacher = objectAt(value, "teacher", source);
  if (teacher.enabled !== undefined && typeof teacher.enabled !== "boolean")
    fail(source, "teacher.enabled must be a boolean");
  if (teacher.provider !== undefined && typeof teacher.provider !== "string")
    fail(source, "teacher.provider must be a string");
  if (
    teacher.model !== undefined &&
    teacher.model !== null &&
    typeof teacher.model !== "string"
  )
    fail(source, "teacher.model must be a string or null");
  const capabilities = objectAt(value, "capabilities", source);
  if (
    capabilities.permissionMode !== undefined &&
    !isPermissionMode(capabilities.permissionMode)
  )
    fail(
      source,
      "capabilities.permissionMode must be ask, workspace, or full-access",
    );
  const recall = objectAt(value, "recall", source);
  if (recall.mode !== undefined && !isRecallMode(recall.mode))
    fail(source, "recall.mode must be global, session, or none");
  if (recall.lookback !== undefined && recall.lookback !== null) {
    if (
      typeof recall.lookback !== "string" ||
      !/^\d+(?:s|m|h|d|w)$/.test(recall.lookback)
    )
      fail(source, "recall.lookback must use a duration such as 90d or null");
  }
  const maxEpisodes = recall.maxEpisodes;
  if (
    maxEpisodes !== undefined &&
    (typeof maxEpisodes !== "number" ||
      !Number.isInteger(maxEpisodes) ||
      maxEpisodes < 0 ||
      maxEpisodes > 100_000)
  )
    fail(source, "recall.maxEpisodes must be an integer from 0 to 100000");
  const output = objectAt(value, "output", source);
  if (
    output.mode !== undefined &&
    !["quiet", "normal", "explain"].includes(String(output.mode))
  )
    fail(source, "output.mode must be quiet, normal, or explain");
}

export function redactedConfig(
  resolved: ResolvedConfig,
): Record<string, unknown> {
  return {
    effective: redact(resolved.config),
    sources: resolved.sources,
    shadowed: resolved.shadowed,
    files: resolved.files,
    cwd: resolved.cwd,
    homeDir: resolved.homeDir,
  };
}

export function applyConfigEnvironment(
  resolved: ResolvedConfig,
  env: NodeJS.ProcessEnv = process.env,
): void {
  if (!env.SPOON_DB) env.SPOON_DB = resolved.config.database.path;
  if (!env.SPOON_TEACHER) env.SPOON_TEACHER = resolved.config.teacher.provider;
  if (!env.SPOON_TEACHER_ENABLED)
    env.SPOON_TEACHER_ENABLED = String(resolved.config.teacher.enabled);
  if (!env.SPOON_TEACHER_MODEL && resolved.config.teacher.model)
    env.SPOON_TEACHER_MODEL = resolved.config.teacher.model;
  if (!env.SPOON_PERMISSION_MODE)
    env.SPOON_PERMISSION_MODE = resolved.config.capabilities.permissionMode;
  if (!env.SPOON_RECALL_MODE)
    env.SPOON_RECALL_MODE = resolved.config.recall.mode;
  if (!env.SPOON_RECALL_MAX_EPISODES)
    env.SPOON_RECALL_MAX_EPISODES = String(resolved.config.recall.maxEpisodes);
}

export async function writeConfigLayer(
  filePath: string,
  patch: Record<string, unknown>,
): Promise<void> {
  const existing = (await readOptionalConfig(filePath)) ?? { version: 1 };
  const next = mergePlain(existing, patch);
  validateConfig(next, filePath);
  const normalized = normalizeLayer(next, path.dirname(filePath));
  await mkdir(path.dirname(filePath), { recursive: true });
  const temporary = `${filePath}.tmp-${process.pid}-${Date.now()}`;
  await writeFile(
    temporary,
    `${JSON.stringify(normalized, null, 2)}\n`,
    "utf8",
  );
  await rename(temporary, filePath);
}

export async function setConfigValue(
  filePath: string,
  key: string,
  value: unknown,
  remove = false,
): Promise<void> {
  if (!/^[a-zA-Z][a-zA-Z0-9]*(?:\.[a-zA-Z][a-zA-Z0-9]*)*$/.test(key))
    throw new Error(`invalid config key '${key}'`);
  const existing = (await readOptionalConfig(filePath)) ?? { version: 1 };
  const segments = key.split(".");
  let cursor: Record<string, unknown> = existing;
  for (const segment of segments.slice(0, -1)) {
    const child = cursor[segment];
    if (!isRecord(child)) cursor[segment] = {};
    cursor = cursor[segment] as Record<string, unknown>;
  }
  const leaf = segments[segments.length - 1]!;
  if (remove) delete cursor[leaf];
  else cursor[leaf] = value;
  await writeConfigLayer(filePath, existing);
}

export function configLayerPath(
  layer: "user" | "project" | "local",
  resolved: Pick<ResolvedConfig, "cwd" | "homeDir">,
): string {
  if (layer === "user")
    return path.join(resolved.homeDir, ".spoon", "config.json");
  const directory = path.join(resolved.cwd, ".spoon");
  return path.join(
    directory,
    layer === "local" ? "config.local.json" : "config.json",
  );
}

export function validateConfigMutation(
  layer: "user" | "project" | "local",
  key: string,
  value: unknown,
  resolved: Pick<ResolvedConfig, "cwd" | "homeDir">,
): void {
  if (
    key === "capabilities.permissionMode" &&
    layer !== "user" &&
    value !== undefined &&
    value !== "ask"
  )
    throw new Error(
      "project config may only force capabilities.permissionMode to ask",
    );
  if (key !== "database.path" || typeof value !== "string") return;
  const target = path.resolve(
    layer === "user" ? resolved.homeDir : resolved.cwd,
    value,
  );
  if (layer === "user") return;
  const root = resolved.cwd;
  if (target !== root && !target.startsWith(`${root}${path.sep}`))
    throw new Error(`database.path must stay within the ${layer} config root`);
}

function configFilePaths(cwd: string, homeDir: string): string[] {
  const ancestors: string[] = [];
  let current = cwd;
  while (true) {
    ancestors.push(current);
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
  }
  ancestors.reverse();
  const paths = [path.join(homeDir, ".spoon", "config.json")];
  for (const directory of ancestors) {
    const candidate = path.join(directory, ".spoon", "config.json");
    if (!paths.includes(candidate)) paths.push(candidate);
  }
  paths.push(path.join(cwd, ".spoon", "config.local.json"));
  return paths;
}

async function readOptionalConfig(
  filePath: string,
): Promise<Record<string, unknown> | undefined> {
  try {
    await access(filePath);
  } catch (error) {
    if (hasCode(error, "ENOENT")) return undefined;
    throw error;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(await readFile(filePath, "utf8")) as unknown;
  } catch (error) {
    throw new Error(`${filePath}: invalid JSON (${String(error)})`);
  }
  if (!isRecord(parsed)) fail(filePath, "configuration must be a JSON object");
  return parsed;
}

function normalizeLayer(
  layer: Record<string, unknown>,
  base: string,
): Record<string, unknown> {
  const result = structuredClone(layer);
  const database = isRecord(result.database) ? result.database : undefined;
  if (
    database &&
    typeof database.path === "string" &&
    !path.isAbsolute(database.path)
  )
    database.path = path.resolve(base, database.path);
  return result;
}

function environmentLayer(env: NodeJS.ProcessEnv): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (env.SPOON_DB) result.database = { path: env.SPOON_DB };
  if (env.SPOON_TEACHER || env.SPOON_TEACHER_MODEL) {
    result.teacher = {
      ...(env.SPOON_TEACHER ? { provider: env.SPOON_TEACHER } : {}),
      ...(env.SPOON_TEACHER_MODEL ? { model: env.SPOON_TEACHER_MODEL } : {}),
    };
  }
  if (env.SPOON_TEACHER_ENABLED !== undefined) {
    if (
      env.SPOON_TEACHER_ENABLED !== "true" &&
      env.SPOON_TEACHER_ENABLED !== "false"
    )
      throw new Error("SPOON_TEACHER_ENABLED must be true or false");
    result.teacher = {
      ...(isRecord(result.teacher) ? result.teacher : {}),
      enabled: env.SPOON_TEACHER_ENABLED === "true",
    };
  }
  if (env.SPOON_PERMISSION_MODE)
    result.capabilities = { permissionMode: env.SPOON_PERMISSION_MODE };
  if (env.SPOON_RECALL_MODE || env.SPOON_RECALL_MAX_EPISODES) {
    result.recall = {
      ...(env.SPOON_RECALL_MODE ? { mode: env.SPOON_RECALL_MODE } : {}),
      ...(env.SPOON_RECALL_MAX_EPISODES
        ? { maxEpisodes: Number(env.SPOON_RECALL_MAX_EPISODES) }
        : {}),
    };
  }
  return result;
}

function mergeConfig(
  base: SpoonConfig,
  layer: Record<string, unknown>,
  source: string,
  sources: Record<string, ConfigSource>,
  shadowed: Record<string, ConfigSource[]>,
): SpoonConfig {
  const merged = deepMerge(
    base as unknown as Record<string, unknown>,
    layer,
    "",
    {
      source,
      kind: source === "environment" ? "environment" : "file",
    },
    sources,
    shadowed,
  );
  validateConfig(merged, source);
  return merged as unknown as SpoonConfig;
}

function deepMerge(
  base: Record<string, unknown>,
  layer: Record<string, unknown>,
  prefix: string,
  source: ConfigSource,
  sources: Record<string, ConfigSource>,
  shadowed: Record<string, ConfigSource[]>,
): Record<string, unknown> {
  const result = structuredClone(base);
  for (const [key, value] of Object.entries(layer)) {
    const fullPath = prefix ? `${prefix}.${key}` : key;
    const prior = sources[fullPath];
    if (prior) (shadowed[fullPath] ??= []).push(prior);
    if (isRecord(value) && isRecord(result[key])) {
      result[key] = deepMerge(
        result[key] as Record<string, unknown>,
        value,
        fullPath,
        source,
        sources,
        shadowed,
      );
    } else {
      result[key] = structuredClone(value);
    }
    sources[fullPath] = {
      ...source,
      source: source.source,
      path: source.kind === "file" ? source.source : undefined,
    };
  }
  return result;
}

function mergePlain(
  base: Record<string, unknown>,
  layer: Record<string, unknown>,
): Record<string, unknown> {
  const result = structuredClone(base);
  for (const [key, value] of Object.entries(layer)) {
    if (isRecord(value) && isRecord(result[key]))
      result[key] = mergePlain(result[key] as Record<string, unknown>, value);
    else result[key] = structuredClone(value);
  }
  return result;
}

function addSourceTree(
  value: unknown,
  prefix: string,
  source: ConfigSource,
  sources: Record<string, ConfigSource>,
): void {
  if (!isRecord(value)) {
    if (prefix) sources[prefix] = source;
    return;
  }
  for (const [key, child] of Object.entries(value)) {
    const childPath = prefix ? `${prefix}.${key}` : key;
    addSourceTree(child, childPath, source, sources);
  }
}

function enforceResolvedConfig(
  config: SpoonConfig,
  cwd: string,
  homeDir: string,
  env: NodeJS.ProcessEnv,
): void {
  validateConfig(config, "effective configuration");
  const databasePath = path.resolve(config.database.path);
  const projectRoot = cwd;
  const isInsideProject =
    databasePath === projectRoot ||
    databasePath.startsWith(`${projectRoot}${path.sep}`);
  const isInsideHome =
    databasePath === homeDir ||
    databasePath.startsWith(`${homeDir}${path.sep}`);
  if (!isInsideProject && !isInsideHome) {
    // An explicit SPOON_DB/CLI override is allowed to select a test database.
    if (!env.SPOON_DB)
      throw new Error(
        `effective database path escapes the project/home boundary: ${databasePath}`,
      );
  }
}

function redact(value: unknown, key = ""): unknown {
  if (/(token|secret|password|credential|api.?key)/i.test(key))
    return "[redacted]";
  if (Array.isArray(value)) return value.map((item) => redact(item, key));
  if (isRecord(value))
    return Object.fromEntries(
      Object.entries(value).map(([child, item]) => [
        child,
        redact(item, child),
      ]),
    );
  return value;
}

function validateKeys(
  value: Record<string, unknown>,
  prefix: string,
  source: string,
): void {
  const allowed = ALLOWED_KEYS[prefix];
  if (!allowed) return;
  for (const key of Object.keys(value)) {
    if (!allowed.includes(key))
      fail(source, `unknown key '${prefix ? `${prefix}.` : ""}${key}'`);
    const child = value[key];
    if (isRecord(child))
      validateKeys(child, prefix ? `${prefix}.${key}` : key, source);
  }
}

function objectAt(
  value: Record<string, unknown>,
  key: string,
  source: string,
): Record<string, unknown> {
  const child = value[key];
  if (child === undefined) return {};
  if (!isRecord(child)) fail(source, `${key} must be an object`);
  return child;
}

function fail(source: string, message: string): never {
  throw new Error(`${source}: ${message}`);
}

function isPermissionMode(value: unknown): value is PermissionMode {
  return value === "ask" || value === "workspace" || value === "full-access";
}

function isRecallMode(value: unknown): value is RecallMode {
  return value === "global" || value === "session" || value === "none";
}

function isRecord(value: unknown): value is Record<string, any> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function hasCode(error: unknown, code: string): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    error.code === code
  );
}
