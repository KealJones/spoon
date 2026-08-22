import { spawn } from "node:child_process";

import { TeacherError } from "./errors.js";
import { buildTeacherPrompt, TEACHER_SYSTEM_PROMPT } from "./prompt.js";
import { SourceReliabilityTracker } from "./reliability.js";
import {
  ProposalValidationPipeline,
  type ProposalValidationPipelineOptions,
} from "./validation.js";
import {
  atProviderBoundary,
  defaultClock,
  defaultIdFactory,
  isJsonValue,
  makeProposal,
} from "./shared.js";
import type {
  Clock,
  CommandInvocation,
  CommandResult,
  CommandRunner,
  IdFactory,
  Teacher,
  TeacherProposal,
  TeacherRequest,
} from "./types.js";

export interface ClaudeTeacherOptions {
  command?: string;
  model?: string;
  cwd?: string;
  runner?: CommandRunner;
  reliabilityTracker?: SourceReliabilityTracker;
  now?: Clock;
  idFactory?: IdFactory;
}

export class ClaudeTeacher implements Teacher {
  readonly #command: string;
  readonly #model?: string;
  readonly #cwd?: string;
  readonly #runner: CommandRunner;
  readonly #tracker: SourceReliabilityTracker;
  readonly #now: Clock;
  readonly #idFactory: IdFactory;
  readonly #source: string;

  constructor(options: ClaudeTeacherOptions = {}) {
    this.#command = options.command ?? "claude";
    this.#model = options.model;
    this.#cwd = options.cwd;
    this.#runner = options.runner ?? runCommand;
    this.#tracker =
      options.reliabilityTracker ?? new SourceReliabilityTracker();
    this.#now = options.now ?? defaultClock;
    this.#idFactory = options.idFactory ?? defaultIdFactory;
    this.#source = `claude:${this.#model ?? "default"}`;
  }

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
    const args = [
      "-p",
      "--output-format",
      "json",
      "--json-schema",
      JSON.stringify(request.desiredOutput),
      "--tools",
      "",
      "--system-prompt",
      TEACHER_SYSTEM_PROMPT,
    ];
    if (this.#model !== undefined) args.push("--model", this.#model);
    args.push(buildTeacherPrompt(request));

    const result = await atProviderBoundary(
      "claude",
      "command invocation failed",
      () =>
        this.#runner({
          command: this.#command,
          args,
          cwd: this.#cwd,
        }),
    );
    if (result.exitCode !== 0) {
      throw new TeacherError(
        "claude",
        `command exited with status ${result.exitCode}: ${result.stderr.trim() || "unknown error"}`,
      );
    }

    let envelope: unknown;
    try {
      envelope = JSON.parse(result.stdout);
    } catch (error) {
      throw new TeacherError(
        "claude",
        "command did not return a valid JSON result envelope",
        {
          cause: error,
        },
      );
    }
    if (
      typeof envelope !== "object" ||
      envelope === null ||
      !("structured_output" in envelope)
    ) {
      throw new TeacherError(
        "claude",
        "result envelope did not contain structured_output",
      );
    }
    const content = envelope.structured_output;
    if (!isJsonValue(content)) {
      throw new TeacherError(
        "claude",
        "structured_output was not JSON-compatible",
      );
    }

    return makeProposal({
      content,
      provider: "claude",
      source: this.#source,
      model: this.#model,
      request,
      requestId: this.#idFactory(),
      generatedAt: this.#now(),
    });
  }

  reliability() {
    return this.#tracker.get(this.#source);
  }

  validationPipeline(
    options: Omit<ProposalValidationPipelineOptions, "reliabilityTracker"> = {},
  ) {
    return new ProposalValidationPipeline({
      ...options,
      reliabilityTracker: this.#tracker,
    });
  }
}

export function runCommand(
  invocation: CommandInvocation,
): Promise<CommandResult> {
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
