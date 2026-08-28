import { TeacherError } from "./errors.js";
import { parseCliJson } from "./command.js";
import { buildTeacherPrompt, TEACHER_SYSTEM_PROMPT } from "./prompt.js";
import { SourceReliabilityTracker } from "./reliability.js";
import {
  ProposalValidationPipeline,
  type ProposalValidationPipelineOptions,
} from "./validation.js";
import { runCommand } from "./claude.js";
import {
  atProviderBoundary,
  defaultClock,
  defaultIdFactory,
  isJsonValue,
  makeProposal,
} from "./shared.js";
import type {
  Clock,
  CommandRunner,
  IdFactory,
  JsonValue,
  PromptBuilder,
  Teacher,
  TeacherProposal,
  TeacherRequest,
} from "./types.js";

export interface CursorTeacherOptions {
  command?: string;
  model?: string;
  cwd?: string;
  runner?: CommandRunner;
  reliabilityTracker?: SourceReliabilityTracker;
  now?: Clock;
  idFactory?: IdFactory;
  systemPrompt?: string;
  promptBuilder?: PromptBuilder;
}

export class CursorTeacher implements Teacher {
  readonly #command: string;
  readonly #model?: string;
  readonly #cwd?: string;
  readonly #runner: CommandRunner;
  readonly #tracker: SourceReliabilityTracker;
  readonly #now: Clock;
  readonly #idFactory: IdFactory;
  readonly #source: string;
  readonly #systemPrompt: string;
  readonly #promptBuilder: PromptBuilder;

  constructor(options: CursorTeacherOptions = {}) {
    this.#command = options.command ?? "agent";
    this.#model = options.model;
    this.#cwd = options.cwd;
    this.#runner = options.runner ?? runCommand;
    this.#tracker =
      options.reliabilityTracker ?? new SourceReliabilityTracker();
    this.#now = options.now ?? defaultClock;
    this.#idFactory = options.idFactory ?? defaultIdFactory;
    this.#source = `cursor:${this.#model ?? "default"}`;
    this.#systemPrompt = options.systemPrompt ?? TEACHER_SYSTEM_PROMPT;
    this.#promptBuilder = options.promptBuilder ?? buildTeacherPrompt;
  }

  async propose(request: TeacherRequest): Promise<TeacherProposal> {
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
        this.#systemPrompt,
        this.#promptBuilder(request),
        "Return only the requested JSON object.",
      ].join("\n\n"),
    );

    const result = await atProviderBoundary(
      "cursor",
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
        "cursor",
        `command exited with status ${result.exitCode}: ${result.stderr.trim() || "unknown error"}`,
      );
    }

    return makeProposal({
      content: parseCursorResult(result.stdout),
      provider: "cursor",
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

export function parseCursorResult(stdout: string): JsonValue {
  let envelope: unknown;
  try {
    envelope = JSON.parse(stdout);
  } catch (error) {
    throw new TeacherError(
      "cursor",
      "command did not return a valid JSON result envelope",
      { cause: error },
    );
  }
  if (typeof envelope !== "object" || envelope === null) {
    throw new TeacherError("cursor", "result envelope did not contain result");
  }
  const record = envelope as Record<string, unknown>;
  if (record.is_error === true || record.subtype === "error") {
    const detail =
      typeof record.result === "string" && record.result.trim() !== ""
        ? record.result.trim()
        : "unknown error";
    throw new TeacherError("cursor", `command reported an error: ${detail}`);
  }
  if (!("result" in record)) {
    throw new TeacherError("cursor", "result envelope did not contain result");
  }
  const content = record.result;
  if (isJsonValue(content) && typeof content !== "string") return content;
  if (typeof content !== "string") {
    throw new TeacherError("cursor", "result was not JSON-compatible");
  }
  try {
    return parseCliJson(content);
  } catch (error) {
    if (error instanceof TeacherError) {
      throw new TeacherError("cursor", error.message.replace(/^cli: /, ""));
    }
    throw error;
  }
}
