import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";

import { CursorLanguageInterpreterError } from "./errors.js";
import {
  buildIntentPrompt,
  reconsiderationProposal,
  wireInterpretation,
} from "./ollama.js";
import type {
  Clock,
  EngineRequest,
  IdFactory,
  IntentProposalWire,
  JsonValue,
  LanguageInterpreter,
} from "./types.js";

export interface CursorCommandInvocation {
  command: string;
  args: string[];
  cwd?: string;
}

export interface CursorCommandResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export type CursorCommandRunner = (
  invocation: CursorCommandInvocation,
) => Promise<CursorCommandResult>;

export interface CursorLanguageInterpreterOptions {
  command?: string;
  model?: string;
  cwd?: string;
  runner?: CursorCommandRunner;
  now?: Clock;
  idFactory?: IdFactory;
}

export class CursorLanguageInterpreter implements LanguageInterpreter {
  readonly #command: string;
  readonly #model?: string;
  readonly #cwd?: string;
  readonly #runner: CursorCommandRunner;
  readonly #now: Clock;
  readonly #idFactory: IdFactory;
  readonly #source: string;

  constructor(options: CursorLanguageInterpreterOptions = {}) {
    this.#command = options.command ?? "agent";
    this.#model = options.model?.trim() || undefined;
    this.#cwd = options.cwd;
    this.#runner = options.runner ?? runCursorCommand;
    this.#now = options.now ?? (() => new Date());
    this.#idFactory = options.idFactory ?? randomUUID;
    this.#source = `cursor:${this.#model ?? "default"}`;
  }

  async interpret(request: EngineRequest): Promise<IntentProposalWire> {
    const reconsideration = reconsiderationProposal(
      request,
      this.#idFactory(),
      this.#now(),
    );
    if (reconsideration !== undefined) return reconsideration;

    const args = [
      "-p",
      "--mode",
      "ask",
      "--output-format",
      "json",
      "--trust",
    ];
    if (this.#model !== undefined) args.push("--model", this.#model);
    args.push(
      [
        buildIntentPrompt(request),
        "Return only the requested JSON object.",
      ].join("\n\n"),
    );

    let result: CursorCommandResult;
    try {
      result = await this.#runner({
        command: this.#command,
        args,
        cwd: this.#cwd,
      });
    } catch (error) {
      throw new CursorLanguageInterpreterError("command invocation failed", {
        cause: error,
      });
    }
    if (result.exitCode !== 0) {
      throw new CursorLanguageInterpreterError(
        `command exited with status ${result.exitCode}: ${result.stderr.trim() || "unknown error"}`,
      );
    }

    return wireInterpretation(
      "cursor",
      this.#source,
      this.#model ?? "default",
      parseCursorInterpreterResult(result.stdout),
      request,
      this.#idFactory(),
      this.#now(),
    );
  }
}

export function parseCursorInterpreterResult(stdout: string): JsonValue {
  let envelope: unknown;
  try {
    envelope = JSON.parse(stdout);
  } catch (error) {
    throw new CursorLanguageInterpreterError(
      "command did not return a valid JSON result envelope",
      { cause: error },
    );
  }
  if (typeof envelope !== "object" || envelope === null) {
    throw new CursorLanguageInterpreterError(
      "result envelope did not contain result",
    );
  }
  const record = envelope as Record<string, unknown>;
  if (record.is_error === true || record.subtype === "error") {
    const detail =
      typeof record.result === "string" && record.result.trim() !== ""
        ? record.result.trim()
        : "unknown error";
    throw new CursorLanguageInterpreterError(
      `command reported an error: ${detail}`,
    );
  }
  if (!("result" in record)) {
    throw new CursorLanguageInterpreterError(
      "result envelope did not contain result",
    );
  }
  const content = record.result;
  if (isJsonValue(content) && typeof content !== "string") return content;
  if (typeof content !== "string") {
    throw new CursorLanguageInterpreterError("result was not JSON-compatible");
  }
  const trimmed = content.trim();
  try {
    return JSON.parse(trimmed) as JsonValue;
  } catch (error) {
    const fenced = trimmed.match(/```(?:json)?\s*([\s\S]*?)\s*```/i)?.[1];
    if (fenced !== undefined) {
      try {
        return JSON.parse(fenced) as JsonValue;
      } catch (fencedError) {
        throw new CursorLanguageInterpreterError(
          "result was not valid JSON",
          { cause: fencedError },
        );
      }
    }
    throw new CursorLanguageInterpreterError("result was not valid JSON", {
      cause: error,
    });
  }
}

function runCursorCommand(
  invocation: CursorCommandInvocation,
): Promise<CursorCommandResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(invocation.command, invocation.args, {
      cwd: invocation.cwd,
      shell: false,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (exitCode) =>
      resolve({ exitCode: exitCode ?? -1, stdout, stderr }),
    );
  });
}

function isJsonValue(value: unknown): value is JsonValue {
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean"
  ) {
    return true;
  }
  if (typeof value === "number" && Number.isFinite(value)) return true;
  if (Array.isArray(value)) return value.every(isJsonValue);
  return (
    typeof value === "object" &&
    value !== null &&
    Object.values(value).every(isJsonValue)
  );
}
