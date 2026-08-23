import { appendFile, mkdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import path from "node:path";

import {
  configLayerPath,
  redactedConfig,
  resolveConfig,
  setConfigValue,
  validateConfigMutation,
  type ResolvedConfig,
} from "./config.js";

interface AdminMutation {
  key: string;
  value: unknown;
  label: string;
}

/**
 * Handle the small, deterministic set of local control-plane requests that
 * should not be delegated to a teacher. The raw request is never persisted;
 * receipts contain only a digest and the resulting redacted configuration.
 */
export async function tryHandleAdminRequest(
  request: string,
  resolved: ResolvedConfig,
): Promise<string | null> {
  const mutation = parseAdminMutation(request);
  if (mutation === null) return null;
  validateConfigMutation("user", mutation.key, mutation.value, resolved);
  const filePath = configLayerPath("user", resolved);
  await setConfigValue(filePath, mutation.key, mutation.value);
  applyMutationEnvironment(mutation);
  const refreshed = await resolveConfig({
    cwd: resolved.cwd,
    homeDir: resolved.homeDir,
    env: process.env,
  });
  await writeReceipt(request, mutation, refreshed);
  const applies =
    mutation.key === "database.path" ? "next launch" : "next cycle";
  return `${mutation.label}. Effective setting applies ${applies}.`;
}

function applyMutationEnvironment(mutation: AdminMutation): void {
  const environmentKey: Record<string, string> = {
    "teacher.enabled": "SPOON_TEACHER_ENABLED",
    "capabilities.permissionMode": "SPOON_PERMISSION_MODE",
    "recall.mode": "SPOON_RECALL_MODE",
    "database.path": "SPOON_DB",
  };
  const variable = environmentKey[mutation.key];
  if (variable !== undefined) process.env[variable] = String(mutation.value);
}

function parseAdminMutation(request: string): AdminMutation | null {
  const text = request.trim().replace(/[.!?]+$/, "");
  const normalized = text.toLowerCase().replace(/\s+/g, " ");
  if (
    /^(turn|switch|set) (off|disable) teacher$/.test(normalized) ||
    normalized === "disable teacher" ||
    normalized === "teacher off"
  ) {
    return { key: "teacher.enabled", value: false, label: "Teacher disabled" };
  }
  if (
    /^(turn|switch|set) (on|enable) teacher$/.test(normalized) ||
    normalized === "enable teacher" ||
    normalized === "teacher on"
  ) {
    return { key: "teacher.enabled", value: true, label: "Teacher enabled" };
  }
  const permission = normalized.match(
    /^(?:set |use |switch to )?(?:permission )?(ask|workspace|full[- ]access)(?: mode)?$/,
  );
  if (permission) {
    const value =
      permission[1]!.replace(/[ -]/g, "") === "fullaccess"
        ? "full-access"
        : permission[1];
    return {
      key: "capabilities.permissionMode",
      value,
      label: `Permission mode set to ${value}`,
    };
  }
  const recall = normalized.match(
    /^(?:set |use |switch to )?(?:episodic )?recall(?: mode)? (?:to )?(global|session|none)$/,
  );
  if (recall) {
    return {
      key: "recall.mode",
      value: recall[1],
      label: `Recall mode set to ${recall[1]}`,
    };
  }
  const database = text.match(
    /^(?:use|set)(?: the)? database(?: path)?(?: to| as)?\s+(.+)$/i,
  );
  if (database) {
    const value = database[1]!.trim().replace(/^['"]|['"]$/g, "");
    if (value.length === 0) return null;
    return {
      key: "database.path",
      value,
      label: `Database path set to ${value}`,
    };
  }
  return null;
}

async function writeReceipt(
  request: string,
  mutation: AdminMutation,
  resolved: ResolvedConfig,
): Promise<void> {
  const directory = path.join(resolved.homeDir, ".spoon");
  await mkdir(directory, { recursive: true });
  const receipt = {
    at: new Date().toISOString(),
    requestDigest: createHash("sha256").update(request).digest("hex"),
    key: mutation.key,
    value: mutation.value,
    effective: redactedConfig(resolved).effective,
  };
  await appendFile(
    path.join(directory, "admin-receipts.jsonl"),
    `${JSON.stringify(receipt)}\n`,
    "utf8",
  );
}
